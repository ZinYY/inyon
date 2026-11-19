"""Resolve a foreground TUI request among thread locks owned by that same process.

Codex retains idle, unsubscribed threads for 60 seconds. Its local diagnostic DB
records start/resume/fork/revert requests with a PID, process UUID and thread ID.
Only an ID already proven owned by this live process can be returned. No log
content leaves the machine, and the database is opened read-only.
"""
import os
import json
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import time


def observed_thread(conn, anchor, owners):
    """Carry a native observation only across a provably unchanged log interval.

    Codex keeps a bounded number of diagnostic rows per thread. The initial
    connection trace can disappear during a long conversation. Every live thread
    of this writer must still retain rows older than our last verified checkpoint;
    otherwise a pruned switch could be missed and we must fail closed.
    """
    pids = anchor.get('owner_pids', [])
    if len(pids) != 1:
        return None
    owner = str(pids[0])
    if anchor['id'] not in owners.get(owner, []):
        return None
    since = (anchor.get('checked_at') or anchor['at']) - 1
    process = None
    for tid in owners[owner]:
        rows = list(conn.execute("""
            SELECT process_uuid, min(ts) FROM logs
            WHERE thread_id=? AND process_uuid LIKE ? GROUP BY process_uuid
        """, (tid, f'pid:{owner}:%')))
        if len(rows) != 1 or rows[0][1] >= since:
            return None
        if process is not None and process != rows[0][0]:
            return None
        process = rows[0][0]
    rows = conn.execute("""
        SELECT feedback_log_body FROM logs WHERE process_uuid=? AND ts>=?
        AND target='codex_core::shell_snapshot'
    """, (process, int(since)))
    for (body,) in rows:
        if body and 'app_server.client_name="codex-tui"' in body and re.search(r'rpc.method="thread/(start|resume|fork|revert)"', body):
            return None
    return anchor['id']


def bound_thread(conn, started_at, anchor, owners):
    """Follow a connection previously identified by this TUI's native footer.

    A shared daemon owns several threads. The observed thread establishes a
    connection, not a permanent current-thread guess: /new, /resume and rewind
    can change that connection's foreground thread while the footer is hidden.
    """
    rows = conn.execute("""
        SELECT process_uuid, feedback_log_body FROM logs
        WHERE thread_id=? AND ts>=? AND ts<=? AND target='codex_core::shell_snapshot'
        ORDER BY ts DESC, ts_nanos DESC LIMIT 64
    """, (anchor['id'], int(started_at) - 5, int(anchor['at']) + 1))

    def fields(process, body):
        if not process or not body or 'rpc.transport="unix_socket"' not in body or 'app_server.client_name="codex-tui"' not in body:
            return None
        owner = re.match(r'^pid:(\d+):', process)
        connection = re.search(r'app_server.connection_id=(\d+)', body)
        if not owner or owner[1] not in owners or not connection or not re.search(r'rpc.method="thread/(start|resume|fork|revert)"', body):
            return None
        return process, connection[1], owner[1]

    bindings = {f for process, body in rows if (f := fields(process, body))}
    if len(bindings) != 1:
        return observed_thread(conn, anchor, owners) if not bindings else None
    process, connection, owner = next(iter(bindings))
    # Include unowned IDs in ordering: a newer thread absent from this probe must
    # not make us fall back to the previous thread's still-held writer lock.
    rows = conn.execute("""
        SELECT ts, ts_nanos, thread_id, feedback_log_body FROM logs
        WHERE process_uuid=? AND ts>=? AND target='codex_core::shell_snapshot'
        ORDER BY ts DESC, ts_nanos DESC LIMIT 256
    """, (process, int(started_at) - 5))
    events = []
    for ts, nanos, tid, body in rows:
        if fields(process, body) != (process, connection, owner):
            continue
        request = re.search(r'rpc.request_id=(?:"([^"]+)"|([^ ]+))', body)
        if not request:
            continue
        rid = request[1] or request[2]
        order = (1, int(rid)) if rid.isdigit() else (0, 0) if rid.startswith('startup-thread-start-') else None
        if order is not None:
            events.append((order, ts, nanos, tid))
    if not events:
        return observed_thread(conn, anchor, owners)
    selected = max(events)[3]
    return selected if selected in owners[owner] else None


def active_thread(conn, pid, started_at, owned, transport='in-process', single_connection=False):
    events = []
    connections = set()
    for tid in owned:
        rows = conn.execute("""
            SELECT ts, ts_nanos, process_uuid, feedback_log_body FROM logs
            WHERE thread_id=? AND ts>=? AND target='codex_core::shell_snapshot'
            AND process_uuid LIKE ? ORDER BY ts DESC, ts_nanos DESC LIMIT 12
        """, (tid, int(started_at) - 5, f'pid:{pid}:%'))
        for ts, nanos, process, body in rows:
            if not body or f'rpc.transport="{transport}"' not in body or 'app_server.client_name="codex-tui"' not in body:
                continue
            if not re.search(r'rpc.method="thread/(start|resume|fork|revert)"', body):
                continue
            request = re.search(r'rpc.request_id=(?:"([^"]+)"|([^ ]+))', body)
            if not request:
                continue
            connection = re.search(r'app_server.connection_id=(\d+)', body)
            if single_connection:
                if not connection:
                    return None
                connections.add(connection[1])
            rid = request[1] or request[2]
            # Numeric TUI request IDs increase even if an older asynchronous
            # snapshot finishes later. The named startup request precedes them.
            if rid.isdigit():
                order = (1, int(rid))
            elif rid.startswith('startup-thread-start-'):
                order = (0, 0)
            else:
                continue
            events.append((process, order, ts, nanos, tid))
    if not events or len({e[0] for e in events}) != 1 or (single_connection and len(connections) != 1):
        return None
    return max(events, key=lambda e: (e[1], e[2], e[3]))[4]


def shared_daemon_pid(output, pid):
    """A single live Codex TUI client of a Unix daemon is unambiguous.

    Kernel socket peer inodes establish the relationship, never directories or
    thread names. Multiple clients/owners deliberately yield no identity.
    """
    rows = {}
    for line in output.splitlines():
        cols = line.split()
        if len(cols) < 9 or cols[0] != 'u_str' or cols[1] != 'ESTAB':
            continue
        processes = re.findall(r'\("([^"]+)",pid=(\d+),', ' '.join(cols[8:]))
        owners = {int(p) for _, p in processes}
        tui_owners = {int(p) for name, p in processes if name == 'codex'}
        rows[cols[5]] = (cols[4], cols[7], owners, tui_owners)
    servers = set()
    for inode, (_, peer, owners, _) in rows.items():
        server = rows.get(peer)
        if owners != {pid} or not server or not server[0].startswith(f'/tmp/codex-daemon-{os.getuid()}/'):
            continue
        if server[1] != inode or len(server[2]) != 1:
            continue
        server_pid = next(iter(server[2]))
        clients = set()
        for address, other, owning, _ in rows.values():
            if owning == {server_pid} and address == server[0]:
                if other not in rows or len(rows[other][2]) != 1:
                    return None
                clients.update(rows[other][3])
        if clients == {pid}:
            servers.add(server_pid)
    return next(iter(servers)) if len(servers) == 1 else None


def daemon_owner(pid):
    if sys.platform == 'darwin':
        return mac_daemon_owner(pid)
    if not sys.platform.startswith('linux'):
        return None
    try:
        result = subprocess.run(['ss', '-xnpH'], capture_output=True, text=True, timeout=1)
        server = shared_daemon_pid(result.stdout, pid) if result.returncode == 0 else None
        if server is None:
            return None
        proc = Path('/proc') / str(server)
        if proc.joinpath('exe').resolve().name != 'codex':
            return None
        owned = []
        for fd in proc.joinpath('fd').iterdir():
            try:
                target = fd.readlink()
                if target.parent.name == 'thread-writer-locks' and target.suffix == '.lock':
                    owned.append(target.stem)
            except OSError:
                pass
        stat = proc.joinpath('stat').read_text().rsplit(')', 1)[1].split()
        boot = next(int(line.split()[1]) for line in Path('/proc/stat').read_text().splitlines() if line.startswith('btime '))
        started = boot + int(stat[19]) / os.sysconf('SC_CLK_TCK')
        return server, started, sorted(set(owned))
    except (OSError, ValueError, StopIteration, subprocess.TimeoutExpired):
        return None


def mac_shared_daemon_pid(output, pid, uid):
    """lsof's Unix socket pointer links a TUI to one named daemon endpoint."""
    rows = []
    for line in output.splitlines():
        cols = line.split(None, 7)
        if len(cols) == 8 and cols[1].isdigit() and cols[4] == 'unix':
            rows.append((int(cols[1]), cols[0], cols[5], cols[7]))
    prefixes = (f'/private/tmp/codex-daemon-{uid}/', f'/tmp/codex-daemon-{uid}/')
    servers = {owner for owner, name, device, path in rows if name == 'codex' and path.startswith(prefixes)
               and any(client == pid and peer == '->' + device for client, _, _, peer in rows)}
    if len(servers) != 1:
        return None
    server = next(iter(servers))
    endpoints = {device for owner, _, device, path in rows if owner == server and path.startswith(prefixes)}
    # Inyon's node helper also connects here briefly for live output. Only Codex TUI clients
    # participate in foreground identity; the log query independently requires client_name=codex-tui.
    clients = {owner for owner, name, _, peer in rows if name == 'codex' and peer.startswith('->') and peer[2:] in endpoints}
    return server if clients == {pid} else None


def mac_daemon_owner(pid):
    try:
        sockets = subprocess.run(['lsof', '-nP', '-U', '-a', '-u', str(os.getuid())], capture_output=True, text=True, timeout=1)
        server = mac_shared_daemon_pid(sockets.stdout, pid, os.getuid())
        if server is None:
            return None
        files = subprocess.run(['lsof', '-nP', '-a', '-p', str(server), '-Fn'], capture_output=True, text=True, timeout=1)
        owned = sorted(set(re.findall(r'^n.*/thread-writer-locks/([0-9a-f-]{36})\.lock$', files.stdout, re.M)))
        elapsed = subprocess.check_output(['ps', '-p', str(server), '-o', 'etime='], text=True, timeout=1).strip()
        days, elapsed = elapsed.split('-', 1) if '-' in elapsed else ('0', elapsed)
        parts = [int(n) for n in elapsed.split(':')]
        seconds = sum(n * 60 ** i for i, n in enumerate(reversed(parts))) + int(days) * 86400
        return server, time.time() - seconds, owned
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def main():
    if sys.argv[1] == '--binding':
        home, started_at, anchor, owners = json.loads(sys.argv[2])
        if os.name == 'nt' and re.match(r'^/[a-zA-Z]/', home):
            home = home[1] + ':' + home[2:]
        path = Path(home) / '.codex' / 'logs_2.sqlite'
        deadline = time.monotonic() + 1.0
        try:
            with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=0.1) as conn:
                conn.execute('PRAGMA query_only=ON')
                conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
                conn.execute('BEGIN')  # Retention checks and switch reads share one snapshot.
                selected = bound_thread(conn, started_at, anchor, owners)
                if selected:
                    print(selected)
        except (OSError, ValueError, sqlite3.Error):
            pass
        return
    home, pid, started_at, *owned = sys.argv[1:]
    pid, started_at = int(pid), float(started_at)
    shared = False
    if not owned:
        daemon = daemon_owner(pid)
        if not daemon:
            return
        pid, daemon_started, owned = daemon
        # The daemon can outlive many TUI processes. Only this TUI's lifetime
        # may contribute foreground requests, even when an older connection
        # used a thread that the current TUI has just resumed.
        started_at = max(started_at, daemon_started)
        shared = True
    if os.name == 'nt' and re.match(r'^/[a-zA-Z]/', home):
        home = home[1] + ':' + home[2:]
    path = Path(home) / '.codex' / 'logs_2.sqlite'
    deadline = time.monotonic() + 1.0
    try:
        with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=0.1) as conn:
            conn.execute('PRAGMA query_only=ON')
            conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
            selected = active_thread(conn, pid, started_at, owned, 'unix_socket' if shared else 'in-process', shared)
            if selected:
                print(selected)
    except (OSError, ValueError, sqlite3.Error):
        pass


if __name__ == '__main__':
    main()
