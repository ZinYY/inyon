/**
 * A hub with one answer changed, for diff.test.ts: the real hub from this checkout behind a proxy
 * that drops the question mark from "Which endpoint?". Started like any hub under test (PORT,
 * BIND and the rest from the environment); the real one listens on a port of its own.
 */
import { freePort, INYON } from '../lib/hub.ts'

const port = Number(process.env.PORT)
const inner = await freePort()
const hub = Bun.spawn([INYON, 'engine', 'hub'], { env: { ...process.env, PORT: String(inner) }, stdout: 'inherit', stderr: 'inherit' })
process.on('SIGTERM', () => {
  hub.kill()
  process.exit(0)
})

Bun.serve({
  port,
  hostname: '127.0.0.1',
  async fetch(req) {
    const url = new URL(req.url)
    url.port = String(inner)
    const res = await fetch(url, { method: req.method, headers: req.headers, body: req.body, redirect: 'manual' })
    const text = (await res.text()).replace('Which endpoint?', 'Which endpoint')
    const headers = new Headers(res.headers)
    headers.delete('content-length')
    return new Response(text, { status: res.status, headers })
  },
})
