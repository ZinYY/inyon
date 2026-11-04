import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid


class OfficialSdkForkTests(unittest.TestCase):
    def test_bounded_fork_uses_new_ids_keeps_source_and_excludes_retracted_turn(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            cwd = root / 'workspace with spaces'
            cwd.mkdir()
            project = root / 'config/projects' / ''.join(c if c.isascii() and c.isalnum() else '-' for c in str(cwd.resolve()))
            project.mkdir(parents=True)
            session = str(uuid.uuid4())
            ids = [str(uuid.uuid4()) for _ in range(4)]
            rows = []
            for i, text in enumerate(['What is regularization?', 'It discourages overly complex models.', 'Give another example.', 'This turn was retracted.']):
                kind = 'user' if i % 2 == 0 else 'assistant'
                rows.append({'type': kind, 'uuid': ids[i], 'parentUuid': ids[i - 1] if i else None,
                             'sessionId': session, 'cwd': str(cwd), 'timestamp': '2026-11-04T00:00:00Z',
                             'message': {'role': kind, 'content': [{'type': 'text', 'text': text}]}})
            rows.append({'type': 'file-history-snapshot', 'messageId': ids[0], 'snapshot': {'trackedFileBackups': {}}})
            source = project / (session + '.jsonl')
            original = ''.join(json.dumps(r) + '\n' for r in rows).encode()
            source.write_bytes(original)
            data = {'sessionId': session, 'upToMessageId': ids[1], 'dir': str(cwd), 'title': 'Regularization branch', 'transcript': str(source)}
            helper = Path(__file__).with_name('claude_fork.py')
            result = subprocess.run([sys.executable, str(helper), json.dumps(data)], capture_output=True, text=True, encoding='utf-8', timeout=40)
            self.assertEqual(result.returncode, 0, result.stderr)
            fork_id = json.loads(result.stdout)['sessionId']
            self.assertNotEqual(fork_id, session)
            fork = [json.loads(line) for line in (project / (fork_id + '.jsonl')).read_text(encoding='utf-8').splitlines()]
            messages = [r for r in fork if r.get('type') in ('user', 'assistant')]
            self.assertEqual(len(messages), 2)
            self.assertEqual(messages[-1]['parentUuid'], messages[0]['uuid'])
            self.assertTrue(all(r['sessionId'] == fork_id and r['uuid'] not in ids for r in messages))
            self.assertNotIn('file-history-snapshot', [r.get('type') for r in fork])
            self.assertEqual(source.read_bytes(), original)
            self.assertNotIn('This turn was retracted.', json.dumps(fork))


if __name__ == '__main__':
    unittest.main()
