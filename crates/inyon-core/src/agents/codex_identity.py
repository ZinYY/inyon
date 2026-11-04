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
import sys
import time


def active_thread(conn, pid, started_at, owned):
    events = []
    for tid in owned:
        rows = conn.execute("""
            SELECT ts, ts_nanos, process_uuid, feedback_log_body FROM logs
            WHERE thread_id=? AND ts>=? AND target='codex_core::shell_snapshot'
            AND process_uuid LIKE ? ORDER BY ts DESC, ts_nanos DESC LIMIT 12
        """, (tid, int(started_at) - 5, f'pid:{pid}:%'))
        for ts, nanos, process, body in rows:
            if not body or 'rpc.transport="in-process"' not in body or 'app_server.client_name="codex-tui"' not in body:
                continue
            if not re.search(r'rpc.method="thread/(start|resume|fork|revert)"', body):
                continue
            request = re.search(r'rpc.request_id=(?:"([^"]+)"|([^ ]+))', body)
            if not request:
                continue
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
    if not events or len({e[0] for e in events}) != 1:
        return None
    return max(events, key=lambda e: (e[1], e[2], e[3]))[4]


def main():
    home, pid, started_at, *owned = sys.argv[1:]
    if os.name == 'nt' and re.match(r'^/[a-zA-Z]/', home):
        home = home[1] + ':' + home[2:]
    path = Path(home) / '.codex' / 'logs_2.sqlite'
    deadline = time.monotonic() + 1.0
    try:
        with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=0.1) as conn:
            conn.execute('PRAGMA query_only=ON')
            conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
            selected = active_thread(conn, int(pid), float(started_at), owned)
            if selected:
                print(selected)
    except (OSError, ValueError, sqlite3.Error):
        pass


if __name__ == '__main__':
    main()
