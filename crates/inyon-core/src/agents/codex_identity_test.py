import sqlite3
import unittest
import os
import contextlib
import io
from pathlib import Path
import tempfile
from unittest.mock import patch
from codex_identity import active_thread, bound_thread, observed_thread, shared_daemon_pid, mac_shared_daemon_pid, main


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

    def test_shared_daemon_uses_only_a_single_foreground_connection(self):
        self.add('parent', 7)
        self.add('fork', 27, method='fork')
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'in-process', 'unix_socket')")
        self.assertEqual(active_thread(self.db, 42, 800, ['parent', 'fork'], 'unix_socket', True), 'fork')
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'connection_id=0', 'connection_id=1') WHERE thread_id='parent'")
        self.assertIsNone(active_thread(self.db, 42, 800, ['parent', 'fork'], 'unix_socket', True))

    def test_a_new_tui_resuming_an_old_thread_ignores_the_previous_tui_connection(self):
        self.add('parent', 7, ts=900)
        self.add('empty-startup', 'startup-thread-start-current', ts=1100, method='start')
        self.add('parent', 8, ts=1101)
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'in-process', 'unix_socket')")
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'connection_id=0', 'connection_id=1') WHERE ts<1000")
        self.db.commit()
        with tempfile.TemporaryDirectory() as home:
            folder = Path(home) / '.codex'
            folder.mkdir()
            with sqlite3.connect(folder / 'logs_2.sqlite') as disk:
                self.db.backup(disk)
            output = io.StringIO()
            with patch('sys.argv', ['identity', home, '99', '1000']), \
                    patch('codex_identity.daemon_owner', return_value=(42, 800, ['parent', 'empty-startup'])), \
                    contextlib.redirect_stdout(output):
                main()
            self.assertEqual(output.getvalue().strip(), 'parent')

    def test_kernel_socket_peers_identify_one_daemon_client_without_folder_matching(self):
        path = f'/tmp/codex-daemon-{os.getuid()}/server'
        rows = f'''u_str ESTAB 0 0 * 101 * 201 users:(("codex",pid=42,fd=4))
u_str ESTAB 0 0 {path} 201 * 101 users:(("codex",pid=90,fd=4))'''
        self.assertEqual(shared_daemon_pid(rows, 42), 90)
        self.assertIsNone(shared_daemon_pid(rows, 43))
        another = f'''\nu_str ESTAB 0 0 * 102 * 202 users:(("codex",pid=43,fd=4))
u_str ESTAB 0 0 {path} 202 * 102 users:(("codex",pid=90,fd=5))'''
        self.assertIsNone(shared_daemon_pid(rows + another, 42))
        self.assertEqual(shared_daemon_pid(rows + another.replace('("codex",pid=43', '("node",pid=43'), 42), 90)
        self.assertIsNone(shared_daemon_pid(rows.replace(path, '/tmp/some-other-server'), 42))

    def test_macos_kernel_endpoints_require_exactly_one_daemon_client(self):
        rows = '''codex 90 user 29u unix 0x111 0t0 /private/tmp/codex-daemon-501/server
codex 90 user 32u unix 0x222 0t0 /private/tmp/codex-daemon-501/server
codex 42 user 39u unix 0x333 0t0 ->0x222'''
        self.assertEqual(mac_shared_daemon_pid(rows, 42, 501), 90)
        self.assertIsNone(mac_shared_daemon_pid(rows, 43, 501))
        self.assertIsNone(mac_shared_daemon_pid(rows, 42, 502))
        self.assertIsNone(mac_shared_daemon_pid(rows + '\ncodex 43 user 4u unix 0x444 0t0 ->0x222', 42, 501))
        self.assertEqual(mac_shared_daemon_pid(rows + '\nnode 43 user 4u unix 0x444 0t0 ->0x222', 42, 501), 90)
        self.assertIsNone(mac_shared_daemon_pid(rows.replace('/private/tmp/codex-daemon-501/', '/tmp/another/'), 42, 501))

    def binding(self):
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'in-process', 'unix_socket')")
        return bound_thread(self.db, 800, {'id': 'parent', 'at': 1001}, {'42': ['parent', 'new', 'other']})

    def test_hidden_footer_follows_its_connection_without_borrowing_another_tui(self):
        self.add('parent', 7)
        self.add('new', 12, ts=1100, method='start')
        self.add('other', 90, ts=1150)
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'connection_id=0', 'connection_id=2') WHERE thread_id='other'")
        self.assertEqual(self.binding(), 'new')
        self.add('parent', 15, ts=1200)
        self.assertEqual(self.binding(), 'parent')

    def test_anchor_can_outlive_its_lock_but_not_the_current_threads_lock(self):
        self.add('parent', 7)
        self.add('new', 12, ts=1100, method='start')
        self.binding()
        anchor = {'id': 'parent', 'at': 1001}
        self.assertEqual(bound_thread(self.db, 800, anchor, {'42': ['new']}), 'new')
        self.assertIsNone(bound_thread(self.db, 800, anchor, {'42': ['parent']}))
        self.assertIsNone(bound_thread(self.db, 800, anchor, {'43': ['new']}))
        self.assertIsNone(bound_thread(self.db, 1200, anchor, {'42': ['new']}))

    def test_an_ambiguous_native_thread_does_not_bind_two_tuis(self):
        self.add('parent', 7)
        self.add('parent', 8)
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'connection_id=0', 'connection_id=2') WHERE feedback_log_body LIKE '%request_id=8 %'")
        self.assertIsNone(self.binding())

    def test_a_later_client_resuming_the_anchor_does_not_change_the_binding(self):
        self.add('parent', 7)
        self.add('new', 12, ts=1100, method='start')
        self.add('parent', 99, ts=1200)
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'connection_id=0', 'connection_id=2') WHERE ts=1200")
        self.assertEqual(self.binding(), 'new')

    def test_hidden_footer_ignores_a_late_old_snapshot_and_non_tui_clients(self):
        self.add('parent', 7)
        self.add('new', 12, ts=1100, method='start')
        self.add('parent', 7, ts=1200)
        self.add('other', 99, ts=1300, client='other-client')
        self.assertEqual(self.binding(), 'new')

    def test_daemon_restart_requires_a_native_observation_after_its_resume(self):
        self.add('parent', 7)
        self.add('parent', 5, ts=1100, process='pid:43:replacement')
        self.db.execute("UPDATE logs SET feedback_log_body=replace(feedback_log_body, 'in-process', 'unix_socket')")
        owners = {'43': ['parent']}
        self.assertIsNone(bound_thread(self.db, 800, {'id': 'parent', 'at': 1001}, owners))
        self.assertEqual(bound_thread(self.db, 800, {'id': 'parent', 'at': 1101}, owners), 'parent')

    def observation(self):
        for tid in ['parent', 'other']:
            self.db.execute('INSERT INTO logs VALUES (?,?,?,?,?,?)',
                (1000, 0, 'pid:42:current', 'turn progress', tid, 'codex_core::session::turn'))
        return {'id': 'parent', 'at': 1100, 'owner_pids': [42]}, {'42': ['parent', 'other']}

    def test_pruned_startup_trace_uses_only_a_continuous_native_observation(self):
        anchor, owners = self.observation()
        self.assertEqual(bound_thread(self.db, 800, anchor, owners), 'parent')
        self.db.execute("DELETE FROM logs WHERE thread_id='parent'")
        self.db.execute('INSERT INTO logs VALUES (?,?,?,?,?,?)',
            (1200, 0, 'pid:42:current', 'turn progress', 'parent', 'codex_core::session::turn'))
        # Losing the interval means a switch could have been pruned: no guessing.
        self.assertIsNone(observed_thread(self.db, anchor, owners))
        anchor['checked_at'] = 1300
        self.assertEqual(observed_thread(self.db, anchor, owners), 'parent')

    def test_any_unbound_tui_switch_invalidates_observation_even_for_another_thread(self):
        anchor, owners = self.observation()
        self.add('other', 12, ts=1200)
        self.assertIsNone(bound_thread(self.db, 800, anchor, owners))
        # A native footer seen after the switch proves which thread this TUI shows.
        anchor['at'] = 1300
        self.assertEqual(bound_thread(self.db, 800, anchor, owners), 'parent')

    def test_observation_rejects_missing_locks_new_threads_and_reused_writer_pids(self):
        anchor, owners = self.observation()
        self.assertIsNone(observed_thread(self.db, anchor, {'43': ['parent']}))
        self.assertIsNone(observed_thread(self.db, anchor, {'42': ['other']}))
        self.assertIsNone(observed_thread(self.db, anchor, {'42': ['parent', 'new']}))
        self.db.execute('INSERT INTO logs VALUES (?,?,?,?,?,?)',
            (1001, 0, 'pid:42:replacement', 'turn progress', 'parent', 'codex_core::session::turn'))
        self.assertIsNone(observed_thread(self.db, anchor, owners))


if __name__ == '__main__':
    unittest.main()
