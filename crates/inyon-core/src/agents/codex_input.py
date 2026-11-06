"""Submit literal text through the existing local Codex server, never terminal keys.

The payload arrives on stdin. The official proxy owns socket discovery; it does not
start or resume a server/thread. Receipts contain only IDs and a content digest.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import struct
import subprocess
import sys
import threading
import time


class Refused(Exception):
    pass


class Proxy:
    def __init__(self, executable):
        self.process = subprocess.Popen(
            [executable, 'app-server', 'proxy'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0,
        )
        self.deadline = time.monotonic() + 20
        self.pending = queue.Queue()
        self.buffer = bytearray()
        self.sequence = 0
        threading.Thread(target=self.pump, daemon=True).start()

    def pump(self):
        try:
            while True:
                data = self.process.stdout.read(65536)
                self.pending.put(data)
                if not data:
                    return
        except OSError:
            self.pending.put(b'')

    def take(self, count):
        while len(self.buffer) < count:
            data = self.pending.get(timeout=max(.001, self.deadline - time.monotonic()))
            if not data:
                raise OSError('closed')
            self.buffer.extend(data)
        data = bytes(self.buffer[:count])
        del self.buffer[:count]
        return data

    def open(self):
        key = base64.b64encode(os.urandom(16)).decode()
        self.process.stdin.write(('GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\n'
            'Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: '
            + key + '\r\n\r\n').encode())
        self.process.stdin.flush()
        header = b''
        while not header.endswith(b'\r\n\r\n'):
            if len(header) >= 8192:
                raise OSError('invalid upgrade')
            header += self.take(1)
        accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        fields = {k.lower(): v for k, v in (line.split(b':', 1) for line in header.split(b'\r\n')[1:] if b':' in line)}
        if b' 101 ' not in header.split(b'\r\n')[0] or fields.get(b'sec-websocket-accept', b'').strip() != accept:
            raise OSError('invalid upgrade')
        self.rpc('initialize', {'clientInfo': {'name': 'inyon', 'version': '1'},
            'capabilities': {'experimentalApi': True, 'requestAttestation': False}})
        self.send({'jsonrpc': '2.0', 'method': 'initialized', 'params': {}})

    def send(self, value, opcode=1):
        data = json.dumps(value, ensure_ascii=False).encode() if opcode == 1 else value
        size = len(data)
        header = bytes([128 | opcode, 128 | size]) if size < 126 else (
            bytes([128 | opcode, 254]) + struct.pack('!H', size) if size < 65536 else
            bytes([128 | opcode, 255]) + struct.pack('!Q', size))
        mask = os.urandom(4)
        self.process.stdin.write(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))
        self.process.stdin.flush()

    def receive(self):
        parts = []
        size = 0
        while True:
            a, b = self.take(2)
            n = b & 127
            if n == 126:
                n = struct.unpack('!H', self.take(2))[0]
            elif n == 127:
                n = struct.unpack('!Q', self.take(8))[0]
            size += n
            if size > 16 * 1024 * 1024:
                raise OSError('oversized response')
            mask = self.take(4) if b & 128 else None
            data = self.take(n)
            if mask:
                data = bytes(v ^ mask[i % 4] for i, v in enumerate(data))
            opcode = a & 15
            if opcode == 9:
                self.send(data, 10)
                continue
            if opcode == 8:
                raise OSError('closed')
            if opcode not in (0, 1):
                continue
            parts.append(data)
            if a & 128:
                return json.loads(b''.join(parts))

    def rpc(self, method, params):
        self.sequence += 1
        request = self.sequence
        self.send({'jsonrpc': '2.0', 'id': request, 'method': method, 'params': params})
        while True:
            reply = self.receive()
            if reply.get('id') == request and 'method' not in reply:
                if 'error' in reply:
                    # Server errors can contain user text or paths; never forward them.
                    raise Refused('Codex refused the message. Check the terminal before retrying.')
                return reply.get('result', {})

    def close(self):
        self.process.kill()
        self.process.wait(timeout=3)


def submit(client, payload, receipts):
    thread, send_id, text = payload['thread'], payload['id'], payload['text']
    key = hashlib.sha256((thread + '\0' + send_id).encode()).hexdigest()
    digest = hashlib.sha256(text.encode()).hexdigest()
    receipts.mkdir(parents=True, exist_ok=True)
    receipt = receipts / (key + '.json')
    if receipt.exists():
        previous = json.loads(receipt.read_text())
        if previous.get('digest') != digest:
            raise Refused('This send ID already belongs to different text. Send a new message.')
        if previous.get('state') == 'accepted':
            return
        raise Refused('Codex may already have received this message. Check the conversation before sending it again.')
    info = client.rpc('thread/read', {'threadId': thread, 'includeTurns': False})['thread']
    if info.get('id') != thread or info.get('canAcceptDirectInput') is False:
        raise Refused('This Codex thread cannot accept direct input.')
    params = {'threadId': thread, 'clientUserMessageId': 'inyon-' + send_id,
        'input': [{'type': 'text', 'text': text}]}
    method = 'thread/queue/add'
    if payload.get('steer'):
        if info.get('status', {}).get('type') != 'active' or not payload.get('turn'):
            raise Refused('The active Codex turn is not identified. Queue this message instead of steering it.')
        method = 'turn/steer'
        params['expectedTurnId'] = payload['turn']
    # O_EXCL also prevents two concurrent callers from dispatching the same send.
    fd = os.open(receipt, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as out:
        json.dump({'state': 'pending', 'digest': digest}, out)
        out.flush()
        os.fsync(out.fileno())
    try:
        result = client.rpc(method, params)
    except Refused:
        receipt.unlink()  # Explicit rejection, before acceptance, can be retried.
        raise
    if method == 'thread/queue/add':
        queued = result.get('queuedSubmission', {})
        if not queued.get('id') or queued.get('clientUserMessageId') != params['clientUserMessageId']:
            raise OSError('missing receipt')
    temporary = receipt.with_suffix('.tmp')
    with temporary.open('w') as out:
        json.dump({'state': 'accepted', 'digest': digest}, out)
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, receipt)


def main():
    client = None
    try:
        payload = json.loads(sys.stdin.buffer.read())
        executable = shutil.which('codex')
        if not executable and os.name == 'nt':
            installed = Path(os.environ.get('LOCALAPPDATA', str(Path.home() / 'AppData' / 'Local'))) / 'Programs' / 'OpenAI' / 'Codex' / 'bin' / 'codex.exe'
            executable = str(installed) if installed.is_file() else None
        if not executable:
            raise Refused('Codex is unavailable on this machine.')
        client = Proxy(executable)
        client.open()
        submit(client, payload, Path.home() / '.inyon' / 'codex-input-receipts')
        print('{"ok":true}')
    except Refused as error:
        print(json.dumps({'error': str(error)}))
        return 1
    except Exception:
        print(json.dumps({'error': 'Could not confirm direct delivery to Codex. Check the conversation before retrying; terminal input was not used.'}))
        return 1
    finally:
        if client:
            client.close()
    return 0


if __name__ == '__main__':
    sys.exit(main())
