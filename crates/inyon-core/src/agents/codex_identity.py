"""Resolve a foreground TUI request among thread locks owned by that same process.

Codex retains idle, unsubscribed threads for 60 seconds. Its local diagnostic DB
records start/resume/fork/revert requests with a PID, process UUID and thread ID.
Only an ID already proven owned by this live process can be returned. No log
content leaves the machine, and the database is opened read-only.
"""
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import time


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
