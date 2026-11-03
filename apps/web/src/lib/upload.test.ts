import { describe, expect, test } from 'bun:test'
import { createHash } from 'node:crypto'
import { Sha256 } from './sha256.ts'
import { NoChunkedUploads, pause, RETRY_WAITS, retryable, uploadChunked, UploadError } from './upload.ts'

const BASE = 'http://hub/api/hosts/box/uploads'
const sha = (b: Uint8Array) => createHash('sha256').update(b).digest('hex')
const bytes = (n: number) => Uint8Array.from({ length: n }, (_, i) => (i * 7) ^ (i >> 11))
const file = (b: Uint8Array<ArrayBuffer>, name = 'clip.mp4') => new File([b], name, { type: 'video/mp4' })

interface Upload {
  name: string
  size: number
  data: number[]
  done?: { name: string; path: string; size: number; type: string }
}

/**
 * A hub with chunked uploads, in memory, as crates/inyon-core/src/features/uploads.rs answers:
 * chunks in order (one already there is skipped, one too far ahead gets 409), a finish that checks
 * the hash. `fail` makes a request fail as a dropped connection does, before or after the hub acts.
 */
function fakeHub(chunk = 4) {
  const uploads = new Map<string, Upload>()
  const seen: string[] = []
  const hub = {
    uploads,
    seen,
    chunk,
    fail: [] as { when: (method: string, path: string) => boolean; after?: boolean; status?: number }[],
    forget: () => uploads.clear(),
    async send(req: Request): Promise<Response> {
      const path = new URL(req.url).pathname.replace('/api/hosts/box/uploads', '')
      seen.push(`${req.method} ${path}`)
      const i = hub.fail.findIndex((f) => f.when(req.method, path))
      const f = i >= 0 ? hub.fail.splice(i, 1)[0] : undefined
      if (f && !f.after) {
        if (f.status) return Response.json({ error: 'busy' }, { status: f.status })
        throw new TypeError('Failed to fetch')
      }
      const res = await hub.answer(req, path)
      if (f?.after) throw new TypeError('Failed to fetch')
      return res
    },
    async answer(req: Request, path: string): Promise<Response> {
      const status = (id: string, u: Upload) => ({ id, name: u.name, size: u.size, received: u.data.length, chunk, path: `/home/me/.inyon/uploads/${u.name}`, done: !!u.done })
      if (req.method === 'POST' && path === '/start') {
        const b = await req.json()
        if (b.size > 100) return Response.json({ error: `${b.name} is larger than 100 MB` }, { status: 413 })
        const id = `u${uploads.size + 1}`
        uploads.set(id, { name: b.name, size: b.size, data: [] })
        return Response.json(status(id, uploads.get(id)!))
      }
      const [, id, rest] = path.split('/')
      const u = uploads.get(id!)
      if (!u) return Response.json({ error: 'This upload is no longer on the hub. Send the file again.' }, { status: 404 })
      if (req.method === 'GET') return Response.json(status(id!, u))
      if (req.method === 'DELETE') return uploads.delete(id!), Response.json({ ok: true })
      if (req.method === 'PUT') {
        const at = Number(rest)
        const data = new Uint8Array(await req.arrayBuffer())
        if (at > u.data.length) return Response.json({ ...status(id!, u), error: 'ahead' }, { status: 409 })
        u.data.push(...data.subarray(u.data.length - at))
        return Response.json(status(id!, u))
      }
      if (req.method === 'POST' && rest === 'finish') {
        const { sha256 } = await req.json()
        if (u.data.length !== u.size) return Response.json({ error: 'short' }, { status: 409 })
        if (sha256 !== sha(Uint8Array.from(u.data))) return uploads.delete(id!), Response.json({ error: "The file arrived damaged: its SHA-256 doesn't match. Send it again." }, { status: 422 })
        u.done ??= { name: u.name, path: `/home/me/.inyon/uploads/${u.name}`, size: u.size, type: 'video/mp4' }
        return Response.json(u.done)
      }
      return Response.json({ error: 'Not found' }, { status: 404 })
    },
  }
  return hub
}

const noWait = async () => {}

describe('sha256', () => {
  test('matches the standard digest, however the bytes are split', () => {
    expect(new Sha256().hex()).toBe('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855')
    expect(new Sha256().update(new TextEncoder().encode('abc')).hex()).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')
    for (const n of [55, 56, 63, 64, 65, 1000, 100_003]) {
      const b = bytes(n)
      const h = new Sha256()
      for (let at = 0; at < n; at += 37) h.update(b.subarray(at, at + 37))
      expect(h.hex()).toBe(sha(b))
    }
  })
})

describe('chunked uploads', () => {
  test('a file goes in chunks in order, with progress, and finishes with its hash', async () => {
    const hub = fakeHub()
    const progress: number[] = []
    const b = bytes(10)
    const u = await uploadChunked(hub.send, BASE, '4', file(b), { onProgress: (f) => progress.push(f), sleep: noWait })
    expect(u.path).toBe('/home/me/.inyon/uploads/clip.mp4')
    expect(hub.seen).toEqual(['POST /start', 'PUT /u1/0', 'PUT /u1/4', 'PUT /u1/8', 'POST /u1/finish'])
    expect(progress).toEqual([0, 0.4, 0.8, 1, 1])
    expect(Uint8Array.from(hub.uploads.get('u1')!.data)).toEqual(b)
  })

  test('the start names the file, its size, type and pane, and chunks go as raw bytes', async () => {
    const sent: Request[] = []
    const hub = fakeHub()
    await uploadChunked((r) => (sent.push(r.clone()), hub.send(r)), BASE, '4', file(bytes(3)), { sleep: noWait })
    expect(await sent[0]!.json()).toEqual({ name: 'clip.mp4', size: 3, type: 'video/mp4', pane: '4' })
    expect(sent[1]!.headers.get('content-type')).toBe('application/octet-stream')
    expect(new Uint8Array(await sent[1]!.arrayBuffer())).toEqual(bytes(3))
  })

  test('a dropped connection waits, asks how much arrived and carries on from there', async () => {
    const hub = fakeHub()
    // the chunk at 4 never gets there, twice; then the one at 8 gets there but its answer is lost
    hub.fail.push({ when: (m, p) => m === 'PUT' && p === '/u1/4' }, { when: (m, p) => m === 'PUT' && p === '/u1/4' }, { when: (m, p) => m === 'PUT' && p === '/u1/8', after: true })
    const waits: number[] = []
    const states: [number, boolean][] = []
    const b = bytes(14)
    await uploadChunked(hub.send, BASE, undefined, file(b), { onProgress: (f, w) => states.push([f, w]), sleep: async (ms) => void waits.push(ms) })
    expect(Uint8Array.from(hub.uploads.get('u1')!.data)).toEqual(b)
    // the chunk whose answer was lost isn't sent again: the hub said it had it
    expect(hub.seen.filter((s) => s === 'PUT /u1/8')).toEqual(['PUT /u1/8'])
    expect(hub.seen).toContain('GET /u1')
    expect(waits).toEqual([1000, 2000])
    expect(states).toContainEqual([4 / 14, true])
    expect(states.at(-1)).toEqual([1, false])
  })

  test('a busy hub or a machine out of reach (5xx) is waited out too', async () => {
    const hub = fakeHub()
    hub.fail.push({ when: (m) => m === 'POST', status: 503 }, { when: (m, p) => p === '/u1/finish', status: 502 })
    const u = await uploadChunked(hub.send, BASE, undefined, file(bytes(5)), { sleep: noWait })
    expect(u.size).toBe(5)
  })

  test('it gives up after the waits run out, and a retry of the same file resumes', async () => {
    const hub = fakeHub()
    const b = bytes(12)
    for (let i = 0; i < 3; i++) hub.fail.push({ when: (m, p) => m === 'PUT' && p === '/u1/4' })
    hub.fail.push({ when: (m, p) => m === 'GET' }, { when: (m, p) => m === 'GET' }, { when: (m, p) => m === 'GET' })
    const f = file(b)
    const err = await uploadChunked(hub.send, BASE, undefined, f, { waits: [1, 1], sleep: noWait }).catch((e) => e)
    expect(err).toBeInstanceOf(TypeError)
    expect(hub.uploads.get('u1')!.data.length).toBe(4)
    // tapped to try again: the same upload carries on, nothing is sent twice
    hub.seen.length = 0
    const progress: number[] = []
    await uploadChunked(hub.send, BASE, undefined, f, { onProgress: (p) => progress.push(p), sleep: noWait })
    expect(hub.seen).toEqual(['PUT /u1/4', 'PUT /u1/8', 'POST /u1/finish'])
    expect(progress[0]).toBe(4 / 12)
    expect(Uint8Array.from(hub.uploads.get('u1')!.data)).toEqual(b)
  })

  test('a retry after a failed finish reads the file again for its hash', async () => {
    const hub = fakeHub()
    const b = bytes(9)
    hub.fail.push({ when: (m, p) => p === '/u1/finish' }, { when: (m, p) => p === '/u1/finish' })
    const f = file(b)
    expect(await uploadChunked(hub.send, BASE, undefined, f, { waits: [1], sleep: noWait }).catch((e) => e)).toBeInstanceOf(TypeError)
    hub.seen.length = 0
    expect((await uploadChunked(hub.send, BASE, undefined, f, { sleep: noWait })).size).toBe(9)
    expect(hub.seen).toEqual(['POST /u1/finish'])
  })

  test('the hub forgetting the upload (a restart) starts the file over', async () => {
    const hub = fakeHub()
    let n = 0
    const b = bytes(12)
    await uploadChunked(
      async (r) => {
        if (r.method === 'PUT' && ++n === 3) hub.forget()
        return hub.send(r)
      },
      BASE,
      undefined,
      file(b),
      { sleep: noWait },
    )
    expect(hub.seen).toContain('POST /start')
    expect(hub.seen.filter((s) => s === 'POST /start').length).toBe(2)
    expect(Uint8Array.from(hub.uploads.get('u1')!.data)).toEqual(b)
  })

  test('refusals fail at once with the hub message', async () => {
    const hub = fakeHub()
    const err = await uploadChunked(hub.send, BASE, undefined, file(bytes(200), 'huge.mov'), { sleep: noWait }).catch((e) => e)
    expect([err.status, err.message]).toEqual([413, 'huge.mov is larger than 100 MB'])
    expect(hub.seen).toEqual(['POST /start'])
  })

  test('a hub from before 0.24 says so, for the caller to send it the old way', async () => {
    const err = await uploadChunked(async () => Response.json({ error: 'Not found' }, { status: 404 }), BASE, undefined, file(bytes(3)), { sleep: noWait }).catch((e) => e)
    expect(err).toBeInstanceOf(NoChunkedUploads)
  })

  test('stopping it deletes what arrived', async () => {
    const hub = fakeHub()
    const stop = new AbortController()
    const p = uploadChunked(
      async (r) => {
        const res = await hub.send(r)
        if (r.method === 'PUT') stop.abort()
        return res
      },
      BASE,
      undefined,
      file(bytes(12)),
      { signal: stop.signal, sleep: noWait },
    )
    await expect(p).rejects.toBeDefined()
    await new Promise((r) => setTimeout(r, 10))
    expect(hub.seen.at(-1)).toBe('DELETE /u1')
    expect(hub.uploads.size).toBe(0)
  })

  test('an empty file still makes a file', async () => {
    const hub = fakeHub()
    const u = await uploadChunked(hub.send, BASE, undefined, file(new Uint8Array()), { sleep: noWait })
    expect(u.size).toBe(0)
    expect(hub.seen).toEqual(['POST /start', 'POST /u1/finish'])
  })

  test('what counts as a dropped connection', () => {
    expect(retryable(new TypeError('Failed to fetch'))).toBe(true)
    expect(retryable(new DOMException('x', 'TimeoutError'))).toBe(true)
    expect(retryable(new UploadError(503, 'x'))).toBe(true)
    expect(retryable(new UploadError(429, 'x'))).toBe(true)
    expect(retryable(new UploadError(413, 'x'))).toBe(false)
    expect(retryable(new UploadError(422, 'x'))).toBe(false)
    expect(retryable(new DOMException('x', 'AbortError'))).toBe(false)
    expect(RETRY_WAITS.reduce((a, b) => a + b, 0)).toBeGreaterThan(9 * 60_000)
  })

  test('a wait ends early when the device is back online, and at once when stopped', async () => {
    const t0 = Date.now()
    const p = pause(60_000)
    globalThis.dispatchEvent(new Event('online'))
    await p
    expect(Date.now() - t0).toBeLessThan(5000)
    const stop = new AbortController()
    const q = pause(60_000, stop.signal)
    stop.abort()
    await expect(q).rejects.toBeDefined()
  })
})
