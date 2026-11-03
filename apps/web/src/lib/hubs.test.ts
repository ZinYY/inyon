import { afterEach, expect, mock, test } from 'bun:test'
import { checkPlain, claimHub, forgetHub, groupsOf, hubHostId, hubOriginOf, HubRevoked, hubTicket, hubToken, listHubs, tellApart, normalizeHubCode, parseHubLink, notPlain, plainHost, plainHttpIp, PlainRefused, revokedAnswer, splitHostId, unreachable } from './hubs.ts'

const realFetch = globalThis.fetch
afterEach(() => {
  for (const h of listHubs()) forgetHub(h.id)
  localStorage.removeItem('inyon.hubs.v1')
  delete (window as any).Capacitor
  globalThis.fetch = realFetch
})

/** The Android app: its secret store, and the name lookup it does natively. */
function androidApp(names: Record<string, string[]> = {}) {
  const secrets = new Map<string, string>()
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: {
      InyonSsh: {
        setSecret: async ({ key, value }: { key: string; value: string }) => void secrets.set(key, value),
        getSecret: async ({ key }: { key: string }) => ({ value: secrets.get(key) ?? null }),
        deleteSecret: async ({ key }: { key: string }) => void secrets.delete(key),
      },
      InyonDevice: { lookUp: async ({ host }: { host: string }) => ({ addresses: names[host] ?? [] }) },
    },
  }
  return secrets
}

/** A hub that answers a claim, counting the requests it got. */
function hubAnswering(body: unknown, status = 200) {
  const f = mock(async (_u: string, _i?: RequestInit) => Response.json(body, { status }))
  globalThis.fetch = f as unknown as typeof fetch
  return f
}
const CLAIMED = { token: 'tok-fixture-1', hub: { id: '3f9c1a7e2b6d4c80', name: 'studio', version: '0.20.2' } }

const link = 'inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio'

test('a hub link from the QR, as the app or the camera gets it', () => {
  expect(parseHubLink(link)).toEqual({
    url: 'https://hub.example',
    code: 'K7Q4MX2T:493812',
    name: 'studio',
  })
  expect(parseHubLink(`https://inyon.tech/#hub?u=http%3A%2F%2Fbox%3A4777&c=K7Q4MX2T%3A493812`)).toEqual({
    url: 'http://box:4777',
    code: 'K7Q4MX2T:493812',
    name: undefined,
  })
  // a machine's pairing link is not a hub's
  expect(parseHubLink('inyon://pair?m=abc&s=secret')).toBeUndefined()
  expect(parseHubLink('inyon://hub?u=ftp%3A%2F%2Fx&c=K7Q4MX2T%3A493812')).toBeUndefined()
})

test('a typed code in any of its spellings', () => {
  expect(normalizeHubCode('k7q4-mx2t 493812')).toBe('K7Q4MX2T:493812')
  expect(normalizeHubCode('K7Q4MX2T:493812')).toBe('K7Q4MX2T:493812')
  expect(normalizeHubCode('K7Q4MX2T 49381')).toBeUndefined()
})

test('a typed address becomes an origin', () => {
  expect(hubOriginOf('box:4777')).toBe('http://box:4777')
  expect(hubOriginOf('hub.example.com')).toBe('https://hub.example.com')
  expect(hubOriginOf('https://hub.example.com/')).toBe('https://hub.example.com')
  expect(hubOriginOf('http://100.97.188.112')).toBe('http://100.97.188.112')
  expect(hubOriginOf('127.0.0.1:4777')).toBe('http://127.0.0.1:4777')
  expect(hubOriginOf('100.97.188.112')).toBe('http://100.97.188.112')
  expect(hubOriginOf('abhash-arm')).toBe('http://abhash-arm')
  expect(hubOriginOf('https://hub.example.com/somewhere')).toBeUndefined()
  expect(hubOriginOf('')).toBeUndefined()
})

test("a hub's machine carries the hub's id, and loses it on the way back", () => {
  localStorage.setItem(
    'inyon.hubs.v1',
    JSON.stringify([
      {
        id: 'h1',
        name: 'studio',
        url: 'https://hub.example',
        token: 't',
        pairedAt: 1,
      },
    ]),
  )
  const id = hubHostId('h1', 'laptop')
  expect(id).toBe('hub:h1/laptop')
  expect(splitHostId(id)).toMatchObject({ hub: { id: 'h1' }, id: 'laptop' })
  expect(splitHostId('laptop')).toEqual({ id: 'laptop' })
  // a hub that was forgotten leaves the id as it is, so nothing is sent anywhere
  forgetHub('h1')
  expect(listHubs()).toEqual([])
  expect(splitHostId(id)).toEqual({ id })
})

test('when nothing comes back from the hub the message says what to check', () => {
  const failed = new TypeError('Failed to fetch')
  // the test page is plain http, like the app's own shell: a plain hub is reachable in principle
  expect(unreachable('http://abhash-arm', failed)).toContain("opens in this phone's browser")
  expect(unreachable('https://hub.example', failed)).toContain("Couldn't reach https://hub.example")
  const slow = Object.assign(new Error('timed out'), { name: 'TimeoutError' })
  expect(unreachable('http://abhash-arm', slow)).toContain("didn't answer in time")
})

test("each connection refreshes the hub's name and version, which were only read at pairing", async () => {
  const hub = { id: 'h1', name: 'abhash-arm', url: 'http://box:4777', token: 't', version: '0.13.3', pairedAt: 1 }
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([hub]))
  const real = globalThis.fetch
  globalThis.fetch = (async () => Response.json({ ticket: 'tk', hub: { id: 'h1', name: 'studio', version: '0.14.1' } })) as unknown as typeof fetch
  try {
    expect(await hubTicket(hub)).toBe('tk')
    expect(listHubs()[0]).toMatchObject({ id: 'h1', name: 'studio', version: '0.14.1' })
    // another hub's answer (a stale address now pointing elsewhere) changes nothing
    globalThis.fetch = (async () => Response.json({ ticket: 'tk', hub: { id: 'other', name: 'x', version: '9.9.9' } })) as unknown as typeof fetch
    await hubTicket(listHubs()[0]!)
    expect(listHubs()[0]).toMatchObject({ name: 'studio', version: '0.14.1' })
  } finally {
    globalThis.fetch = real
  }
})

test('machines that share a name are told apart by their hub', () => {
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h2', name: 'abhash-arm', url: 'http://100.97.188.112:47321', token: 't', pairedAt: 1 }]))
  const m = (id: string, name: string) => ({ config: { id, name } })
  expect(tellApart([m('local', 'abhash-arm'), m(hubHostId('h2', 'local'), 'abhash-arm'), m('db', 'db')]).map((h) => h.config.name)).toEqual(['abhash-arm', 'abhash-arm · 100.97.188.112:47321', 'db'])
  // on a phone both come through hubs, in whichever order they answer: the one paired first keeps its name
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([
    { id: 'h1', name: 'abhash-arm', url: 'https://abhash-arm', token: 't', pairedAt: 1 },
    { id: 'h2', name: 'abhash-arm', url: 'http://100.97.188.112:47321', token: 't', pairedAt: 2 },
  ]))
  const both = [m(hubHostId('h1', 'local'), 'abhash-arm'), m(hubHostId('h2', 'local'), 'abhash-arm')]
  expect(tellApart(both).map((h) => h.config.name)).toEqual(['abhash-arm', 'abhash-arm · 100.97.188.112:47321'])
  expect(tellApart(both.reverse()).map((h) => h.config.name)).toEqual(['abhash-arm · 100.97.188.112:47321', 'abhash-arm'])
  // a name nobody shares stays as it is
  expect(tellApart([m(hubHostId('h2', 'local'), 'abhash-arm')])[0]!.config.name).toBe('abhash-arm')
})

test("a hub link carries the hub's id and certificate, which the app shows to compare", () => {
  const fp = 'ab'.repeat(32)
  expect(parseHubLink(`${link}&h=3F9C1A7E2B6D4C80&f=${fp}`)).toEqual({ url: 'https://hub.example', code: 'K7Q4MX2T:493812', name: 'studio', hubId: '3f9c1a7e2b6d4c80', tlsFingerprint: fp })
  // anything else in their place is left out rather than shown
  expect(parseHubLink(`${link}&h=zz&f=12`)).toEqual({ url: 'https://hub.example', code: 'K7Q4MX2T:493812', name: 'studio' })
  expect(groupsOf('3f9c1a7e2b6d4c80')).toBe('3f9c 1a7e 2b6d 4c80')
  expect(groupsOf(fp)).toBe('abab abab abab abab')
})

test('plain http goes only to this device, Tailscale or NetBird, as the inyon command allows', () => {
  for (const ip of ['127.0.0.1', '127.8.0.1', '100.64.0.1', '100.97.188.112', '100.127.255.255', '::1', '[::1]', 'fd7a:115c:a1e0::1', '[fd7a:115c:a1e0:ab12::5]', '::ffff:100.100.1.1', 'fd7a:115c:a1e0::1%wlan0'])
    expect(plainHttpIp(ip)).toBe(true)
  for (const ip of ['10.0.0.5', '10.0.2.2', '192.168.1.2', '172.16.0.1', '100.63.255.255', '100.128.0.1', '203.0.113.5', '0.0.0.0', 'fd7a:115c:a1e1::1', 'fe80::1', '2001:db8::1', '::ffff:192.168.1.2', 'not an ip', '1.2.3', '300.1.1.1'])
    expect(plainHttpIp(ip)).toBe(false)
  expect(plainHost('localhost')).toBe('ok')
  expect(plainHost('100.97.188.112')).toBe('ok')
  expect(plainHost('[fd7a:115c:a1e0::1]')).toBe('ok')
  expect(plainHost('203.0.113.5')).toBe('refused')
  expect(plainHost('[2001:db8::1]')).toBe('refused')
  expect(plainHost('abhash-arm')).toBe('look-up')
  expect(plainHost('hub.example.com')).toBe('look-up')
})

test('in the app a name is looked up, and every address it has must be allowed', async () => {
  androidApp({ 'abhash-arm': ['100.97.188.112'], 'nas.lan': ['192.168.1.20'], mixed: ['100.97.188.112', '203.0.113.5'], 'hub.example.com': ['203.0.113.5'] })
  await checkPlain('http://abhash-arm')
  await checkPlain('https://hub.example.com')
  await checkPlain('http://127.0.0.1:4777')
  await expect(checkPlain('http://nas.lan:4777')).rejects.toThrow("Inyon won't use plain http with nas.lan (it points to 192.168.1.20)")
  await expect(checkPlain('http://mixed')).rejects.toBeInstanceOf(PlainRefused)
  await expect(checkPlain('http://hub.example.com')).rejects.toBeInstanceOf(PlainRefused)
  await expect(checkPlain('http://203.0.113.5:4777')).rejects.toThrow('Tailscale or NetBird address (100.x.y.z)')
  // a name that doesn't resolve right now is worth trying again, not a refusal
  const err = await checkPlain('http://gone').catch((e) => e)
  expect(err).toBeInstanceOf(Error)
  expect(err).not.toBeInstanceOf(PlainRefused)
})

test("a NetBird name with its own IPv6 passes, and a LAN or public address beside it doesn't", async () => {
  const nb = ['100.97.188.112', 'fd91:8fa2:3e51:2fcb:2587:6cbd:82:4402']
  expect(notPlain(nb)).toBeUndefined()
  expect(notPlain(['fd7a:115c:a1e0::5'])).toBeUndefined()
  expect(notPlain(['100.97.188.112', '192.168.1.5'])).toBe('192.168.1.5')
  expect(notPlain(['100.97.188.112', '2001:db8::1'])).toBe('2001:db8::1')
  // a unique-local address alone could be any LAN's
  expect(notPlain(['fd91:8fa2:3e51:2fcb::1'])).toBe('fd91:8fa2:3e51:2fcb::1')
  expect(notPlain(['fd7a:115c:a1e0::5', 'fd91::1'])).toBe('fd91::1')
  androidApp({ 'abhash-arm': nb, mixed: ['100.97.188.112', 'fd91::1', '192.168.1.5'] })
  await checkPlain('http://abhash-arm')
  await expect(checkPlain('http://mixed')).rejects.toThrow('(it points to 192.168.1.5)')
})

test("a browser can't look names up, so only the names those VPNs give out pass", async () => {
  await checkPlain('http://box:4777')
  await checkPlain('http://abhash-arm.netbird.selfhosted')
  await checkPlain('http://studio.tail1234.ts.net')
  await expect(checkPlain('http://hub.example.com')).rejects.toThrow("A browser can't check where hub.example.com points")
})

test('pairing with a plain-http hub on a public or LAN address sends nothing', async () => {
  androidApp({ 'nas.lan': ['192.168.1.20'] })
  const f = hubAnswering(CLAIMED)
  await expect(claimHub('http://203.0.113.5:4777', 'K7Q4MX2T:493812', 'Pixel')).rejects.toBeInstanceOf(PlainRefused)
  await expect(claimHub('http://nas.lan:4777', 'K7Q4MX2T:493812', 'Pixel')).rejects.toBeInstanceOf(PlainRefused)
  expect(f).not.toHaveBeenCalled()
  expect(listHubs()).toEqual([])
})

test("the hub's token goes to the phone's secret store, never to localStorage", async () => {
  const secrets = androidApp({ 'abhash-arm': ['100.97.188.112'] })
  hubAnswering(CLAIMED)
  const hub = await claimHub('http://abhash-arm', 'K7Q4MX2T:493812', 'Pixel', { hubId: '3f9c1a7e2b6d4c80' })
  expect(hub).toMatchObject({ id: '3f9c1a7e2b6d4c80', name: 'studio', url: 'http://abhash-arm' })
  for (let i = 0; i < localStorage.length; i++) expect(localStorage.getItem(localStorage.key(i)!)).not.toContain('tok-fixture-1')
  expect(secrets.get(`hub-token:${hub.id}`)).toBe('tok-fixture-1')
  expect(await hubToken(hub.id)).toBe('tok-fixture-1')
  forgetHub(hub.id)
  await Promise.resolve()
  expect(secrets.size).toBe(0)
})

test('a token an older version kept in localStorage moves to the secret store', async () => {
  const secrets = androidApp()
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'old', name: 'studio', url: 'https://hub.example', token: 'kept-before', pairedAt: 1 }]))
  expect(await hubToken('old')).toBe('kept-before')
  expect(secrets.get('hub-token:old')).toBe('kept-before')
  expect(localStorage.getItem('inyon.hubs.v1')).not.toContain('kept-before')
  expect(listHubs()).toEqual([{ id: 'old', name: 'studio', url: 'https://hub.example', pairedAt: 1 }])
})

test("a hub answering with another id than the code's is not kept", async () => {
  androidApp()
  hubAnswering(CLAIMED)
  await expect(claimHub('https://hub.example', 'K7Q4MX2T:493812', 'Pixel', { hubId: '0000000000000000' })).rejects.toThrow("isn't the one in the code")
  expect(listHubs()).toEqual([])
})

test("only the hub's own answer forgets it: a portal's or a proxy's 401 or 403 doesn't", async () => {
  expect(revokedAnswer(401, { error: 'Not signed in', reason: 'revoked' })).toBe(true)
  expect(revokedAnswer(403, { error: 'x', reason: 'revoked' })).toBe(true)
  // hubs before 0.20.2 said only this
  expect(revokedAnswer(401, { error: 'Not signed in' })).toBe(true)
  expect(revokedAnswer(403, { error: 'Only a paired phone connects with a ticket.' })).toBe(false)
  expect(revokedAnswer(401, undefined)).toBe(false)
  expect(revokedAnswer(403, { error: 'Not signed in' })).toBe(false)
  expect(revokedAnswer(401, { error: 'Not signed in', login: 'https://portal' })).toBe(false)
  expect(revokedAnswer(500, { reason: 'revoked' })).toBe(false)

  const hub = { id: 'h1', name: 'studio', url: 'https://hub.example', pairedAt: 1 }
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ ...hub, token: 't' }]))
  globalThis.fetch = (async () => new Response('<html>Sign in to the Wi-Fi</html>', { status: 403 })) as unknown as typeof fetch
  const err = await hubTicket(hub).catch((e) => e)
  expect(err).not.toBeInstanceOf(HubRevoked)
  globalThis.fetch = (async () => Response.json({ error: 'Not signed in', reason: 'revoked' }, { status: 401 })) as unknown as typeof fetch
  expect(await hubTicket(hub).catch((e) => e)).toBeInstanceOf(HubRevoked)
})
