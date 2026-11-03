// The Rust phone core as the web app meets it: the wasm module that apps/web/scripts/phone-core.ts
// writes, started and called the way apps/web/src/lib/phone.ts calls it.
// Run it after that script: bun test crates/inyon-phone/js. With INYON_BIN set to an inyon
// program, it also pairs with that program's `inyon link`.
import { beforeAll, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { PhoneRuntime, PhoneStore, StartPhone } from '../../../apps/web/src/lib/phone-types.ts'

const dir = join(import.meta.dir, '../../../apps/web/src/phone-core')
let startPhone: StartPhone

/** The app's store, in memory: what it was handed, as it was handed. */
function memoryStore(seed: Record<string, unknown> = {}) {
  const data = new Map(Object.entries(seed))
  const writes: [string, unknown][] = []
  const store: PhoneStore = {
    get: <T>(key: string) => data.get(key) as T | undefined,
    set: (key, value) => {
      writes.push([key, value])
      data.set(key, value)
    },
    del: (key) => void data.delete(key),
    keys: (prefix) => [...data.keys()].filter((k) => k.startsWith(prefix)),
  }
  return { store, data, writes }
}

const FINGERPRINT = 'SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG'

/** A stand-in for the native plugin: calls are recorded, every server refuses commands. */
function fakeSsh() {
  const secrets = new Map<string, string>()
  const calls: { method: string; args: any }[] = []
  const events: string[] = []
  const answer = (method: string, value?: unknown) => async (args: any) => (calls.push({ method, args }), value)
  const ssh = {
    probe: answer('probe', { fingerprint: FINGERPRINT, keyType: 'ssh-ed25519' }),
    connect: async (args: any) => {
      calls.push({ method: 'connect', args })
      if (args.host === 'locked') throw Object.assign(new Error('denied'), { code: 'AUTH_FAILED' })
    },
    disconnect: answer('disconnect'),
    exec: answer('exec', { code: 1, stdout: '', stderr: 'not in this test' }),
    setSecret: async ({ key, value }: any) => void secrets.set(key, value),
    getSecret: async ({ key }: any) => ({ value: secrets.get(key) ?? null }),
    deleteSecret: async ({ key }: any) => void secrets.delete(key),
    addListener: (event: string) => (events.push(event), { remove() {} }),
  }
  return { ssh: ssh as never, secrets, calls, events }
}

const { ssh, secrets, calls, events } = fakeSsh()
const home = memoryStore({ hosts: [{ id: 'box', name: 'box', kind: 'ssh', target: 'me@box', hostKey: 'SHA256:x' }] })
let rt: PhoneRuntime

beforeAll(async () => {
  const glue = await import(join(dir, 'inyon_phone.js'))
  glue.initSync({ module: readFileSync(join(dir, 'inyon_phone_bg.wasm')) })
  startPhone = glue.startPhone
  // a start that fails leaves the page free to start it properly
  await expect(startPhone({ ssh } as never)).rejects.toThrow('needs a store')
  rt = await startPhone({ store: home.store, ssh, name: 'Test phone' })
})

const call = async (method: string, path: string, body?: unknown) => {
  const res = await rt.fetch(new Request(`http://localhost${path}`, { method, body: body === undefined ? undefined : JSON.stringify(body), headers: body === undefined ? {} : { 'content-type': 'application/json' } }))
  return { status: res.status, type: res.headers.get('content-type'), body: await res.json().catch(() => null) }
}

test('the phone is its own hub, with nothing to sign in to', async () => {
  expect(await call('GET', '/api/session')).toEqual({ status: 200, type: 'application/json;charset=utf-8', body: { authed: true, phone: true } })
  expect((await call('POST', '/api/logout')).status).toBe(200)
})

test('a path nothing answers is 404, and a known path asked with the wrong method 405, as JSON', async () => {
  expect(await call('GET', '/api/nothing')).toMatchObject({ status: 404, body: { error: 'Not found' } })
  expect(await call('DELETE', '/api/settings')).toMatchObject({ status: 405, body: { error: 'Method not allowed' } })
})

test("the core's records go to the app's store as values, and come back from it", async () => {
  expect((await call('PUT', '/api/settings', { autoResume: true })).status).toBe(200)
  const [, saved] = home.writes.findLast(([k]) => k === 'settings')!
  expect(saved).toMatchObject({ autoResume: true })
  expect((await call('GET', '/api/settings')).body).toMatchObject({ autoResume: true })
})

test("a body the core can't read is the hub's 400", async () => {
  const res = await rt.fetch(new Request('http://localhost/api/settings', { method: 'PUT', body: '{not json', headers: { 'content-type': 'application/json' } }))
  expect(res.status).toBe(400)
  expect(await res.json()).toEqual({ error: 'Invalid JSON' })
})

test('a screen hears the machines as messages, and its timers run on the page', async () => {
  const heard: any[] = []
  const c = rt.connect((msg) => heard.push(msg))
  c.message(JSON.stringify({ t: 'sub.overview' }))
  await Bun.sleep(20)
  const first = heard.find((m) => m.t === 'overview')
  expect(first.hosts.map((h: any) => h.config.name)).toEqual(['box'])
  // the registry polls it on the core's timers; with no way to reach a server yet, it says so
  const deadline = Date.now() + 8000
  while (!heard.some((m) => m.t === 'overview' && m.hosts[0].status === 'error') && Date.now() < deadline) await Bun.sleep(100)
  expect(heard.findLast((m) => m.t === 'overview').hosts[0]).toMatchObject({ status: 'error' })
  c.close()
  const before = heard.length
  rt.resume()
  await Bun.sleep(300)
  expect(heard.length).toBe(before)
  rt.background(true)
  rt.background(false)
}, 15000)

test("the plugin's events are listened to from the start", () => {
  expect(events.sort()).toEqual(['closed', 'ptyData', 'ptyExit', 'streamExit', 'streamLines'])
})

test("the phone's key is one OpenSSH ed25519 line, made once and kept in the keystore", async () => {
  const a = (await call('GET', '/api/phone/key')).body.publicKey
  expect(a).toMatch(/^ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI[\w+/]+=* inyon-phone$/)
  expect((await call('GET', '/api/phone/key')).body.publicKey).toBe(a)
  const saved = JSON.parse(secrets.get('phone-key')!)
  expect(Buffer.from(saved.seed, 'base64')).toHaveLength(32)
})

test('a server is probed, added with its password in the keystore, and dialed through the plugin', async () => {
  expect((await call('POST', '/api/phone/probe', { target: 'me@10.0.0.5', port: 2222 })).body.fingerprint).toBe(FINGERPRINT)
  expect(calls.findLast((c) => c.method === 'probe')!.args).toEqual({ host: '10.0.0.5', port: 2222 })
  const add = await call('POST', '/api/hosts', { name: 'Laptop', target: 'me@10.0.0.5', port: 2222, hostKey: FINGERPRINT, auth: 'password', password: 'hunter22' })
  expect(add.status).toBe(200)
  expect(JSON.parse(secrets.get(`server:${add.body.id}`)!)).toEqual({ password: 'hunter22' })
  const deadline = Date.now() + 5000
  while (!calls.some((c) => c.method === 'exec' && c.args.id === add.body.id) && Date.now() < deadline) await Bun.sleep(20)
  expect(calls.find((c) => c.method === 'connect' && c.args.id === add.body.id)!.args).toEqual({ id: add.body.id, host: '10.0.0.5', port: 2222, username: 'me', hostKey: FINGERPRINT, password: 'hunter22' })
  expect(calls.find((c) => c.method === 'exec' && c.args.id === add.body.id)!.args.command).toStartWith("sh -c '")
  expect((await call('DELETE', `/api/hosts/${add.body.id}`)).status).toBe(200)
  expect(secrets.has(`server:${add.body.id}`)).toBe(false)
})

test("a plugin's error code reaches the screen as what to check", async () => {
  const add = await call('POST', '/api/hosts', { name: 'Locked', target: 'me@locked', hostKey: FINGERPRINT, auth: 'none' })
  const heard: any[] = []
  const c = rt.connect((msg) => heard.push(msg))
  c.message(JSON.stringify({ t: 'sub.overview' }))
  const host = () => heard.findLast((m) => m.t === 'overview')?.hosts.find((h: any) => h.config.id === add.body.id)
  const deadline = Date.now() + 8000
  while (host()?.status !== 'error' && Date.now() < deadline) await Bun.sleep(50)
  expect(host().error).toContain("The server didn't accept the sign-in. Check the user name and that this phone's key is in ~/.ssh/authorized_keys. (denied)")
  c.close()
  await call('DELETE', `/api/hosts/${add.body.id}`)
}, 15000)

const BIN = process.env.INYON_BIN

test.skipIf(!BIN)('pairs with a computer running inyon link, sees it online and removes it', async () => {
  const home = mkdtempSync(join(tmpdir(), 'inyon-phone-wasm-'))
  const port = 4990 + Math.floor(Math.random() * 100)
  // TMUX empty: in tmux, `pair` leaves the link out
  const env = { ...process.env, HOME: home, USERPROFILE: home, TMUX: '' }
  const run = (...args: string[]) => Bun.spawn([BIN!, ...args], { env, stdout: 'pipe', stderr: 'pipe' })
  const service = run('link', '--no-tunnel', '--lan', '--port', String(port))
  const until = async (ok: () => boolean | Promise<boolean>, ms = 15_000) => {
    for (const end = Date.now() + ms; Date.now() < end; await Bun.sleep(100)) if (await ok()) return true
    return false
  }
  try {
    expect(await until(() => fetch(`http://127.0.0.1:${port}/`).then((r) => r.ok, () => false))).toBe(true)
    const link = (await new Response(run('pair').stdout).text()).match(/inyon:\/\/pair\?\S+/)![0]
    const paired = await call('POST', '/api/phone/pair', { link, name: 'Laptop' })
    expect(paired.status).toBe(200)
    const { id, machineId } = paired.body
    expect(machineId).toHaveLength(52)
    expect((await call('GET', '/api/hosts')).body.find((h: any) => h.id === id)).toMatchObject({ kind: 'link', name: 'Laptop', machineId })

    // the core keeps a channel open to it and polls it through that channel
    const heard: any[] = []
    const c = rt.connect((msg) => heard.push(msg))
    c.message(JSON.stringify({ t: 'sub.overview' }))
    const state = () => heard.findLast((m) => m.t === 'overview')?.hosts.find((h: any) => h.config.id === id)
    expect(await until(() => state()?.status === 'online')).toBe(true)
    expect(state().hostname).toBeTruthy()
    c.close()

    // the computer sends this browser's notifications: it's handed the registration, and loses it
    // when notifications are turned off
    const pushes = async () => (await new Response(run('push', 'list').stdout).text()).trim()
    const browser = {
      subscription: { endpoint: 'https://push.example.net/phone', keys: { p256dh: 'p', auth: 'a' } },
      vapid: { privateJwk: { kty: 'EC', crv: 'P-256', d: 'd', x: 'x', y: 'y' }, subject: 'https://inyon.tech' },
      done: false,
    }
    expect((await call('POST', '/api/push', browser)).status).toBe(200)
    expect(await until(async () => /Test phone\s+push\.example\.net \(needs-you only\)/.test(await pushes()))).toBe(true)
    expect((await call('DELETE', '/api/push', { endpoint: browser.subscription.endpoint })).status).toBe(200)
    expect(await until(async () => (await pushes()).startsWith('No browser gets notifications'))).toBe(true)

    // removed while it's connected: it's told to forget the phone at once
    expect((await call('DELETE', `/api/hosts/${id}`)).body).toEqual({ ok: true })
    const phones = async () => (await new Response(run('phones').stdout).text()).trim()
    expect(await until(async () => !(await phones()).includes('Test phone'))).toBe(true)
  } finally {
    service.kill()
    rmSync(home, { recursive: true, force: true })
  }
}, 60_000)

test('a code that is neither a pairing link nor a short code is refused', async () => {
  expect(await call('POST', '/api/phone/pair', { link: 'hello' })).toMatchObject({ status: 400, body: { error: "That isn't an Inyon pairing code. Run `inyon pair` on the computer and scan the code it shows." } })
})

test("a socket that closed lets go of its handlers, and frames sent before it opened go out once it does", async () => {
  const sockets: any[] = []
  class FakeSocket {
    readyState = 0
    binaryType = ''
    sent: number[] = []
    onopen: any = null
    onmessage: any = null
    onclose: any = null
    constructor(public url: string) {
      sockets.push(this)
      setTimeout(() => {
        this.readyState = 1
        this.onopen?.({ target: this })
      }, 5)
    }
    // the computer isn't there after all: the handshake's first message goes, then it closes
    send(data: Uint8Array) {
      this.sent.push(data.length)
      setTimeout(() => {
        this.readyState = 3
        this.onclose?.({ code: 1006, reason: '', target: this })
      }, 5)
    }
    close() {}
  }
  const real = { ws: globalThis.WebSocket, fetch: globalThis.fetch }
  // no pkarr lookup leaves the machine
  Object.assign(globalThis, { WebSocket: FakeSocket, fetch: async () => Promise.reject(new TypeError('offline')) })
  try {
    const link = `inyon://pair?m=${'y'.repeat(52)}&s=0123456789abcdef&n=laptop&u=${encodeURIComponent('ws://10.0.0.5:4890')}`
    expect((await call('POST', '/api/phone/pair', { link })).status).toBe(502)
    expect(sockets.length).toBeGreaterThan(0)
    for (const s of sockets) {
      expect(s.sent.length).toBe(1)
      expect([s.onopen, s.onmessage, s.onclose]).toEqual([null, null, null])
    }
  } finally {
    Object.assign(globalThis, { WebSocket: real.ws, fetch: real.fetch })
  }
}, 30_000)

test('a page runs one core', async () => {
  await expect(startPhone({ store: memoryStore().store, ssh })).rejects.toThrow('already running')
})
