import json
from pathlib import Path
import queue
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest
from codex_goal import read_goal


class GoalStoreTest(unittest.TestCase):
    def test_read_only_missing_database_is_unknown_not_clear(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'missing.sqlite'
            with self.assertRaises(sqlite3.Error):
                read_goal(path, 'test')
            self.assertFalse(path.exists())

    def test_wal_transitions_are_scoped_quiet_and_stop_on_stdin_close(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'goals_1.sqlite'
            conn = sqlite3.connect(path)
            conn.execute('PRAGMA journal_mode=WAL')
            conn.execute('CREATE TABLE thread_goals(thread_id TEXT PRIMARY KEY, objective TEXT, status TEXT, updated_at_ms INTEGER)')
            conn.execute('INSERT INTO thread_goals VALUES(?,?,?,?)', ('test', '目标', 'active', 1000))
            conn.execute('INSERT INTO thread_goals VALUES(?,?,?,?)', ('other', 'Other', 'complete', 1000))
            conn.commit()
            proc = subprocess.Popen([sys.executable, '-u', str(Path(__file__).with_name('codex_goal.py')), str(path), 'test'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
            lines = queue.Queue()
            def collect():
                for line in proc.stdout:
                    lines.put(json.loads(line))
            threading.Thread(target=collect, daemon=True).start()
            try:
                self.assertEqual(lines.get(timeout=5)['goal']['status'], 'active')
                for status in ['paused', 'active', 'blocked', 'usage_limited', 'budget_limited', 'complete']:
                    conn.execute('UPDATE thread_goals SET status=?,updated_at_ms=updated_at_ms+1000 WHERE thread_id=?', (status, 'test')); conn.commit()
                    goal = lines.get(timeout=5)['goal']
                    self.assertEqual((goal['condition'], goal['status'], goal['met']), ('目标', status, status == 'complete'))
                conn.execute('UPDATE thread_goals SET updated_at_ms=updated_at_ms+1000'); conn.commit()
                with self.assertRaises(queue.Empty):
                    lines.get(timeout=1.5)
                conn.execute('DELETE FROM thread_goals WHERE thread_id=?', ('test',)); conn.commit()
                self.assertIsNone(lines.get(timeout=5)['goal'])
                proc.stdin.close(); proc.wait(timeout=3)
                self.assertEqual(proc.returncode, 0, proc.stderr.read())
            finally:
                if proc.poll() is None: proc.kill(); proc.wait()
                for pipe in [proc.stdin, proc.stdout, proc.stderr]: pipe.close()
                conn.close()


if __name__ == '__main__':
    unittest.main()
