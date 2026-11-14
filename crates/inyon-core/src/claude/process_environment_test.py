import os
import subprocess
import sys
import time
import unittest
from process_environment import selected, process_proxies, process_start, process_executable


class EnvironmentTest(unittest.TestCase):
    def test_allowlist_preserves_values_and_excludes_other_environment(self):
        self.assertEqual(selected(['https_proxy=http://localhost/a=b', 'NO_PROXY=localhost,::1', 'PATH=/bin', 'API_KEY=private']),
                         {'https_proxy': 'http://localhost/a=b', 'no_proxy' if os.name == 'nt' else 'NO_PROXY': 'localhost,::1'})

    def test_reads_proxies_from_another_process_on_this_platform(self):
        env = os.environ.copy()
        env['http_proxy'] = 'http://127.0.0.1:12345'
        env['NO_PROXY'] = 'localhost,127.0.0.1'
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(10)'], env=env)
        try:
            time.sleep(.2)
            found = {k.lower(): v for k, v in process_proxies(child.pid).items()}
            self.assertEqual(found['http_proxy'], env['http_proxy'])
            self.assertEqual(found['no_proxy'], env['NO_PROXY'])
            self.assertTrue(set(found).issubset({'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'}))
            start = process_start(child.pid)
            self.assertTrue(start)
            self.assertEqual(process_start(child.pid), start)
            if os.name == 'nt' or sys.platform.startswith('linux'):
                self.assertTrue(os.path.samefile(process_executable(child.pid), sys.executable))
        finally:
            child.terminate()
            child.wait()


if __name__ == '__main__':
    unittest.main()
