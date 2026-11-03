import { afterEach, expect, test } from 'bun:test'
import { installKeepAlive, keepAliveWanted, timing } from './keepalive.ts'
import { useStore } from './store.ts'
import { host } from '../test/fixtures.ts'

type Listener = (e: any) => void

function phone(device: Record<string, any> = {}) {
  const listeners: Record<string, Listener> = {}
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: {
      InyonDevice: {
        startKeepAlive: async () => ({ ok: true }),
        addListener: (event: string, fn: Listener) => ((listeners[event] = fn), Promise.resolve({ remove() {} })),
        ...device,
      },
    },
  }
  return listeners
}

afterEach(() => {
  delete (window as any).Capacitor
  localStorage.removeItem('inyon.keepAlive')
  localStorage.removeItem('inyon.batteryAsked')
  localStorage.removeItem('inyon.notifyWhyShown')
  useStore.setState({ toasts: [], hosts: [] })
})

// the launch ask waits a moment in the app; not here
timing.askAfterMs = 0
const settle = () => new Promise((r) => setTimeout(r, 10))

test('stays connected unless turned off, asking for the notification permission first', async () => {
  const calls: string[] = []
  const device = { requestNotificationPermission: async () => void calls.push('permission'), startKeepAlive: async () => void calls.push('start') }
  useStore.setState({ hosts: [host('box')] })
  phone(device)
  expect(keepAliveWanted()).toBe(true)
  installKeepAlive()
  await settle()
  expect(calls).toEqual(['permission', 'start'])

  // turned off: alerts still need the permission, but nothing runs in the background
  localStorage.setItem('inyon.keepAlive', '0')
  calls.length = 0
  phone(device)
  installKeepAlive()
  await settle()
  expect(calls).toEqual(['permission'])
})

test('a first launch asks nothing until a computer is paired, then says why before asking', async () => {
  localStorage.removeItem('inyon.notifyWhyShown')
  const calls: string[] = []
  phone({
    notificationsEnabled: async () => ({ enabled: false }),
    requestNotificationPermission: async () => void calls.push('permission'),
    startKeepAlive: async () => void calls.push('start'),
  })
  installKeepAlive()
  await settle()
  expect(calls).toEqual([])
  expect(useStore.getState().toasts).toEqual([])

  useStore.setState({ hosts: [host('box')] })
  await settle()
  expect(calls).toEqual(['permission', 'start'])
  expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['Alerts when an agent needs you'])

  // the line is said once; a later launch without the permission asks again, quietly
  useStore.setState({ toasts: [] })
  calls.length = 0
  installKeepAlive()
  await settle()
  expect(calls).toEqual(['permission', 'start'])
  expect(useStore.getState().toasts).toEqual([])
  localStorage.removeItem('inyon.notifyWhyShown')
})

test('nothing is asked when notifications are already on', async () => {
  const calls: string[] = []
  useStore.setState({ hosts: [host('box')] })
  phone({ notificationsEnabled: async () => ({ enabled: true }), requestNotificationPermission: async () => void calls.push('permission'), startKeepAlive: async () => void calls.push('start') })
  installKeepAlive()
  await settle()
  expect(calls).toEqual(['start'])
  expect(useStore.getState().toasts).toEqual([])
})

test("asks Android once, when there's a machine, not to put Inyon to sleep", async () => {
  let asked = 0
  let ignoring = false
  phone({ isIgnoringBatteryOptimizations: async () => ({ ignoring }), requestBatteryExemption: async () => (asked++, { ignoring: (ignoring = true) }) })
  installKeepAlive()
  await settle()
  expect(asked).toBe(0)
  useStore.setState({ hosts: [host('box')] })
  await settle()
  expect(asked).toBe(1)
  // never again, even when it's restricted again later
  ignoring = false
  installKeepAlive()
  useStore.setState({ hosts: [host('box'), host('b2')] })
  await settle()
  expect(asked).toBe(1)
})

test("doesn't ask when it's already allowed or Stay connected is off", async () => {
  let asked = 0
  phone({ isIgnoringBatteryOptimizations: async () => ({ ignoring: true }), requestBatteryExemption: async () => (asked++, { ignoring: true }) })
  installKeepAlive()
  useStore.setState({ hosts: [host('box')] })
  await settle()
  localStorage.setItem('inyon.keepAlive', '0')
  phone({ isIgnoringBatteryOptimizations: async () => ({ ignoring: false }), requestBatteryExemption: async () => (asked++, { ignoring: true }) })
  useStore.setState({ hosts: [] })
  installKeepAlive()
  useStore.setState({ hosts: [host('box')] })
  await settle()
  expect(asked).toBe(0)
})

test('Stop on the notification turns the setting off', () => {
  localStorage.setItem('inyon.keepAlive', '1')
  const listeners = phone()
  installKeepAlive()
  listeners.keepAliveStopped({ reason: 'stopped' })
  expect(keepAliveWanted()).toBe(false)
  expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['Background connection off'])
})

test('Android stopping it leaves the setting on, to start again later', () => {
  localStorage.setItem('inyon.keepAlive', '1')
  const listeners = phone()
  installKeepAlive()
  listeners.keepAliveStopped({ reason: 'timeout' })
  expect(keepAliveWanted()).toBe(true)
  expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['Background connection paused'])
})
