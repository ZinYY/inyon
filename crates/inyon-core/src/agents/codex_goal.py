"""Follow one thread's authoritative goal without joining or changing its agent.

Automatic budget/usage transitions are not always written to the rollout. Only
query the indexed thread row when the DB or WAL changes; send semantic changes,
not token accounting updates. Missing/locked/unsupported stores are unknown.
"""
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import threading
import time
from datetime import datetime, timezone


def native_path(path):
    if os.name == 'nt' and re.match(r'^/[a-zA-Z]/', path):
        path = path[1] + ':' + path[2:]
    return Path(path)


def read_goal(path, thread):
    with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=.1) as conn:
        conn.execute('PRAGMA query_only=ON')
        deadline = time.monotonic() + .2
        conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
        row = conn.execute('SELECT objective,status,updated_at_ms FROM thread_goals WHERE thread_id=?', (thread,)).fetchone()
    if row is None:
        return None
    objective, status, at = row
    if not isinstance(objective, str) or not objective.strip() or not isinstance(status, str) or not status.strip():
        raise ValueError('Incomplete goal row')
    return dict(condition=objective, status=status, met=status == 'complete',
                ts=datetime.fromtimestamp(at / 1000, timezone.utc).isoformat(timespec='milliseconds').replace('+00:00', 'Z'))


def stamp(path):
    result = []
    for item in [path, Path(str(path) + '-wal')]:
        try:
            s = item.stat()
            result.append((s.st_ino, s.st_size, s.st_mtime_ns))
        except FileNotFoundError:
            result.append(None)
    return result


def main():
    sys.stdout.reconfigure(encoding='utf-8')
    path, thread = native_path(sys.argv[1]), sys.argv[2]
    stopped = threading.Event()
    def input_closed():
        try:
            while sys.stdin.buffer.read(1):
                pass
        finally:
            stopped.set()
    threading.Thread(target=input_closed, daemon=True).start()
    previous_stamp, previous_value = None, object()
    while not stopped.is_set():
        try:
            current_stamp = stamp(path)
            if current_stamp != previous_stamp:
                goal = read_goal(path, thread)
                # Ignore changes to accounting timestamps when state is unchanged.
                value = None if goal is None else (goal['condition'], goal['status'])
                if value != previous_value:
                    print(json.dumps(dict(threadId=thread, goal=goal), ensure_ascii=False), flush=True)
                    previous_value = value
                previous_stamp = current_stamp
        except (OSError, ValueError, TypeError, OverflowError, sqlite3.Error):
            pass
        stopped.wait(1)


if __name__ == '__main__':
    main()
