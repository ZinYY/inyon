import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest


SCRIPT = Path(__file__).with_name('claude-auto-title.py')
spec = importlib.util.spec_from_file_location('auto_title', SCRIPT)
title = importlib.util.module_from_spec(spec)
spec.loader.exec_module(title)


class AutoTitleTest(unittest.TestCase):
    def test_complete_json_does_not_wait_for_stdin_eof(self):
        code = 'import runpy,sys; m=runpy.run_path(sys.argv[1]); print(m["read_event"](sys.stdin.buffer)["session_id"], flush=True)'
        process = subprocess.Popen([sys.executable, '-c', code, str(SCRIPT)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        try:
            process.stdin.write(json.dumps({'session_id': 'current', 'prompt': 'Explain machine learning'}).encode())
            process.stdin.flush()  # Deliberately keep stdin open until after the hook exits.
            process.wait(timeout=2)
            self.assertEqual(process.stdout.read().strip(), b'current')
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stdin.close()
            process.stdout.close()

    def test_timeout_output_can_be_retried_without_overwriting_a_user_name(self):
        state = {'sessions': {'s': {'project': 'QYY', 'title': 'Machine learning'}}, 'managed_titles': {'QYY': ['Machine learning']}}
        self.assertEqual(title.choose_title('QYY', 's', 'Another question', 'QYY', ['First question'], state), 'Machine learning')
        self.assertIsNone(title.choose_title('QYY', 's', 'Another question', 'Machine learning', [], state))
        self.assertIsNone(title.choose_title('QYY', 's', 'Another question', 'My deliberate name', [], state))

    def test_a_new_session_after_clear_can_replace_its_inherited_managed_title(self):
        state = {'sessions': {'old': {'project': 'QYY', 'title': 'Machine learning'}}, 'managed_titles': {'QYY': ['Machine learning']}}
        self.assertEqual(title.choose_title('QYY', 'new', 'Cross validation', 'Machine learning', [], state), 'Cross validation')
        self.assertIsNone(title.choose_title('QYY', 'new', '/resume', 'Machine learning', [], state))

    def test_input_is_bounded_and_invalid_objects_are_ignored(self):
        self.assertIsNone(title.read_event(io.BytesIO(b'{' + b' ' * (2 * 1024 * 1024))))
        self.assertIsNone(title.read_event(io.BytesIO(b'[]')))


if __name__ == '__main__':
    unittest.main()
