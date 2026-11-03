import { afterEach, beforeEach, describe, expect, jest, mock, test } from 'bun:test'
import type { ServerMsg } from '@inyon/protocol'
import type { PairedHub } from './hubs.ts'
import type { Tagged } from './socket.ts'
import { realSocketModule } from '../test/fakes.ts'

// The real connection to a paired hub (the rest of the suite gets a fake socket), against a fake
// WebSocket and a fake ticket endpoint.
const { HubConn } = realSocketModule

class FakeWS {
  static all: FakeWS[] = []
  readyState = 0
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onclose: ((e: { code: number }) => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) {
    FakeWS.all.push(this)
  }
  send(d: string) {
    this.sent.push(d)
  }
  close() {
    this.readyState = 3
  }
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  hear(m: ServerMsg) {
    this.onmessage?.({ data: JSON.stringify(m) })
  }
}
;(FakeWS as any).CONNECTING = 0
;(FakeWS as any).OPEN = 1
;(FakeWS as any).CLOSED = 3

const hub: PairedHub = { id: 'h1', name: 'studio', url: 'http://box:4777', pairedAt: 1 }
const realWS = globalThis.WebSocket
const realFetch = globalThis.fetch
let heard: Tagged[] = []
let conn: InstanceType<typeof HubConn>
beforeEach(() => {
  FakeWS.all = []
  heard = []
  localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ ...hub, token: 'tok' }]))
  globalThis.WebSocket = FakeWS as any
  globalThis.fetch = mock(async () => new Response(JSON.stringify({ ticket: 't' }))) as any
  conn = new HubConn(hub, (m) => heard.push(m), () => true, () => [])
})
afterEach(() => {
  conn.stop()
  globalThis.WebSocket = realWS
  globalThis.fetch = realFetch
})

test('connecting again closes the connection it replaces', async () => {
  await conn.connect()
  FakeWS.all[0]!.open()
  // the ping found it quiet, or the phone came back to the app
  await conn.connect()
  expect(FakeWS.all).toHaveLength(2)
  expect(FakeWS.all[0]!.readyState).toBe(3)
  expect(FakeWS.all[1]!.readyState).toBe(0)
})

test('two connects at once open one connection', async () => {
  await Promise.all([conn.connect(), conn.connect()])
  expect(FakeWS.all).toHaveLength(1)
})

test("the hub's machines get its id, and a draft keeps saying where it was typed", async () => {
  await conn.connect()
  const ws = FakeWS.all[0]!
  ws.open()
  ws.hear({ t: 'overview', hosts: [{ config: { id: 'srv', name: 'srv', kind: 'ssh', via: 'laptop' } } as any] })
  ws.hear({ t: 'chat.draft', key: 'local:%3', text: 'half a thought', source: 'terminal', at: 1 })
  ws.hear({ t: 'chat.info', key: 'local:%3', info: { hostId: 'local', paneId: '%3' } as any })
  ws.hear({ t: 'notify', host: 'local', pane: '%3', kind: 'done', title: 'Done', body: '' })
  expect(heard[0]).toMatchObject({ t: 'overview', source: 'hub:h1', hosts: [{ config: { id: 'hub:h1/srv', via: 'hub:h1/laptop' } }] })
  expect(heard[1]).toEqual({ t: 'chat.draft', key: 'hub:h1/local:%3', text: 'half a thought', source: 'terminal', at: 1 })
  expect(heard[2]).toMatchObject({ t: 'chat.info', key: 'hub:h1/local:%3', info: { hostId: 'hub:h1/local', paneId: '%3' } })
  expect(heard[3]).toEqual({ t: 'notify', host: 'hub:h1/local', pane: '%3', kind: 'done', title: 'Done', body: '' })
  // the companion's reply has no chat key: its machine is tagged as a notice's is
  ws.hear({ t: 'companion.reply', id: 'r1', host: 'local', pane: '%9', text: 'All quiet.', at: 1 })
  expect(heard[4]).toEqual({ t: 'companion.reply', id: 'r1', host: 'hub:h1/local', pane: '%9', text: 'All quiet.', at: 1 })
  // a page's board moving on opens that hub's copy of the page
  ws.hear({ t: 'page.notify', host: 'local', slug: 'robot', title: 'Robot', body: '3 of 12 done' })
  expect(heard[5]).toEqual({ t: 'page.notify', host: 'hub:h1/local', slug: 'robot', title: 'Robot', body: '3 of 12 done' })
})

test('a hub out of reach when the app opens lists the machines this device remembers as connecting', async () => {
  // the phone opened off its VPN: the ticket request fails before anything was ever heard
  localStorage.setItem('inyon.overview.v1', JSON.stringify({ at: Date.now(), data: [
    { config: { id: 'hub:h1/srv', name: 'srv', kind: 'ssh' }, status: 'online', sessions: [] },
    { config: { id: 'own', name: 'own', kind: 'link' }, status: 'online', sessions: [] },
  ] }))
  globalThis.fetch = mock(async () => { throw new TypeError('Failed to fetch') }) as any
  const cold = new HubConn(hub, (m) => heard.push(m), () => true, () => [])
  await cold.connect()
  cold.stop()
  localStorage.removeItem('inyon.overview.v1')
  expect(heard).toHaveLength(1)
  expect(heard[0]).toMatchObject({ t: 'overview', source: 'hub:h1', hosts: [{ config: { id: 'hub:h1/srv' }, status: 'connecting', error: 'Connecting to studio…' }] })
})

test('messages for the hub name its machines as the hub does', async () => {
  await conn.connect()
  const ws = FakeWS.all[0]!
  ws.open()
  conn.send({ t: 'draft', host: 'hub:h1/local', pane: '%3', text: 'hi' })
  expect(JSON.parse(ws.sent.at(-1)!)).toEqual({ t: 'draft', host: 'local', pane: '%3', text: 'hi' })
})

test('a hub reached while the app is in the background hears it is quiet and gets no chats', async () => {
  const quiet = new HubConn(hub, (m) => heard.push(m), () => true, () => [{ host: 'hub:h1/local', pane: '%3' }], () => true)
  await quiet.connect()
  FakeWS.all[0]!.open()
  quiet.stop()
  expect(FakeWS.all[0]!.sent.map((s) => JSON.parse(s))).toEqual([{ t: 'sub.overview' }, { t: 'quiet', on: true }, { t: 'sub.chat', host: 'local', pane: '%3' }])
})

test('the connection log hears when a hub connects, drops and why', async () => {
  const { connLog } = await import('./connlog.ts')
  connLog.forget('hub:h1')
  await conn.connect()
  const ws = FakeWS.all[0]!
  ws.open()
  expect(connLog.get('hub:h1')).toMatchObject({ kind: 'hub', name: 'studio', up: true })
  ws.onclose?.({ code: 1006 })
  expect(connLog.lastDrop('hub:h1')).toMatchObject({ cause: 'closed', code: 1006 })
  // the next try can't even get a ticket: one more failed try on the same drop
  globalThis.fetch = mock(async () => { throw new TypeError('Failed to fetch') }) as any
  await conn.connect()
  expect(connLog.get('hub:h1')!.drops).toHaveLength(1)
  expect(connLog.lastDrop('hub:h1')).toMatchObject({ tries: 1, last: 'Failed to fetch' })
  connLog.forget('hub:h1')
})

test('a hub that forgot the phone is logged as signed out', async () => {
  const { connLog, why } = await import('./connlog.ts')
  connLog.forget('hub:h1')
  await conn.connect()
  FakeWS.all[0]!.open()
  FakeWS.all[0]!.onclose?.({ code: 4001 })
  expect(why(connLog.lastDrop('hub:h1')!)).toBe('studio no longer knows this phone')
  connLog.forget('hub:h1')
})

test('a hub forgotten while its ticket was on its way leaves no drop behind', async () => {
  const { connLog } = await import('./connlog.ts')
  connLog.forget('hub:h1')
  let fail: ((e: Error) => void) | undefined
  globalThis.fetch = mock(() => new Promise((_, reject) => (fail = reject))) as any
  const going = conn.connect()
  // the plain-http check and the token come first
  for (let i = 0; i < 100 && !fail; i++) await Promise.resolve()
  conn.stop()
  fail!(new TypeError('Failed to fetch'))
  await going
  expect(connLog.get('hub:h1')).toBeUndefined()
})

test("an old socket's sign-in check doesn't relabel the drop of the one after it", async () => {
  const { Socket } = realSocketModule
  const { connLog } = await import('./connlog.ts')
  connLog.forget('self')
  const answers: ((r: Response) => void)[] = []
  globalThis.fetch = mock(() => new Promise<Response>((r) => answers.push(r))) as any
  const s = new Socket()
  s.start()
  const first = FakeWS.all.find((w) => w.url.includes('/ws?'))!
  first.open()
  first.onclose?.({ code: 1006 })
  // the next socket comes and goes before the first one's check answers
  s.reconnect()
  const second = FakeWS.all.filter((w) => w.url.includes('/ws?')).at(-1)!
  expect(second).not.toBe(first)
  second.open()
  second.onclose?.({ code: 1012 })
  answers[0]!(new Response(JSON.stringify({ authed: false })))
  for (let i = 0; i < 5; i++) await Promise.resolve()
  expect(connLog.lastDrop('self')).toMatchObject({ cause: 'closed', code: 1012 })
  s.stop()
  connLog.forget('self')
})

describe('in the background', () => {
  const { Socket, QUIET_AFTER_MS } = realSocketModule
  let hidden = false
  beforeEach(() => {
    hidden = false
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (hidden ? 'hidden' : 'visible') })
  })
  afterEach(() => {
    delete (document as any).visibilityState
    jest.useRealTimers()
  })
  const hubSocket = async () => {
    // the ticket comes first
    for (let i = 0; i < 100 && !FakeWS.all.some((w) => w.url.includes('box:4777')); i++) await Promise.resolve()
    return FakeWS.all.find((w) => w.url.includes('box:4777'))!
  }
  // coming back also checks the connection with a ping
  const sent = (ws: FakeWS) => ws.sent.map((s) => JSON.parse(s)).filter((m) => m.t !== 'ping')
  const flip = (h: boolean) => {
    hidden = h
    document.dispatchEvent(new Event('visibilitychange'))
  }

  test('the app stops following chats after a while, and follows them again on return', async () => {
    jest.useFakeTimers()
    const s = new Socket()
    s.start()
    const ws = await hubSocket()
    ws.open()
    s.subscribeOverview()
    s.subscribeChat('hub:h1/local', '%3')
    ws.sent = []
    flip(true)
    jest.advanceTimersByTime(QUIET_AFTER_MS - 1000)
    // a glance at another app changes nothing
    expect(sent(ws)).toEqual([])
    jest.advanceTimersByTime(1000)
    expect(sent(ws)).toEqual([{ t: 'quiet', on: true }, { t: 'unsub.chat', host: 'local', pane: '%3' }])
    // a chat opened meanwhile waits for the return
    s.subscribeChat('hub:h1/local', '%5')
    ws.sent = []
    flip(false)
    expect(sent(ws)).toEqual([{ t: 'quiet', on: false }, { t: 'sub.chat', host: 'local', pane: '%3' }, { t: 'sub.chat', host: 'local', pane: '%5' }])
    s.stop()
  })

  test('back before the wait is over, nothing was sent', async () => {
    jest.useFakeTimers()
    const s = new Socket()
    s.start()
    const ws = await hubSocket()
    ws.open()
    s.subscribeChat('hub:h1/local', '%3')
    ws.sent = []
    flip(true)
    jest.advanceTimersByTime(5000)
    flip(false)
    jest.advanceTimersByTime(QUIET_AFTER_MS)
    expect(sent(ws)).toEqual([])
    s.stop()
  })
})
