import json
import unittest
from branch import index


def line(uid, parent, **extra):
    return (json.dumps({'type': 'user', 'uuid': uid, 'parentUuid': parent, **extra}, ensure_ascii=False) + '\n').encode()


class BranchTests(unittest.TestCase):
    def test_rewind_retains_ancestors_and_removes_abandoned_turns(self):
        raw = [line('a', None), line('b', 'a', type='assistant'), line('c', 'b'), line('d', 'c', type='assistant'), line('e', 'b')]
        result = index(raw)
        self.assertEqual(set(result['known']), {'a', 'b', 'e'})
        self.assertEqual(set(result['discarded']), {'c', 'd'})
        self.assertEqual(result['size'], sum(map(len, raw)))

    def test_multiple_rewinds_and_sidechains(self):
        raw = [line('a', None), line('b', 'a'), line('c', 'a'), line('side', 'a', isSidechain=True), line('d', 'c'), line('e', 'a')]
        self.assertEqual(set(index(raw)['known']), {'a', 'e'})
        self.assertEqual(set(index(raw)['discarded']), {'b', 'c', 'd'})

    def test_compaction_keeps_the_earlier_history(self):
        raw = [line('a', None), line('b', None, type='system', subtype='compact_boundary'), line('c', 'b')]
        self.assertFalse(index(raw)['discarded'])

    def test_metadata_partial_lines_and_unicode_do_not_change_the_chain(self):
        raw = [line('a', None, message={'content': '你好'}), b'{"type":"custom-title","customTitle":"Coffee"}\n', b'{incomplete']
        self.assertEqual(index(raw)['known'], ['a'])

    def test_confirmed_rewind_applies_before_the_next_turn_is_written(self):
        raw = [line('a', None), line('b', 'a', type='assistant'), line('c', 'b'), line('d', 'c', type='assistant')]
        size = sum(map(len, raw))
        self.assertEqual(set(index(raw, 'c', size)['known']), {'a', 'b'})
        raw.append(b'{"type":"custom-title","customTitle":"Coffee"}\n')
        self.assertEqual(set(index(raw, 'c', size)['known']), {'a', 'b'})
        raw.append(line('e', 'b', message={'content': 'Another example'}))
        self.assertEqual(set(index(raw, 'c', size)['known']), {'a', 'b', 'e'})

    def test_rewind_to_first_prompt_can_show_an_empty_conversation(self):
        raw = [line('a', None), line('b', 'a', type='assistant')]
        self.assertEqual(index(raw, 'a', sum(map(len, raw)))['known'], [])


if __name__ == '__main__':
    unittest.main()
