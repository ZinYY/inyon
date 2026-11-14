"""Read completed Markdown from this TUI's existing diagnostic log, read-only.

No logging setting is changed. A source is emitted only if its UTF-8 length
exactly matches Codex's consolidation record. Partial/rotated logs are skipped.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import threading
import time

STREAM = 'codex_tui::markdown_stream'
DONE = 'codex_tui::app::agent_message_consolidation'
MAX_BYTES = 256 * 1024


class Sources:
    def __init__(self):
        self.pending = bytearray()
        self.seen = []

    def apply(self, target, body):
        if target == STREAM and body.startswith('push_delta: '):
            try:
                # Rust Debug strings use JSON escapes, plus \u{...} and \0.
                encoded = body[len('push_delta: '):]
                encoded = re.sub(r'(?<!\\)\\u\{([0-9a-fA-F]+)\}', lambda m: chr(int(m[1], 16)), encoded)
                encoded = re.sub(r'(?<!\\)\\0', r'\\u0000', encoded)
                delta = json.loads(encoded)
                if not isinstance(delta, str):
                    raise ValueError('not text')
                self.pending.extend(delta.encode())
                if len(self.pending) > MAX_BYTES:
                    self.pending.clear()
            except (ValueError, UnicodeError):
                self.pending.clear()
        elif target == DONE and (match := re.search(r'\bsource_len=(\d+)\b', body)):
            raw, self.pending = self.pending, bytearray()
            if not raw or len(raw) != int(match[1]):
                return None
            digest = hashlib.sha256(raw).digest()
            if digest in self.seen:
                return None
            self.seen = (self.seen + [digest])[-64:]
            return raw.decode()
        return None


def main():
    sys.stdout.reconfigure(encoding='utf-8')
    pid, started = int(sys.argv[1]), float(sys.argv[2])
    path = Path(os.environ.get('CODEX_HOME', str(Path.home() / '.codex'))) / 'logs_2.sqlite'
    stop = threading.Event()
    def input_closed():
        sys.stdin.buffer.read()
        stop.set()
    threading.Thread(target=input_closed, daemon=True).start()
    with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=0.1) as conn:
        conn.execute('PRAGMA query_only=ON')
        deadline = time.monotonic() + 1
        conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
        process = conn.execute('''SELECT process_uuid FROM logs
            WHERE thread_id IS NULL AND process_uuid GLOB ? AND ts>=?
            ORDER BY ts DESC, ts_nanos DESC LIMIT 1''', (f'pid:{pid}:*', int(started)-5)).fetchone()
        if not process:
            return
        process = process[0]
        cursor = conn.execute('SELECT coalesce(max(id),0) FROM logs').fetchone()[0]
        rows = conn.execute('''SELECT id,target,feedback_log_body FROM logs
            WHERE process_uuid=? AND thread_id IS NULL AND id<=?
            AND target IN (?,?) ORDER BY ts DESC,ts_nanos DESC LIMIT 8192''',
            (process, cursor, STREAM, DONE)).fetchall()
        parser = Sources()
        def emit(rows):
            for _, target, body in sorted(rows):
                text = parser.apply(target, body or '')
                if text is not None:
                    print(json.dumps({'source': text}, ensure_ascii=False), flush=True)
        emit(rows)
        while not stop.wait(1):
            deadline = time.monotonic() + 0.5
            high = min(conn.execute('SELECT coalesce(max(id),0) FROM logs').fetchone()[0], cursor + 20000)
            if high < cursor:  # rotated database: never concatenate unrelated chunks
                return
            # Primary-key ranges advance even when only other processes logged.
            rows = conn.execute('''SELECT id,target,feedback_log_body FROM logs NOT INDEXED
                WHERE id>? AND id<=? AND process_uuid=? AND thread_id IS NULL
                AND target IN (?,?) ORDER BY id''', (cursor, high, process, STREAM, DONE)).fetchall()
            cursor = high
            emit(rows)


if __name__ == '__main__':
    try:
        main()
    except (OSError, sqlite3.Error, ValueError):
        pass  # Screen capture remains available when the diagnostic source is absent.
