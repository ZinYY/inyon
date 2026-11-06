import io
import json
from pathlib import Path
import queue
import struct
import tempfile
import unittest
from codex_input import Proxy, Refused, submit


class Client:
    def __init__(self, failure=None, status='idle'):
        self.calls = []
        self.failure = failure
        self.status = status

    def rpc(self, method, params):
        self.calls.append((method, params))
        if method == 'thread/read':
            return {'thread': {'id': params['threadId'], 'status': {'type': self.status}}}
        if self.failure:
            raise self.failure
        return {'queuedSubmission': {'id': 'queued', 'clientUserMessageId': params['clientUserMessageId']}}


class LiteralInputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.receipts = Path(self.temp.name)
        self.payload = {'thread': 'test-thread', 'id': 'send-1', 'text': "first\n\t\t中文 customer's text\n\nlast"}

    def tearDown(self):
        self.temp.cleanup()

    def test_exact_text_and_repeat_after_consumption_does_not_send_again(self):
        client = Client()
        submit(client, self.payload, self.receipts)
        submit(client, self.payload, self.receipts)
        self.assertEqual([m for m, _ in client.calls], ['thread/read', 'thread/queue/add'])
        self.assertEqual(client.calls[-1][1]['input'], [{'type': 'text', 'text': self.payload['text']}])
        self.assertNotIn(self.payload['text'], next(self.receipts.glob('*.json')).read_text())
        with self.assertRaises(Refused):
            submit(client, {**self.payload, 'text': 'changed'}, self.receipts)

    def test_uncertain_delivery_is_never_replayed(self):
        client = Client(OSError('connection lost after acceptance'))
        with self.assertRaises(OSError):
            submit(client, self.payload, self.receipts)
        client.failure = None
        with self.assertRaises(Refused):
            submit(client, self.payload, self.receipts)
        self.assertEqual(len(client.calls), 2)

    def test_explicit_rejection_can_be_retried(self):
        client = Client(Refused('rejected'))
        with self.assertRaises(Refused):
            submit(client, self.payload, self.receipts)
        self.assertFalse(list(self.receipts.iterdir()))
        client.failure = None
        submit(client, self.payload, self.receipts)

    def test_steering_needs_an_expected_active_turn(self):
        client = Client(status='active')
        with self.assertRaises(Refused):
            submit(client, {**self.payload, 'steer': True}, self.receipts)
        self.assertFalse(list(self.receipts.iterdir()))
        submit(client, {**self.payload, 'steer': True, 'turn': 'turn-1'}, self.receipts)
        self.assertEqual(client.calls[-1][0], 'turn/steer')
        self.assertEqual(client.calls[-1][1]['expectedTurnId'], 'turn-1')

    def test_unloaded_steering_is_not_silently_changed_to_queuing(self):
        client = Client(status='notLoaded')
        with self.assertRaises(Refused):
            submit(client, {**self.payload, 'steer': True, 'turn': 'old-turn'}, self.receipts)
        self.assertEqual([m for m, _ in client.calls], ['thread/read'])
        self.assertFalse(list(self.receipts.iterdir()))

    def test_fragmented_rpc_reply_and_ping(self):
        proxy = object.__new__(Proxy)
        proxy.pending, proxy.buffer = queue.Queue(), bytearray()
        proxy.deadline = float('inf')
        sent = []
        proxy.send = lambda data, opcode=1: sent.append((opcode, data))
        raw = json.dumps({'result': "中文\ttext"}, ensure_ascii=False).encode()
        def frame(op, data):
            return bytes([op, len(data)]) + data
        # A ping interleaved with a fragmented UTF-8 response must not lose either half.
        proxy.pending.put(frame(1, raw[:15]) + frame(137, b'ping') + frame(128, raw[15:]))
        self.assertEqual(proxy.receive(), {'result': "中文\ttext"})
        self.assertEqual(sent, [(10, b'ping')])

    def test_large_outgoing_frame_masks_the_exact_utf8_payload(self):
        proxy = object.__new__(Proxy)
        proxy.process = type('Process', (), {'stdin': io.BytesIO()})()
        value = {'text': "中文\t\n" * 20000}
        proxy.send(value)
        data = proxy.process.stdin.getvalue()
        self.assertEqual(data[:2], bytes([129, 255]))
        size = struct.unpack('!Q', data[2:10])[0]
        mask = data[10:14]
        body = bytes(b ^ mask[i % 4] for i, b in enumerate(data[14:]))
        self.assertEqual(len(body), size)
        self.assertEqual(json.loads(body), value)


if __name__ == '__main__':
    unittest.main()
