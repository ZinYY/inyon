import { expect, test } from 'bun:test'
import type { HostState } from '@inyon/protocol'
import { PKARR_RELAYS, runChecks, SKIPPED, type CheckDeps, type Sock, type Step, type Target } from './connectCheck.ts'
import { PlainRefused, type PairedHub } from './hubs.ts'

const hub: PairedHub = { id: 'h1', name: 'studio', url: 'https://studio.example', pairedAt: 1 }
const HUB: Target = { kind: 'hub', id: 'hub:h1', name: 'studio', hub }
const SELF: Target = { kind: 'self', id: 'self', name: 'box:4777', origin: 'http://box:4777' }

const timeout = () => Object.assign(new Error('The operation timed out.'), { name: 'TimeoutError' })
const failed = () => new TypeError('Failed to fetch')
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

type Answer = Response | Error | ((init?: RequestInit) => Response | Error)
/** A fake network: the first route whose key the URL contains answers. */
function net(routes: Record<string, Answer>) {
  const asked: { url: string; init?: RequestInit }[] = []
  const fetch = async (url: string, init?: RequestInit) => {
    asked.push({ url, init })
    const key = Object.keys(routes).find((k) => url.includes(k))
    if (!key) throw failed()
    const a = routes[key]!
    const r = typeof a === 'function' ? a(init) : a
    if (r instanceof Error) throw r
    return r.clone()
  }
  return { fetch, asked }
}

/** A WebSocket that opens, closes with a code, or never answers. */
const sock = (how: 'open' | number | 'silent') => (_url: string): Sock => {
  const s: Sock = { onopen: null, onclose: null, onerror: null, close() {} }
  if (how !== 'silent') setTimeout(() => (how === 'open' ? s.onopen?.({}) : s.onclose?.({ code: how, reason: '' })), 1)
  return s
}

/** Everything working, for a paired hub; each test breaks one thing. */
function deps(o: Partial<CheckDeps> & { routes?: Record<string, Answer> } = {}): CheckDeps {
  const { fetch } = net({
    '/api/session': json(200, { authed: true }),
    '/api/phones/ticket': json(200, { ticket: 't1' }),
    ...o.routes,
  })
  return {
    fetch,
    socket: sock('open'),
    online: () => true,
    lookUp: async () => ['100.64.0.7'],
    checkPlain: async () => {},
    token: async () => 'tok',
    wsUrl: () => 'ws://box:4777/ws',
    hubWsUrl: (_h, t) => `wss://studio.example/ws?ticket=${t}`,
    host: () => undefined,
    timeoutMs: 1000,
    handshakeMs: 30,
    sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 5))),
    ...o,
  }
}

async function run(t: Target, d: CheckDeps) {
  const seen: Step[][] = []
  const steps = await runChecks(t, d, (s) => seen.push(s))
  return { steps, seen, by: (id: string) => steps.find((s) => s.id === id)! }
}

test('a hub that works passes every step, one after another', async () => {
  const { steps, seen } = await run(HUB, deps())
  expect(steps.map((s) => [s.id, s.state])).toEqual([
    ['network', 'pass'],
    ['address', 'pass'],
    ['answers', 'pass'],
    ['host', 'pass'],
    ['signed-in', 'pass'],
    ['socket', 'pass'],
  ])
  // each step showed as running, with its sentence, before its answer
  expect(seen.some((s) => s.find((x) => x.id === 'answers')?.state === 'running')).toBe(true)
  expect(seen.flat().filter((x) => x.state === 'running').length).toBe(steps.length)
  expect(steps[0]!.doing).toBe('Checking this device is online…')
})

test('offline: the rest is skipped', async () => {
  const { steps } = await run(HUB, deps({ online: () => false }))
  expect(steps[0]).toMatchObject({ state: 'fail', say: 'This device is offline.' })
  expect(steps.slice(1).every((s) => s.state === 'skip' && s.say === SKIPPED)).toBe(true)
})

test('a name that points nowhere, or only on a VPN that is off', async () => {
  const vpn = await run({ ...HUB, hub: { ...hub, url: 'http://homebox:4777' } }, deps({ lookUp: async () => [] }))
  expect(vpn.by('address')).toMatchObject({ state: 'fail', say: "homebox doesn't point to any address from here." })
  expect(vpn.by('address').fix).toContain('Tailscale or NetBird')
  const dns = await run(HUB, deps({ lookUp: async () => [] }))
  expect(dns.by('address').fix).toContain('spelt as the hub shows it')
})

test('plain http to a LAN address is refused at the address step', async () => {
  const { by } = await run({ ...HUB, hub: { ...hub, url: 'http://192.168.1.5:4777' } }, deps({ checkPlain: async () => { throw new PlainRefused("Inyon won't use plain http with 192.168.1.5") } }))
  expect(by('address')).toMatchObject({ state: 'fail', say: "Inyon won't use plain http with 192.168.1.5" })
  expect(by('answers').state).toBe('skip')
})

test("a browser can't look names up, and says so instead of failing", async () => {
  const { by } = await run(SELF, deps({ lookUp: undefined }))
  expect(by('address').state).toBe('skip')
  expect(by('answers').state).toBe('pass')
})

test("the hub doesn't answer: timed out, or nothing there", async () => {
  const slow = await run(HUB, deps({ routes: { '/api/session': timeout() } }))
  expect(slow.by('answers')).toMatchObject({ state: 'fail', say: "The hub didn't answer within 1 second." })
  expect(slow.by('answers').fix).toContain('inyon service status')
  expect(slow.by('answers').detail).toContain('TimeoutError')
  const none = await run(HUB, deps({ routes: { '/api/session': failed() } }))
  expect(none.by('answers')).toMatchObject({ state: 'fail', say: "Couldn't connect to studio.example." })
  expect(none.by('answers').fix).toContain('certificate')
  expect(none.by('host').state).toBe('skip')
})

test('the wrong host: 421 on the hub page, a refused request from the app', async () => {
  const self = await run(SELF, deps({ routes: { '/api/session': json(421, { error: "This hub doesn't answer to that address." }) } }))
  expect(self.by('host')).toMatchObject({ state: 'fail', say: "The hub doesn't answer to box." })
  expect(self.by('host').fix).toContain('INYON_ALLOWED_HOSTS')
  // the app's own request can't read the 421 (it carries no CORS header): answered without cors, refused with it
  const app = await run(HUB, deps({ routes: { '/api/session': (init) => (init?.mode === 'no-cors' ? new Response(null) : failed()) } }))
  expect(app.by('answers').state).toBe('pass')
  expect(app.by('host')).toMatchObject({ state: 'fail', say: 'Something answered at studio.example but turned the app away.' })
  expect(app.by('host').fix).toContain('INYON_ALLOWED_HOSTS')
})

test('signed out of the hub page offers to sign in', async () => {
  const { by } = await run(SELF, deps({ routes: { '/api/session': json(200, { authed: false }) } }))
  expect(by('signed-in')).toMatchObject({ state: 'fail', say: 'You are signed out of this hub.', action: 'sign-in' })
  expect(by('socket').state).toBe('skip')
})

test('a hub that forgot the phone, and a sign-in page in the way', async () => {
  const forgot = await run(HUB, deps({ routes: { '/api/phones/ticket': json(401, { error: 'Not signed in', reason: 'revoked' }) } }))
  expect(forgot.by('signed-in')).toMatchObject({ state: 'fail', say: 'The hub no longer knows this phone.', action: 'pair-hub' })
  const portal = await run(HUB, deps({ routes: { '/api/phones/ticket': json(403, { message: 'Log in to the Wi-Fi' }) } }))
  expect(portal.by('signed-in')).toMatchObject({ state: 'fail', say: 'Something other than the hub answered 403.' })
  expect(portal.by('signed-in').action).toBeUndefined()
  const noKey = await run(HUB, deps({ token: async () => undefined }))
  expect(noKey.by('host').state).toBe('skip')
  expect(noKey.by('signed-in')).toMatchObject({ state: 'fail', action: 'pair-hub' })
})

test("a hub's 421 can be read from the app, and a ticket must come back", async () => {
  // the hub lets any origin read its refusal of a name, without a token or preflight
  const wrong = await run(HUB, deps({ routes: { '/api/session': (init) => (init?.mode === 'no-cors' ? new Response(null) : init?.headers ? failed() : json(421, {})) } }))
  expect(wrong.by('host')).toMatchObject({ state: 'fail', say: "The hub doesn't answer to studio.example.", detail: 'HTTP 421' })
  const empty = await run(HUB, deps({ routes: { '/api/phones/ticket': json(200, {}) } }))
  expect(empty.by('signed-in')).toMatchObject({ state: 'fail', say: 'The hub answered without a ticket for the live connection.' })
  expect(empty.by('socket').state).toBe('skip')
})

test('the live connection: refused, signed out, or silent', async () => {
  const refused = await run(HUB, deps({ socket: sock(1006) }))
  expect(refused.by('socket')).toMatchObject({ state: 'fail', say: 'The live connection was refused (code 1006).' })
  expect(refused.by('socket').fix).toContain('WebSockets')
  const out = await run(SELF, deps({ socket: sock(4001) }))
  expect(out.by('socket')).toMatchObject({ state: 'fail', action: 'sign-in' })
  const silent = await run(HUB, deps({ socket: sock('silent') }))
  expect(silent.by('socket').say).toContain("didn't open")
})

// ---------- a computer paired with the phone ----------

const pc = (o: Partial<HostState> = {}): HostState =>
  ({
    config: { id: 'pc1', name: 'laptop', kind: 'link', machineId: 'yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o', relays: ['https://relay.example'] },
    status: 'connecting',
    sessions: [],
    agents: [],
    ...o,
  }) as unknown as HostState
const PC = (h = pc()): Target => ({ kind: 'computer', id: 'pc1', name: 'laptop', host: h })

test('a computer that comes back during the check passes the handshake', async () => {
  let reconnected = false
  const { steps } = await run(PC(), deps({
    routes: { [PKARR_RELAYS[0]!]: new Response('x', { status: 200 }), [PKARR_RELAYS[1]!]: new Response('', { status: 404 }), 'relay.example': new Response(null) },
    reconnect: () => void (reconnected = true),
    host: () => (reconnected ? pc({ status: 'online' }) : pc()),
  }))
  expect(steps.map((s) => [s.id, s.state])).toEqual([
    ['network', 'pass'],
    ['lookup', 'pass'],
    ['relay', 'pass'],
    ['handshake', 'pass'],
  ])
})

test("a computer that never published, whose relay is down, and doesn't answer", async () => {
  const error = "Connecting to laptop… (Couldn't find the computer online. Is `inyon link` running there? It needs to be on and connected to the internet.)"
  const { by } = await run(PC(), deps({
    routes: { pkarr: new Response('', { status: 404 }), 'iroh.link': new Response('', { status: 404 }), 'relay.example': failed() },
    host: () => pc({ error }),
  }))
  expect(by('lookup')).toMatchObject({ state: 'warn', say: "The computer hasn't published where it is." })
  expect(by('relay')).toMatchObject({ state: 'warn', say: "The relay at relay.example doesn't answer." })
  // the computer's own words, without the "Connecting to…" around them
  expect(by('handshake')).toMatchObject({ state: 'fail', say: "Couldn't find the computer online. Is `inyon link` running there? It needs to be on and connected to the internet." })
  expect(by('handshake').fix).toContain('inyon service status')
})

test('a computer that unpaired the phone offers to pair again', async () => {
  const { by } = await run(PC(pc({ config: { id: 'pc1', name: 'laptop', kind: 'link' } as never })), deps({ host: () => pc({ error: 'This computer is no longer paired with this phone' }) }))
  expect(by('lookup').state).toBe('skip')
  expect(by('relay').state).toBe('skip')
  expect(by('handshake')).toMatchObject({ state: 'fail', action: 'pair-computer' })
})

test('a lookup service the network blocks is only worth a look', async () => {
  const { by } = await run(PC(), deps({ host: () => pc({ status: 'online' }) }))
  expect(by('lookup')).toMatchObject({ state: 'warn', say: "Couldn't reach the lookup service (pkarr)." })
  expect(by('handshake').state).toBe('pass')
})

// ---------- a machine a hub carries, and Android ----------

test("a hub's machine: the hub's checks, then whether the hub reaches it", async () => {
  const node = { config: { id: 'n1', name: 'server', kind: 'node' }, status: 'error', error: 'Machine offline', sessions: [], agents: [] } as unknown as HostState
  const { steps, by } = await run({ kind: 'machine', id: 'n1', name: 'server', host: node, via: SELF }, deps())
  expect(steps.map((s) => s.id)).toEqual(['network', 'address', 'answers', 'host', 'signed-in', 'socket', 'machine'])
  expect(by('machine')).toMatchObject({ state: 'fail', say: "The hub can't reach server: Machine offline" })
  expect(by('machine').fix).toContain('inyon logs --role node')
})

test('Android: battery limits and Stay connected, even when the hub check failed', async () => {
  const off = await run(HUB, deps({ online: () => false, power: async () => ({ ignoringOptimizations: true }), keepAliveWanted: () => false }))
  expect(off.by('battery')).toMatchObject({ state: 'warn', action: 'stay-connected' })
  const limited = await run(HUB, deps({ power: async () => ({ ignoringOptimizations: false }), keepAliveWanted: () => true }))
  expect(limited.by('battery')).toMatchObject({ state: 'warn', action: 'battery' })
  const restricted = await run(HUB, deps({ power: async () => ({ ignoringOptimizations: true, backgroundRestricted: true }), keepAliveWanted: () => true }))
  expect(restricted.by('battery')).toMatchObject({ state: 'warn', action: 'battery-settings' })
  const stopped = await run(HUB, deps({ power: async () => ({ ignoringOptimizations: true, keepAlive: false }), keepAliveWanted: () => true }))
  expect(stopped.by('battery')).toMatchObject({ state: 'warn', action: 'stay-connected', say: 'Stay connected is on, but Android stopped its background connection.' })
  const fine = await run(HUB, deps({ power: async () => ({ ignoringOptimizations: true, keepAlive: true }), keepAliveWanted: () => true }))
  expect(fine.by('battery').state).toBe('pass')
})

test('a stopped run stops where it is', async () => {
  let n = 0
  const steps = await runChecks(HUB, deps(), () => {}, () => ++n > 3)
  expect(steps.some((s) => s.state === 'pending')).toBe(true)
})
