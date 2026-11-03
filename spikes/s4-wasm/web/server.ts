// The machine's side of the spike: packages/link (TypeScript) answering the Rust/wasm phone over
// a WebSocket, plus the page and its files.
import { accept, identityFromSeed, LinkError, noiseKeyOf, verify, type Wire } from '../../../packages/link/src/index.ts'

const seed = (f: (i: number) => number) => Uint8Array.from({ length: 32 }, (_, i) => f(i))
export const machine = identityFromSeed(seed((i) => i + 100))
export const phoneSeed = seed((i) => i + 1)
const stranger = identityFromSeed(seed(() => 9))

const hex = (b: Uint8Array) => Buffer.from(b).toString('hex')
const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i])
const TYPES: Record<string, string> = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm' }

type Data = { wire?: Wire }

export function startServer(dir = import.meta.dir) {
  return Bun.serve<Data, {}>({
    hostname: '127.0.0.1',
    port: 0,
    async fetch(req, srv) {
      const { pathname } = new URL(req.url)
      if (pathname === '/link') return srv.upgrade(req, { data: {} }) ? undefined : new Response('upgrade failed', { status: 400 })
      if (pathname === '/config.json') return Response.json({ machineId: machine.id, phoneSeed: [...phoneSeed], strangerId: stranger.id })
      const rel = pathname === '/' ? '/index.html' : pathname
      if (rel.includes('..')) return new Response('no', { status: 404 })
      const file = Bun.file(dir + rel)
      if (!(await file.exists())) return new Response('not found', { status: 404 })
      const ext = rel.slice(rel.lastIndexOf('.'))
      return new Response(file, { headers: { 'content-type': TYPES[ext] ?? 'application/octet-stream', 'cache-control': 'no-store' } })
    },
    websocket: {
      open(ws) {
        const wire: Wire = { send: (d) => void ws.send(d), close: (code, reason) => ws.close(code, reason) }
        ws.data.wire = wire
        accept(wire, {
          noise: machine.noise,
          // the caller's Noise key must be the one its ed25519 id stands for
          check: (rs, hello) => {
            if (!same(rs, noiseKeyOf(hello.phone))) throw new LinkError('REFUSED', "That key doesn't belong to that phone")
            return { machine: machine.id, from: 'bun' }
          },
        })
          .then(({ channel, hello }) => {
            channel.onmessage = (m: any) => {
              if (m.t === 'proof') channel.send({ t: 'proof', ok: verify(hello.phone, channel.binding, Uint8Array.from(m.sig)), binding: hex(channel.binding) })
              else if (m.t === 'echo') channel.send(m)
              // multi-byte text, so a 60000-byte piece can end inside a character
              else if (m.t === 'big') channel.send({ t: 'big', data: 'é'.repeat(m.n) })
            }
          })
          .catch((e) => console.error('accept:', e.message))
      },
      message(ws, msg) {
        if (typeof msg !== 'string') ws.data.wire?.onmessage?.(new Uint8Array(msg))
      },
      close(ws, code, reason) {
        ws.data.wire?.onclose?.(code, reason)
      },
    },
  })
}

if (import.meta.main) {
  const s = startServer()
  console.log(`http://127.0.0.1:${s.port}/`)
}
