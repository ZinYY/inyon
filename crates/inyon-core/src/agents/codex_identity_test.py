import sqlite3
import unittest
from codex_identity import active_thread


class ForegroundThreadTest(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:')
        self.db.execute('CREATE TABLE logs (ts INTEGER, ts_nanos INTEGER, process_uuid TEXT, feedback_log_body TEXT, thread_id TEXT, target TEXT)')

    def tearDown(self):
        self.db.close()

    def add(self, thread, request, *, ts=1000, process='pid:42:current', client='codex-tui', method='resume', target='codex_core::shell_snapshot'):
        body = f'rpc.transport="in-process" app_server.client_name="{client}" rpc.method="thread/{method}" rpc.request_id={request}: session_init'
        # The real trace has a space before the next field or span delimiter.
        body = body.replace(f'{request}:', f'{request} app_server.connection_id=0:')
        self.db.execute('INSERT INTO logs VALUES (?,?,?,?,?,?)', (ts, 0, process, body, thread, target))

    def test_resume_back_selects_the_latest_request_not_the_newest_thread(self):
        self.add('parent', 7, ts=900)
        self.add('fork', 27, ts=1000, method='fork')
        self.add('parent', 34, ts=1100)
        self.assertEqual(active_thread(self.db, 42, 800, ['parent', 'fork']), 'parent')

    def test_late_snapshot_completion_does_not_restore_the_previous_thread(self):
        self.add('old', 7, ts=1300)
        self.add('new', 12, ts=1200, method='start')
        self.assertEqual(active_thread(self.db, 42, 800, ['old', 'new']), 'new')

    def test_unowned_threads_other_processes_and_old_pid_instances_are_ignored(self):
        self.add('parent', 1)
        self.add('unowned', 99)
        self.add('fork', 100, process='pid:43:other')
        self.add('fork', 200, process='pid:42:previous', ts=700)
        self.assertEqual(active_thread(self.db, 42, 800, ['parent', 'fork']), 'parent')

    def test_shared_servers_and_subagents_are_not_foreground_tui_switches(self):
        self.add('parent', 7, client='other-client')
        self.add('fork', 8, method='turn/start')
        self.assertIsNone(active_thread(self.db, 42, 800, ['parent', 'fork']))

    def test_background_cleanup_cannot_choose_an_old_thread(self):
        self.add('parent', 3, target='codex_core::session::handlers', ts=1200)
        self.add('new', 'startup-thread-start-uuid', method='start')
        self.assertEqual(active_thread(self.db, 42, 800, ['parent', 'new']), 'new')

    def test_ambiguous_process_instances_fail_closed(self):
        self.add('parent', 7)
        self.add('fork', 8, process='pid:42:another')
        self.assertIsNone(active_thread(self.db, 42, 800, ['parent', 'fork']))


if __name__ == '__main__':
    unittest.main()
