#!/usr/bin/env python3
"""python3 skills/blueprint/tests/test_progress.py

progress.py writes progress.json whole (lock, temp file, rename), so readers never see half a file and
writers running at once never lose each other's changes.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, '..', 'scripts', 'progress.py')
TEMPLATE = os.path.join(HERE, '..', 'assets', 'site', 'progress.json')
ENV = {**os.environ, 'PYTHONDONTWRITEBYTECODE': '1'}


class ProgressTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix='bp-progress-')
        self.file = os.path.join(self.dir, 'progress.json')
        shutil.copy(TEMPLATE, self.file)

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def run_cmd(self, *args):
        return subprocess.run([sys.executable, SCRIPT, '--file', self.file, *args], env=ENV, capture_output=True, text=True)

    def start(self, *args):
        return subprocess.Popen([sys.executable, SCRIPT, '--file', self.file, *args], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def read(self):
        with open(self.file, encoding='utf-8') as f:
            return json.load(f)

    def leftovers(self):
        return sorted(n for n in os.listdir(self.dir) if n != 'progress.json')

    def test_commands_keep_working(self):
        for args in (['task', 'r1-sync', 'done', 'synced'], ['add', 'R2', 'r2-extra', 'One more task'],
                     ['link', 'r2-extra', 'https://example.com/pr/1'], ['now', 'Checking'], ['log', 'A line']):
            r = self.run_cmd(*args)
            self.assertEqual(r.returncode, 0, r.stderr)
        d = self.read()
        r1, r2 = d['releases'][0], d['releases'][1]
        self.assertEqual([t['status'] for t in r1['tasks'] if t['id'] == 'r1-sync'], ['done'])
        self.assertTrue(any(t['id'] == 'r2-extra' and t['link'] == 'https://example.com/pr/1' for t in r2['tasks']))
        self.assertEqual(d['now'], 'Checking')
        self.assertEqual(d['log'][0]['text'], 'A line')
        self.assertEqual(self.leftovers(), [])

    def test_replaces_the_file_instead_of_writing_into_it(self):
        os.chmod(self.file, 0o640)
        before = os.stat(self.file).st_ino
        self.assertEqual(self.run_cmd('now', 'x').returncode, 0)
        st = os.stat(self.file)
        self.assertNotEqual(st.st_ino, before)
        self.assertEqual(st.st_mode & 0o777, 0o640)
        self.assertEqual(self.leftovers(), [])

    def test_a_refused_command_leaves_the_file_and_no_lock(self):
        with open(self.file, 'rb') as f:
            before = f.read()
        r = self.run_cmd('task', 'nope', 'done')
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('no task nope', r.stderr)
        with open(self.file, 'rb') as f:
            self.assertEqual(f.read(), before)
        self.assertEqual(self.leftovers(), [])

    def test_concurrent_writers_lose_nothing_and_readers_never_see_half_a_file(self):
        stop = threading.Event()
        bad = []
        reads = [0]

        def reader():
            while not stop.is_set():
                try:
                    with open(self.file, encoding='utf-8') as f:
                        json.load(f)
                    reads[0] += 1
                except ValueError as e:
                    bad.append(str(e))

        t = threading.Thread(target=reader)
        t.start()
        try:
            # eight workers at once, each running six commands one after another
            def worker(w):
                for _ in range(6):
                    r = self.run_cmd('log', f'w{w}')
                    if r.returncode:
                        bad.append(r.stderr)
            threads = [threading.Thread(target=worker, args=(w,)) for w in range(8)]
            for x in threads:
                x.start()
            for x in threads:
                x.join()
        finally:
            stop.set()
            t.join()
        self.assertEqual(bad, [])
        self.assertGreater(reads[0], 0)
        texts = [line['text'] for line in self.read()['log']]
        for w in range(8):
            self.assertEqual(texts.count(f'w{w}'), 6, texts[:60])
        self.assertEqual(self.leftovers(), [])

    def test_a_stale_lock_is_taken_over(self):
        lock = self.file + '.lock'
        open(lock, 'w').close()
        old = time.time() - 60
        os.utime(lock, (old, old))
        r = self.run_cmd('now', 'after a crash')
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.read()['now'], 'after a crash')
        self.assertFalse(os.path.exists(lock))

    def test_a_held_lock_is_waited_for(self):
        lock = self.file + '.lock'
        open(lock, 'w').close()
        p = self.start('now', 'waited')
        try:
            time.sleep(0.6)
            self.assertIsNone(p.poll())
            self.assertNotEqual(self.read()['now'], 'waited')
            os.unlink(lock)
            self.assertEqual(p.wait(timeout=10), 0)
        finally:
            p.kill()
            p.wait()
        self.assertEqual(self.read()['now'], 'waited')
        self.assertEqual(self.leftovers(), [])


if __name__ == '__main__':
    unittest.main()
