import fcntl
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('devin_store', Path(__file__).with_name('devin_store.py'))
devin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(devin)


class DevinStoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.data = self.root / 'native'
        self.data.mkdir()
        (self.data / 'session_locks').mkdir()
        (self.data / 'logs').mkdir()
        self.config = patch.multiple(devin, DATA=self.data, ROOT=self.root / 'inyon')
        self.config.start()
        self.db = sqlite3.connect(self.data / 'sessions.db')
        self.db.executescript('''
          CREATE TABLE sessions(id TEXT PRIMARY KEY,title TEXT,working_directory TEXT,model TEXT,agent_mode TEXT,last_activity_at INTEGER,main_chain_id INTEGER,workspace_dirs TEXT,hidden INTEGER DEFAULT 0);
          CREATE TABLE message_nodes(node_id INTEGER,parent_node_id INTEGER,chat_message TEXT,metadata TEXT,session_id TEXT);
          CREATE TABLE prompt_history(id INTEGER PRIMARY KEY,session_id TEXT,content TEXT);
        ''')

    def tearDown(self):
        self.db.close()
        self.config.stop()
        self.temp.cleanup()

    def session(self, sid='test-session', head=2):
        self.db.execute('INSERT INTO sessions VALUES(?,?,?,?,?,?,?,?,0)', (sid, 'Test', '/tmp/test', 'test-model', 'normal', 100, head, '[]'))

    def message(self, node, parent, role, text, sid='test-session', **meta):
        m = {'message_id': 'message-' + str(node), 'role': role, 'content': text, 'metadata': meta}
        self.db.execute('INSERT INTO message_nodes VALUES(?,?,?,?,?)', (node, parent, json.dumps(m), '{}', sid))

    def finished(self, sid='test-session'):
        self.session(sid)
        self.message(1, None, 'user', 'hello', sid, is_user_input=True)
        self.message(2, 1, 'assistant', 'answer', sid, finish_reason='stop')
        self.db.commit()

    def test_native_database_is_never_writable(self):
        self.finished()
        with devin.connect() as db:
            with self.assertRaises(sqlite3.OperationalError):
                db.execute('DELETE FROM sessions')

    def test_tmux_names_keep_chinese_spaces_and_parentheses(self):
        self.assertEqual(devin.tmux_name('测试会话 (fork)', 'sample-session'), 'devin-测试会话 (fork)')
        self.assertEqual(devin.tmux_name('one:two.three', 'sample-session'), 'devin-one_two_three')
        self.assertEqual(devin.tmux_name('', 'sample-session'), 'devin-sample-session')

    def test_a_stale_placeholder_above_a_current_draft_is_not_an_empty_composer(self):
        idle = '── title\n❭ Ask Devin to build features, fix bugs, or work on your code\n──\nModel'
        self.assertTrue(devin.composer_ready(idle))
        self.assertFalse(devin.composer_ready(idle + '\n── title\n❭ unsent user draft\n──\nModel'))
        self.assertFalse(devin.composer_ready(idle.replace('❭ Ask Devin', '❭ Guide Devin')))

    def test_selected_branch_excludes_abandoned_and_internal_messages(self):
        self.finished()
        self.message(3, 1, 'assistant', 'abandoned')
        self.message(4, 2, 'system', 'internal context')
        self.db.execute('UPDATE sessions SET main_chain_id=4')
        self.db.commit()
        with devin.connect() as db:
            _, messages = devin.history(db, 'test-session')
            self.assertEqual([m['text'] for m in messages.values()], ['hello', 'answer'])
            self.assertTrue(devin.completed_head(db, 'test-session'))

    def test_unfinished_user_turn_is_not_ready_even_without_a_process(self):
        self.finished()
        self.message(3, 2, 'user', 'still pending', is_user_input=True)
        self.db.execute('UPDATE sessions SET main_chain_id=3')
        self.db.commit()
        with devin.connect() as db:
            with self.assertRaises(devin.Refused):
                devin.owner_ready(db, 'test-session')

    def test_imported_desktop_reply_needs_independent_idle_evidence(self):
        self.session()
        self.message(1, None, 'user', 'question', is_user_input=True)
        self.message(2, 1, 'assistant', 'Desktop reply without a finish_reason')
        self.db.commit()
        self.assertFalse(devin.completed_head(self.db, 'test-session'))
        self.assertTrue(devin.completed_head(self.db, 'test-session', native_idle=True))
        with patch.object(devin, 'lock_owner', return_value=123), patch.object(devin, 'log_idle', return_value=False):
            with self.assertRaises(devin.Refused):
                devin.owner_ready(self.db, 'test-session')

    def test_long_graph_and_compaction_links_preserve_source_history(self):
        rows = [(i, i - 1 if i else None, '{}', '{}') for i in range(6000)]
        rows.append((6000, None, '{}', json.dumps({'extensions': {'compact/prior_node_ids': [5999]}})))
        self.assertEqual(len(devin.selected_nodes(rows, 6000)), 6001)
        with self.assertRaises(devin.Refused):
            devin.selected_nodes([(1, 2, '{}', '{}'), (2, 1, '{}', '{}')], 1)

    def test_stale_lock_pid_does_not_mean_a_live_owner(self):
        (self.data / 'session_locks/test-session.lock').write_text(str(os.getpid()))
        self.assertIsNone(devin.lock_owner('test-session'))

    def test_busy_owner_blocks_before_any_tmux_write(self):
        self.finished()
        with patch.object(devin, 'lock_owner', return_value=123), patch.object(devin, 'log_idle', return_value=False), patch.object(devin, 'tmux') as tmux:
            with self.assertRaisesRegex(devin.Refused, 'previous response'):
                devin.send_message({'id': 'test-session', 'text': 'hello', 'requestId': 'request-1'})
            tmux.assert_not_called()

    def test_unknown_log_state_fails_closed(self):
        self.finished()
        with patch.object(devin, 'lock_owner', return_value=123), patch.object(devin, 'log_idle', return_value=None):
            with self.assertRaisesRegex(devin.Refused, 'confirmed finished'):
                devin.owner_ready(self.db, 'test-session')

    def test_another_unfinished_session_owned_by_same_process_blocks_force(self):
        self.finished()
        self.finished('other-session')
        self.message(3, 2, 'user', 'pending', 'other-session', is_user_input=True)
        self.db.execute('UPDATE sessions SET main_chain_id=3 WHERE id=?', ('other-session',))
        self.db.commit()
        (self.data / 'session_locks/other-session.lock').write_text('123')
        with patch.object(devin, 'lock_owner', return_value=123), patch.object(devin, 'log_idle', return_value=True):
            with self.assertRaises(devin.Refused):
                devin.owner_ready(self.db, 'test-session')

    def test_prompt_completion_log_must_follow_the_latest_prompt(self):
        p = self.data / 'logs/devin_example_123.log'
        end = 'INFO acp_bridge_dispatch{method="session/prompt" queue_wait_ms=0}: chisel_agent::acp_server::manager: close time.busy=1ms\n'
        p.write_text(end)
        self.assertTrue(devin.log_idle(123))
        p.write_text(end + 'INFO acp_bridge_dispatch{method="session/prompt" queue_wait_ms=0}:prompt:session_db_call: close time.busy=1ms\n')
        self.assertFalse(devin.log_idle(123))

    def test_retry_never_injects_the_same_message_again(self):
        devin.ROOT.mkdir()
        text = 'hello'
        devin.write_record('test-session', {'id': 'test-session', 'tmux': 'visible-test', 'pane': '%8', 'receipts': {'request-1': {'status': 'sent', 'digest': devin.hashlib.sha256(text.encode()).hexdigest()}}})
        with patch.object(devin, 'tmux') as tmux:
            r = devin.send_message({'id': 'test-session', 'text': text, 'requestId': 'request-1'})
            self.assertEqual(r['status'], 'sent')
            tmux.assert_not_called()
            with self.assertRaises(devin.Refused):
                devin.send_message({'id': 'test-session', 'text': 'different', 'requestId': 'request-1'})

    def test_existing_cli_detection_uses_pid_ancestry_and_not_names(self):
        from subprocess import CompletedProcess
        panes = CompletedProcess([], 0, b'manual-session\t%8\t100\nother\t%9\t200\ninyon-view-demo\t%8\t100\n', b'')
        tree = CompletedProcess([], 0, '100 1\n110 100\n120 110\n200 1\n', '')
        with patch.object(devin, 'tmux', return_value=panes), patch.object(devin.subprocess, 'run', return_value=tree):
            self.assertEqual(devin.owner_pane(120), {'tmux': 'manual-session', 'pane': '%8', 'panePid': 100})
            self.assertIsNone(devin.owner_pane(999))


if __name__ == '__main__':
    unittest.main()
