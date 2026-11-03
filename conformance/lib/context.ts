import crypto from 'node:crypto'
import http from 'node:http'
import net from 'node:net'
import { fold, foldActions, type Folded, type Msg } from './record.ts'
import type { Hub } from './hub.ts'
import type { Machine } from './machine.ts'
import { FakeNode } from './node.ts'
import type { Normalizer } from './normalize.ts'

/** Response headers worth comparing; Date, Content-Length and the like are left out. */
const HEADERS = [
  'content-type', 'set-cookie', 'cache-control', 'content-disposition', 'vary', 'access-control-allow-origin',
  'access-control-allow-headers', 'access-control-allow-methods', 'access-control-max-age', 'content-security-policy',
  'x-content-type-options', 'x-dns-prefetch-control', 'referrer-policy', 'location', 'etag', 'allow',
]
/** bodies longer than this are recorded by size and checksum */
const MAX_TEXT = 4096

export type As = 'browser' | 'anon' | { cookie: string } | { bearer: string }

export interface RestOptions {
  as?: As
  /** the Origin header; the browser's own by default when signed in as the browser */
  origin?: string | null
  headers?: Record<string, string>
  /** a raw body (not JSON-encoded) */
  raw?: string
  /** leave the call out of the transcript (setup the scenario isn't about) */
  quiet?: boolean
  /** a file download: the body is written down as its size and checksum */
  binary?: boolean
}

export interface RestResult {
  status: number
  headers: Headers
  body: any
  text: string
}

export interface WaitOptions {
  /** how long to wait for it (default 15 s) */
  ms?: number
}

/** One open /ws connection, as the app holds it. */
/** What a request sent, with long strings (padding, big pastes) cut to their length, so a golden line stays readable. */
function shorten(v: unknown): unknown {
  if (typeof v === 'string') return v.length > 512 ? `<${v.length} characters>` : v
  if (Array.isArray(v)) return v.map(shorten)
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, shorten(x)]))
  return v
}

export class AppSocket {
  ws!: WebSocket
  buffer: Msg[] = []
  all: Msg[] = []
  /** the last snapshot written per stream, normalized (see Context.dropRepeats) */
  lastSnapshot = new Map<string, string>()
  closed?: { code: number; reason: string }
  closeWritten = false
  private waiters = new Set<() => void>()

  constructor(
    readonly name: string,
    private ctx: Context,
  ) {}

  open(as: As = 'browser', query = '', extra: Record<string, string> = {}): Promise<string> {
    const headers: Record<string, string> = { ...extra }
    const cookie = this.ctx.cookieFor(as)
    if (cookie) headers.cookie = cookie
    if (typeof as === 'object' && 'bearer' in as) headers.authorization = `Bearer ${as.bearer}`
    if (as === 'browser') headers.origin = this.ctx.hub.base
    this.ws = new WebSocket(`${this.ctx.hub.base.replace(/^http/, 'ws')}/ws${query}`, { headers } as any)
    return new Promise((resolve) => {
      this.ws.onopen = () => resolve('open')
      this.ws.onmessage = (e) => {
        const m = JSON.parse(String(e.data)) as Msg
        this.buffer.push(m)
        this.all.push(m)
        this.ctx.activityAt = Date.now()
        for (const w of this.waiters) w()
      }
      this.ws.onclose = (e) => {
        this.closed = { code: e.code, reason: e.reason }
        this.ctx.activityAt = Date.now()
        resolve(`closed ${e.code}`)
        for (const w of this.waiters) w()
      }
      this.ws.onerror = () => {}
    })
  }

  send(m: Record<string, unknown>) {
    this.ctx.entry({ send: this.name, msg: m })
    this.ws.send(JSON.stringify(m))
  }

  /**
   * Waits for a message matching `pred` among those not yet written to the transcript. A timeout
   * is written down rather than thrown, so a hub that never sends it shows up as a diff.
   */
  async until(label: string, pred: (m: Msg) => boolean, o: WaitOptions = {}) {
    const deadline = Date.now() + (o.ms ?? 15_000)
    while (Date.now() < deadline) {
      const hit = this.buffer.find(pred)
      if (hit) return hit
      if (this.closed) break
      await new Promise<void>((r) => {
        const done = () => (this.waiters.delete(done), r())
        this.waiters.add(done)
        setTimeout(done, Math.max(1, deadline - Date.now()))
      })
    }
    this.ctx.entry({ timeout: `${this.name}: ${label}` })
    return undefined
  }

  /** Waits until the socket is closed by the hub. */
  async untilClosed(ms = 5000) {
    const deadline = Date.now() + ms
    while (!this.closed && Date.now() < deadline) await Bun.sleep(20)
    return this.closed
  }

  close() {
    this.ws?.close()
  }
}

/**
 * What a scenario works with: the hub, its fake machine, REST and WebSocket helpers that write
 * everything they see to the transcript, and steps that end once the hub has gone quiet.
 */
export class Context {
  readonly entries: unknown[] = []
  readonly sockets: AppSocket[] = []
  /** the browser's session cookie (name=value) */
  cookie = ''
  activityAt = 0
  private inStep = false

  constructor(
    readonly hub: Hub,
    readonly norm: Normalizer,
    readonly machine?: Machine,
    readonly node?: FakeNode,
    /** the machine's pairing token, already known to the hub */
    readonly nodeToken = '',
  ) {}

  /** The fake machine connects to /node with its token. */
  async connectNode() {
    if (!this.node) throw new Error('this scenario has no machine')
    const r = await this.node.connect(this.hub.base, this.nodeToken)
    this.entry({ node: 'connect', result: r })
    return r
  }

  /** more machines than the scenario's own, all on the same fake machine, closed at the end */
  readonly otherNodes: FakeNode[] = []

  /** Another machine connects to /node with `token` (one just paired, say); it shares the fake machine's files. */
  async connectOtherNode(name: string, token: string, hello: Record<string, unknown>) {
    if (!this.machine) throw new Error('this scenario has no machine')
    const node = new FakeNode(this.machine, hello)
    this.otherNodes.push(node)
    const r = await node.connect(this.hub.base, token)
    this.entry({ node: name, connect: r })
    return node
  }

  /** Waits for the hub to close a machine's connection and writes down how. */
  async nodeClosed(name: string, node: FakeNode, ms = 5000) {
    const closed = await Promise.race([node.closed, Bun.sleep(ms).then(() => undefined)])
    this.entry(closed ? { node: name, closed } : { timeout: `${name}: connection still open` })
  }

  /** Adds a line to the transcript (normalized when the transcript is written). */
  entry(e: unknown) {
    this.entries.push(e)
  }

  cookieFor(as: As) {
    if (as === 'browser') return this.cookie
    if (typeof as === 'object' && 'cookie' in as) return as.cookie
    return ''
  }

  /** Starts a step: the previous one first settles and writes what the hub pushed during it. */
  async step(name: string) {
    if (this.inStep) await this.settle()
    this.inStep = true
    this.entry({ step: name })
  }

  /**
   * Waits until nothing new has arrived for `quiet` ms (at most `max`), then writes the step's
   * pushed messages and machine actions, folded (see record.ts).
   */
  async settle(quiet = 900, max = 12_000) {
    const start = Date.now()
    while (Date.now() - start < max) {
      const last = Math.max(this.activityAt, this.machine?.activityAt ?? 0, start)
      if (Date.now() - last >= quiet) break
      await Bun.sleep(50)
    }
    for (const s of this.sockets) {
      if (s.buffer.length) {
        const got = this.dropRepeats(s, fold(s.buffer))
        if (Object.keys(got).length) this.entry({ ws: s.name, got })
      }
      s.buffer = []
      // the hub closing a socket is part of the answer (a removed phone, a removed machine)
      if (s.closed && !s.closeWritten) {
        s.closeWritten = true
        this.entry({ ws: s.name, closed: s.closed })
      }
    }
    if (this.machine?.actions.length) {
      this.entry({ machine: foldActions(this.machine.actions.splice(0)) })
    }
  }

  /**
   * A snapshot identical (after normalization) to the last one written for the same socket and
   * stream tells the app nothing new. The hub sends a new one when a value changes that the
   * normalizer hides (a clock reading, say), so whether a step catches one depends on timing:
   * those are left out.
   */
  private dropRepeats(s: AppSocket, got: Folded): Folded {
    const same = (path: string, v: unknown) => {
      const k = this.norm.key(v)
      if (s.lastSnapshot.get(path) === k) return true
      s.lastSnapshot.set(path, k)
      return false
    }
    if (got.overview && same('overview', got.overview)) delete got.overview
    for (const [key, chat] of Object.entries(got.chats ?? {})) {
      for (const part of ['info', 'screen', 'live', 'draft', 'progress'] as const) {
        if (part in chat && same(`chat ${key} ${part}`, chat[part])) delete chat[part]
      }
      if (!Object.keys(chat).length) delete got.chats![key]
    }
    if (got.chats && !Object.keys(got.chats).length) delete got.chats
    return got
  }

  async end() {
    if (this.inStep) await this.settle()
    this.inStep = false
    for (const s of this.sockets) s.close()
    for (const n of this.otherNodes) n.close()
  }

  socket(name = 'app') {
    const s = new AppSocket(name, this)
    this.sockets.push(s)
    return s
  }

  /** Waits for something on the fake machine (a draft typed, a screen shown); a timeout is written down. */
  async untilMachine(label: string, pred: () => boolean, ms = 10_000) {
    const deadline = Date.now() + ms
    while (Date.now() < deadline) {
      if (pred()) return true
      await Bun.sleep(25)
    }
    this.entry({ timeout: `machine: ${label}` })
    return false
  }

  /** Opens /ws and records how that went; `headers` go on the upgrade request. */
  async connect(name = 'app', as: As = 'browser', query = '', headers: Record<string, string> = {}) {
    const s = this.socket(name)
    const r = await s.open(as, query, headers)
    this.entry({ ws: name, open: r })
    return s
  }

  /**
   * A WebSocket upgrade the hub should refuse, sent as a raw HTTP request so the refusal's status
   * and text can be recorded (a WebSocket client only sees that it closed). An upgrade the hub
   * accepts is recorded as 101.
   */
  async refusedUpgrade(name: string, path: string, headers: Record<string, string> = {}) {
    const url = new URL(path, this.hub.base)
    const r = await new Promise<{ status: number; body: string }>((resolve, reject) => {
      const req = http.request({
        host: url.hostname,
        port: url.port,
        path: url.pathname + url.search,
        headers: { connection: 'Upgrade', upgrade: 'websocket', 'sec-websocket-version': '13', 'sec-websocket-key': crypto.randomBytes(16).toString('base64'), ...headers },
      })
      req.on('upgrade', (res, socket) => {
        socket.destroy()
        resolve({ status: res.statusCode ?? 0, body: '' })
      })
      req.on('response', (res) => {
        let body = ''
        res.on('data', (d) => (body += d))
        res.on('end', () => resolve({ status: res.statusCode ?? 0, body }))
      })
      req.on('error', reject)
      req.end()
    })
    this.entry({ ws: name, upgrade: path, status: r.status, body: r.body })
    return r
  }

  async rest(method: string, path: string, body?: unknown, o: RestOptions = {}): Promise<RestResult> {
    const as = o.as ?? 'browser'
    const headers: Record<string, string> = { ...(o.headers ?? {}) }
    const cookie = this.cookieFor(as)
    if (cookie) headers.cookie = cookie
    if (typeof as === 'object' && 'bearer' in as) headers.authorization = `Bearer ${as.bearer}`
    const origin = o.origin === undefined ? (as === 'browser' && method !== 'GET' ? this.hub.base : null) : o.origin
    if (origin) headers.origin = origin
    let payload: string | undefined
    if (o.raw !== undefined) payload = o.raw
    else if (body !== undefined) {
      payload = JSON.stringify(body)
      headers['content-type'] ??= 'application/json'
    }
    const res = await fetch(`${this.hub.base}${path}`, { method, headers, body: payload, redirect: 'manual' })
    const bytes = Buffer.from(await res.arrayBuffer())
    const text = o.binary ? '' : bytes.toString('utf8')
    let parsed: any = o.binary ? { bytes: bytes.length, contentSha256: crypto.createHash('sha256').update(bytes).digest('hex') } : text
    if ((res.headers.get('content-type') ?? '').includes('json')) {
      try {
        parsed = JSON.parse(text)
      } catch {}
    }
    this.activityAt = Date.now()
    if (!o.quiet) {
      // shown decoded, so the paths and ids in a query string are normalized like any other text
      let shown = path
      try {
        shown = decodeURIComponent(path)
      } catch {}
      const sent: Record<string, unknown> = { method, path: shown }
      if (body !== undefined) sent.body = shorten(body)
      if (o.raw !== undefined) sent.raw = shorten(o.raw)
      if (as !== 'browser') sent.as = typeof as === 'string' ? as : 'cookie' in as ? 'other cookie' : 'bearer'
      if (o.origin !== undefined) sent.origin = o.origin
      if (o.headers) sent.headers = o.headers
      this.entry({ rest: sent, status: res.status, headers: this.headers(res.headers), body: this.body(parsed, text) })
    }
    return { status: res.status, headers: res.headers, body: parsed, text }
  }

  /**
   * A request written byte for byte on a socket, for targets a URL parser would rewrite before
   * sending (`..`, `%2e%2e`, a backslash) and for a Host header fetch won't send. Recorded like
   * rest(), with `raw` in place of `rest`.
   */
  async raw(method: string, target: string, o: { as?: As; headers?: Record<string, string> } = {}): Promise<RestResult> {
    const as = o.as ?? 'anon'
    const headers: Record<string, string> = { host: new URL(this.hub.base).host, connection: 'close', ...(o.headers ?? {}) }
    const cookie = this.cookieFor(as)
    if (cookie) headers.cookie = cookie
    if (typeof as === 'object' && 'bearer' in as) headers.authorization = `Bearer ${as.bearer}`
    const url = new URL(this.hub.base)
    const bytes = await new Promise<Buffer>((resolve, reject) => {
      const sock = net.connect(Number(url.port), url.hostname, () => {
        sock.write(`${method} ${target} HTTP/1.1\r\n${Object.entries(headers).map(([k, v]) => `${k}: ${v}\r\n`).join('')}\r\n`)
      })
      const chunks: Buffer[] = []
      sock.on('data', (c: Buffer) => chunks.push(c))
      sock.on('end', () => resolve(Buffer.concat(chunks)))
      sock.on('error', reject)
    })
    const split = bytes.indexOf('\r\n\r\n')
    const head = bytes.subarray(0, split).toString('latin1').split('\r\n')
    const status = Number(head[0]?.split(' ')[1])
    const h = new Headers()
    for (const line of head.slice(1)) {
      const i = line.indexOf(':')
      h.append(line.slice(0, i).trim(), line.slice(i + 1).trim())
    }
    let body = bytes.subarray(split + 4)
    if (/chunked/i.test(h.get('transfer-encoding') ?? '')) {
      const out: Buffer[] = []
      for (let at = 0; at < body.length; ) {
        const eol = body.indexOf('\r\n', at)
        const size = parseInt(body.subarray(at, eol).toString(), 16)
        if (!size) break
        out.push(body.subarray(eol + 2, eol + 2 + size))
        at = eol + 2 + size + 2
      }
      body = Buffer.concat(out)
    }
    const text = body.toString('utf8')
    this.activityAt = Date.now()
    const sent: Record<string, unknown> = { method, target }
    if (as !== 'anon') sent.as = typeof as === 'string' ? as : 'cookie' in as ? 'other cookie' : 'bearer'
    if (o.headers) sent.headers = o.headers
    this.entry({ raw: sent, status, headers: this.headers(h), body: this.body(text, text) })
    return { status, headers: h, body: text, text }
  }

  private headers(h: Headers) {
    const out: Record<string, unknown> = {}
    for (const name of HEADERS) {
      if (name === 'set-cookie') {
        const all = h.getSetCookie()
        if (all.length) out[name] = all.map((c) => this.cookieShape(c))
        continue
      }
      const v = h.get(name)
      if (v !== null) out[name] = v
    }
    return out
  }

  /**
   * A Set-Cookie header as its parts: the session's fields (decoded), whether its signature checks
   * out against the hub's secret, and the attributes in order.
   */
  private cookieShape(header: string) {
    const [pair = '', ...attrs] = header.split(/;\s*/)
    const eq = pair.indexOf('=')
    const name = pair.slice(0, eq)
    const value = pair.slice(eq + 1)
    const out: Record<string, unknown> = { name }
    if (value) {
      const [payload = '', sig = ''] = value.split('.')
      try {
        out.session = JSON.parse(Buffer.from(payload, 'base64url').toString())
      } catch {
        out.value = value
      }
      const secret = this.hub.secret()
      if (secret && sig) out.signature = crypto.createHmac('sha256', secret).update(payload).digest('base64url') === sig ? 'valid' : 'invalid'
    } else out.value = ''
    out.attributes = attrs
    return out
  }

  private body(parsed: unknown, text: string) {
    if (typeof parsed !== 'string') return parsed
    if (text.length <= MAX_TEXT) return text
    return { bytes: Buffer.byteLength(text), contentSha256: crypto.createHash('sha256').update(text).digest('hex') }
  }

  /** First-run setup through the API, as the app's welcome screen does, without recording it. */
  async signIn(password = 'correct horse battery') {
    const r = await this.rest('POST', '/api/setup', { password, code: this.hub.setupCode() }, { quiet: true, origin: this.hub.base })
    if (r.status !== 200) throw new Error(`setup failed: ${r.status} ${r.text}`)
    this.takeCookie(r)
  }

  /** Keeps the session cookie a response set, and teaches the normalizer its value. */
  takeCookie(r: RestResult) {
    const c = r.headers.getSetCookie().find((x) => x.startsWith('inyon_session='))
    if (!c) return
    this.cookie = c.split(';')[0]!
    this.norm.learn(this.cookie.slice('inyon_session='.length), 'cookie', true)
  }
}
