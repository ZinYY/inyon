import json
import unittest
from side_source import Sources, STREAM, DONE, MAX_BYTES


class SourcesTest(unittest.TestCase):
    def test_exact_source_length_recovers_tex_and_unicode_without_replaying(self):
        p = Sources()
        source = 'Distance: \\(d=v \\times t\\).\n\n\\[d=\\frac{5}{4}\\]\nCafé.'
        for _ in range(2):
            for piece in [source[:20], source[20:]]:
                self.assertIsNone(p.apply(STREAM, 'push_delta: '+json.dumps(piece)))
            result = p.apply(DONE, f'ConsolidateAgentMessage: source_len={len(source.encode())}')
            self.assertEqual(result, source if _ == 0 else None)

    def test_missing_chunks_and_unrelated_logs_are_not_sources(self):
        p = Sources()
        p.apply(STREAM, 'push_delta: "tail"')
        self.assertIsNone(p.apply(DONE, 'source_len=99'))
        self.assertIsNone(p.apply('other', 'push_delta: "wrong"'))
        p.apply(STREAM, 'push_delta: "right"')
        self.assertEqual(p.apply(DONE, 'source_len=5'), 'right')

    def test_malformed_or_oversized_stream_does_not_leak_partial_source(self):
        p = Sources()
        p.apply(STREAM, 'push_delta: "prefix"')
        p.apply(STREAM, 'push_delta: invalid')
        self.assertIsNone(p.apply(DONE, 'source_len=6'))
        p.apply(STREAM, 'push_delta: '+json.dumps('x'*(MAX_BYTES+1)))
        self.assertIsNone(p.apply(DONE, f'source_len={MAX_BYTES+1}'))

    def test_rust_unicode_escape(self):
        p = Sources()
        p.apply(STREAM, r'push_delta: "a\u{200b}b"')
        self.assertEqual(p.apply(DONE, 'source_len=5'), 'a\u200bb')


if __name__ == '__main__':
    unittest.main()
