import { Sha256 } from './sha256.ts'

/**
 * Uploads a chunk at a time (hubs from 0.24): start the upload, PUT its bytes in order, then
 * finish with the file's SHA-256. A dropped connection waits and asks the hub how much arrived,
 * then carries on from there, for up to about ten minutes of trying; a retry of the same file
 * later (the attachment's "tap to try again") picks up where it stopped too. The hash is worked
 * out from the chunks as they're read, so the file is read once and never held whole.
 */

export interface Uploaded {
  name: string
  path: string
  size: number
  type: string
}

export interface UploadStatus {
  id: string
  received: number
  chunk: number
}

/** How long to wait before each try while the hub can't be reached, in ms: about ten minutes in all. */
export const RETRY_WAITS = [1000, 2000, 4000, 8000, 15_000, ...Array(18).fill(30_000)]

export interface ChunkedOptions {
  /** `fraction` of the file the machine has; `waiting` while the connection is down */
  onProgress?: (fraction: number, waiting: boolean) => void
  signal?: AbortSignal
  waits?: number[]
  /** waits, so tests can skip them */
  sleep?: (ms: number, signal?: AbortSignal) => Promise<void>
}

/** The hub answered, with an error. */
export class UploadError extends Error {
  constructor(public status: number, message: string, public body: any = {}) {
    super(message)
  }
}

/** The hub has no chunked uploads (before 0.24): the caller sends the file the old way. */
export class NoChunkedUploads extends Error {}

/** What a later try of the same file picks up: the hub's upload, and the hash of what was read. */
interface Resume {
  base: string
  pane?: string
  status?: UploadStatus
  hasher: Sha256
  hashed: number
}
const resumes = new WeakMap<Blob, Resume>()

/** A dropped connection, a timeout or a busy server, as opposed to a refusal that a retry won't change. */
export function retryable(err: unknown) {
  if (err instanceof TypeError) return true
  if (err instanceof DOMException) return err.name === 'TimeoutError' || err.name === 'NetworkError'
  if (err instanceof UploadError) return err.status === 0 || err.status === 408 || err.status === 429 || err.status >= 500
  return typeof err === 'object' && err !== null && 'status' in err && (err as { status: number }).status === 0
}

/** Waits `ms`, sooner if the device comes back online; rejects if `signal` aborts. */
export function pause(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const done = () => {
      clearTimeout(t)
      globalThis.removeEventListener?.('online', online)
      signal?.removeEventListener('abort', aborted)
    }
    const online = () => (done(), setTimeout(resolve, 300))
    const aborted = () => (done(), reject(signal!.reason ?? new DOMException('Aborted', 'AbortError')))
    const t = setTimeout(() => (done(), resolve()), ms)
    globalThis.addEventListener?.('online', online)
    signal?.addEventListener('abort', aborted, { once: true })
  })
}

/** A signal that aborts with `signal` or after `ms`, whichever comes first; `done` lets both go. */
function within(ms: number, signal?: AbortSignal) {
  const c = new AbortController()
  const t = setTimeout(() => c.abort(new DOMException('The hub took too long to answer', 'TimeoutError')), ms)
  const stop = () => (clearTimeout(t), c.abort(signal!.reason))
  signal?.addEventListener('abort', stop, { once: true })
  if (signal?.aborted) stop()
  return { signal: c.signal, done: () => (clearTimeout(t), signal?.removeEventListener('abort', stop)) }
}

/**
 * Sends `file` to `base` (a machine's `/api/hosts/<id>/uploads` URL) with `send` (the app's way to
 * reach that machine's hub). Resolves to the saved file.
 */
export async function uploadChunked(send: (req: Request) => Promise<Response>, base: string, pane: string | undefined, file: File, o: ChunkedOptions = {}): Promise<Uploaded> {
  const waits = o.waits ?? RETRY_WAITS
  const sleep = o.sleep ?? pause
  const size = file.size
  const kept = resumes.get(file)
  const r: Resume = kept && kept.base === base && kept.pane === pane ? kept : { base, pane, hasher: new Sha256(), hashed: 0 }
  resumes.set(file, r)
  const report = (received: number, waiting = false) => o.onProgress?.(size ? Math.min(1, received / size) : waiting ? 0 : 1, waiting)

  const call = async (method: string, url: string, body: BodyInit | undefined, ms: number, json = true, signal: AbortSignal | null = o.signal ?? null) => {
    signal?.throwIfAborted()
    const limit = within(ms, signal ?? undefined)
    try {
      const res = await send(new Request(url, { method, body, headers: body === undefined ? {} : { 'content-type': json ? 'application/json' : 'application/octet-stream' }, credentials: 'same-origin', signal: limit.signal }))
      const data = await res.json().catch(() => ({}))
      if (!res.ok) throw new UploadError(res.status, data.error ?? `Upload failed (${res.status})`, data)
      return data
    } catch (err) {
      if (o.signal?.aborted) throw o.signal.reason ?? err
      throw err
    } finally {
      limit.done()
    }
  }

  /** Runs `fn` until it works, waiting between tries while the hub can't be reached. */
  const persist = async <T>(fn: () => Promise<T>, received: () => number): Promise<T> => {
    for (let i = 0; ; i++) {
      try {
        return await fn()
      } catch (err) {
        if (o.signal?.aborted || !retryable(err) || i >= waits.length) throw err
        report(received(), true)
        await sleep(waits[i], o.signal)
      }
    }
  }

  // the hub may have dropped the upload (restarted, or an hour went by): start the file over, a
  // couple of times at most
  for (let fresh = 0; ; fresh++) {
    try {
      if (!r.status) {
        const start = JSON.stringify({ name: file.name || 'file', size, type: file.type, ...(pane ? { pane } : {}) })
        r.status = await persist(async () => {
          try {
            return await call('POST', `${base}/start`, start, 60_000)
          } catch (err) {
            if (err instanceof UploadError && err.status === 404 && err.message === 'Not found') throw new NoChunkedUploads()
            throw err
          }
        }, () => 0)
      }
      const s = r.status!
      let received = s.received
      report(received)
      while (received < size) {
        const at = received
        const end = Math.min(at + s.chunk, size)
        const bytes = new Uint8Array(await file.slice(at, end).arrayBuffer())
        if (end > r.hashed) {
          r.hasher.update(bytes.subarray(Math.max(0, r.hashed - at)))
          r.hashed = end
        }
        const got = await persist(
          async () => {
            try {
              // a megabyte takes about two minutes at 64 kbit/s
              return (await call('PUT', `${base}/${s.id}/${at}`, bytes, 180_000, false)) as UploadStatus
            } catch (err) {
              // the hub has more (or less) than this chunk follows on from: carry on from there
              if (err instanceof UploadError && err.status === 409 && typeof err.body.received === 'number') return err.body as UploadStatus
              // the answer may be lost rather than the chunk: ask before sending it again
              if (retryable(err)) {
                const now = (await call('GET', `${base}/${s.id}`, undefined, 30_000).catch(() => undefined)) as UploadStatus | undefined
                if (now && now.received !== at) return now
              }
              throw err
            }
          },
          () => received,
        )
        received = s.received = got.received
        report(received)
      }
      // what was sent before a retry that found it all there: read for the hash only
      for (let at = r.hashed; at < size; at += s.chunk) r.hasher.update(new Uint8Array(await file.slice(at, Math.min(at + s.chunk, size)).arrayBuffer()))
      const sha256 = r.hasher.hex()
      // the hasher is spent; a later retry reads the file again
      r.hasher = new Sha256()
      r.hashed = 0
      const done = (await persist(() => call('POST', `${base}/${s.id}/finish`, JSON.stringify({ sha256 }), 180_000), () => size)) as Uploaded
      resumes.delete(file)
      report(size)
      return done
    } catch (err) {
      if (err instanceof UploadError && (err.status === 404 || err.status === 410) && fresh < 2) {
        r.status = undefined
        continue
      }
      if (err instanceof NoChunkedUploads || !retryable(err) || o.signal?.aborted) resumes.delete(file)
      // a cancelled upload leaves nothing on the machine
      if (o.signal?.aborted && r.status) void call('DELETE', `${base}/${r.status.id}`, undefined, 15_000, true, null).catch(() => {})
      throw err
    }
  }
}
