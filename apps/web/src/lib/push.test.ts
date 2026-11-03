import { afterEach, expect, test } from 'bun:test'
import { api } from './api.ts'
import { enablePush, rotatePush } from './push.ts'

// A browser with push: its service worker's push manager, which keeps one subscription tied to
// the key it was made with.
function browserWithPush() {
  let sub: { options: { applicationServerKey: ArrayBuffer }; endpoint: string; unsubscribe: () => Promise<boolean>; toJSON: () => unknown } | null = null
  const ended: string[] = []
  let n = 0
  const pushManager = {
    getSubscription: async () => sub,
    subscribe: async (o: { applicationServerKey: Uint8Array }) => {
      const endpoint = `https://push.example.net/${++n}`
      sub = {
        options: { applicationServerKey: o.applicationServerKey.slice().buffer },
        endpoint,
        unsubscribe: async () => (ended.push(endpoint), (sub = null), true),
        toJSON: () => ({ endpoint, keys: { p256dh: 'p', auth: 'a' } }),
      }
      return sub
    },
  }
  Object.defineProperty(window, 'isSecureContext', { value: true, configurable: true })
  Object.defineProperty(navigator, 'serviceWorker', { value: { ready: Promise.resolve({ pushManager }) }, configurable: true })
  ;(window as any).PushManager = class {}
  ;(window as any).Notification = { permission: 'granted' }
  return { ended, key: () => new Uint8Array(sub!.options.applicationServerKey) }
}

afterEach(() => {
  delete (navigator as any).serviceWorker
  delete (window as any).PushManager
  delete (window as any).Notification
})

test('once a computer is removed, the page signs with a new key and hands the computers left a new subscription', async () => {
  const b = browserWithPush()
  const first = await enablePush(true)
  expect(first.endpoint).toBe('https://push.example.net/1')
  const before = b.key()
  const handed = (api.push as any).mock.calls.at(-1)[0]
  // the same key the next time the page starts
  expect((await enablePush(true)).endpoint).toBe(first.endpoint)

  const after = await rotatePush(true)
  expect(after.endpoint).toBe('https://push.example.net/2')
  expect(b.ended).toEqual(['https://push.example.net/1'])
  expect(b.key()).not.toEqual(before)
  const now = (api.push as any).mock.calls.at(-1)[0]
  expect(now.vapid.privateJwk.d).not.toBe(handed.vapid.privateJwk.d)
  expect(now.subscription.endpoint).toBe('https://push.example.net/2')
})

test('a new subscription the computers missed is handed over again, a few times at most', async () => {
  browserWithPush()
  await enablePush(true)
  const push = api.push as any
  const calls = push.mock.calls.length
  push.mockImplementationOnce(async () => {
    throw new Error('offline')
  })
  const r = await rotatePush(true, [0, 0])
  expect(r.error).toBeUndefined()
  expect(push.mock.calls.length).toBe(calls + 2)
  // still failing after the last try: it stops, and says why
  push.mockImplementation(async () => {
    throw new Error('offline')
  })
  const gave = await rotatePush(true, [0, 0])
  expect(gave.error).toBe('offline')
  expect(push.mock.calls.length).toBe(calls + 5)
})

test('the computers are told whether page notices are wanted', async () => {
  browserWithPush()
  await enablePush(true, false)
  expect((api.push as any).mock.calls.at(-1)[0]).toMatchObject({ done: true, pages: false })
  await enablePush(false)
  expect((api.push as any).mock.calls.at(-1)[0]).toMatchObject({ done: false, pages: true })
})

test('registrations reach the computers one at a time, in the order the switches changed', async () => {
  browserWithPush()
  const push = api.push as any
  let busy = 0
  let most = 0
  const sent: boolean[] = []
  push.mockImplementation(async (b: any) => {
    most = Math.max(most, ++busy)
    await new Promise((ok) => setTimeout(ok, sent.length ? 0 : 20))
    sent.push(b.pages)
    busy--
  })
  await Promise.all([enablePush(true, true), enablePush(true, false)])
  expect(most).toBe(1)
  expect(sent).toEqual([true, false])
})
