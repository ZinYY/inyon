import { afterEach, describe, expect, mock, test } from 'bun:test'
import { askNativeNotifications, installNative, nativeNotificationsOn, nativeNotify, readClipboard } from './native.ts'

const flush = () => new Promise((r) => setTimeout(r, 0))
type Listener = (e: any) => void

/** A fake shell: plugins as plain objects of promise-returning methods, plus their listeners. */
function app(plugins: Record<string, Record<string, (...a: any[]) => any>>) {
  const listeners: Record<string, Listener[]> = {}
  for (const [name, p] of Object.entries(plugins)) {
    p.addListener = (event: string, fn: Listener) => {
      ;(listeners[`${name}.${event}`] ||= []).push(fn)
      return Promise.resolve({ remove() {} })
    }
  }
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: plugins }
  return (key: string, e: unknown) => listeners[key]?.forEach((f) => f(e))
}

const realClipboard = navigator.clipboard

afterEach(() => {
  delete (window as any).Capacitor
  Object.defineProperty(navigator, 'clipboard', { value: realClipboard, configurable: true })
  window.history.replaceState({}, '', '/')
})

describe('notifications', () => {
  test('posted through the Android app, with its kind for the buttons', async () => {
    const postNotification = mock(async (_o: unknown) => ({ ok: true, posted: true }))
    app({ InyonDevice: { postNotification } })
    expect(await nativeNotify('claude is waiting', 'approve?', '/m/box/p/3', 'box:%3', 'waiting')).toBe(true)
    expect(postNotification).toHaveBeenCalledWith({ title: 'claude is waiting', body: 'approve?', href: '/m/box/p/3', tag: 'box:%3', kind: 'waiting' })
  })

  test('false when notifications are off or the app is too old to post them', async () => {
    app({ InyonDevice: { postNotification: async () => ({ posted: false }) } })
    expect(await nativeNotify('t', 'b', '/')).toBe(false)
    app({ InyonDevice: {} })
    expect(await nativeNotify('t', 'b', '/')).toBe(false)
    delete (window as any).Capacitor
    expect(await nativeNotify('t', 'b', '/')).toBe(false)
  })

  test('permission is asked and checked through the Android app', async () => {
    app({ InyonDevice: { requestNotificationPermission: async () => ({ granted: true }), notificationsEnabled: async () => ({ enabled: false }) } })
    expect(await askNativeNotifications()).toBe(true)
    expect(await nativeNotificationsOn()).toBe(false)
    delete (window as any).Capacitor
    expect(await askNativeNotifications()).toBeUndefined()
    expect(await nativeNotificationsOn()).toBeUndefined()
  })
})

describe('installNative', () => {
  // installNative makes the page follow the app's visibility; later tests get the browser's again
  afterEach(() => {
    delete (document as any).visibilityState
    delete (document as any).hidden
  })

  test('a tapped notification opens its link, while running and from a cold start', async () => {
    const emit = app({ InyonDevice: { takeLaunchHref: async () => ({ href: '/m/box/p/1' }) }, App: { getLaunchUrl: async () => undefined } })
    installNative()
    await flush()
    expect(location.pathname).toBe('/m/box/p/1')
    emit('InyonDevice.pushOpened', { href: '/m/box/p/2' })
    expect(location.pathname).toBe('/m/box/p/2')
  })

  test('a pairing link that started the app opens the pairing sheet', async () => {
    app({ InyonDevice: { takeLaunchHref: async () => ({ href: null }) }, App: { getLaunchUrl: async () => ({ url: 'inyon://pair?c=abc' }) } })
    installNative()
    await flush()
    expect(location.pathname + location.search).toBe(`/machines?pair=${encodeURIComponent('inyon://pair?c=abc')}`)
  })

  test("the page follows the app's visibility, which Chromium no longer reports", () => {
    const emit = app({ InyonDevice: {}, App: { getLaunchUrl: async () => undefined } })
    installNative()
    const changes: string[] = []
    const seen = () => changes.push(document.visibilityState)
    document.addEventListener('visibilitychange', seen)
    try {
      expect(document.visibilityState).toBe('visible')
      emit('InyonDevice.appVisibility', { visible: false })
      expect([document.visibilityState, document.hidden]).toEqual(['hidden', true])
      // the same state again is no change
      emit('InyonDevice.appVisibility', { visible: false })
      emit('InyonDevice.appVisibility', {})
      emit('InyonDevice.appVisibility', { visible: true })
      expect([document.visibilityState, document.hidden]).toEqual(['visible', false])
      expect(changes).toEqual(['hidden', 'visible'])
    } finally {
      document.removeEventListener('visibilitychange', seen)
    }
  })

  test('a page that starts while the app is off screen asks, and is hidden', async () => {
    app({ InyonDevice: { appVisibility: async () => ({ visible: false }) }, App: { getLaunchUrl: async () => undefined } })
    installNative()
    await flush()
    expect(document.visibilityState).toBe('hidden')
  })

  test('an event that comes before the answer wins over it', async () => {
    let answer: (r: { visible: boolean }) => void = () => {}
    const emit = app({ InyonDevice: { appVisibility: () => new Promise((r) => (answer = r)) }, App: { getLaunchUrl: async () => undefined } })
    installNative()
    emit('InyonDevice.appVisibility', { visible: false })
    answer({ visible: true })
    await flush()
    expect(document.visibilityState).toBe('hidden')
  })

  test('back closes an open sheet before leaving the screen', async () => {
    const exitApp = mock(() => {})
    const back = mock(() => {})
    const realBack = window.history.back
    window.history.back = back
    const emit = app({ App: { exitApp, getLaunchUrl: async () => undefined } })
    installNative()
    const close = (e: Event) => e.preventDefault()
    window.addEventListener('inyon:back', close)
    emit('App.backButton', { canGoBack: true })
    expect(back).not.toHaveBeenCalled()
    window.removeEventListener('inyon:back', close)
    emit('App.backButton', { canGoBack: false })
    expect(exitApp).toHaveBeenCalledTimes(1)
    window.history.back = realBack
  })
})

describe('readClipboard', () => {
  test('reads natively in the Android app', async () => {
    const readText = mock(async () => 'browser')
    Object.defineProperty(navigator, 'clipboard', { value: { readText }, configurable: true })
    app({ InyonDevice: { readClipboardText: async () => ({ text: 'from android' }) } })
    expect(await readClipboard()).toBe('from android')
    expect(readText).not.toHaveBeenCalled()
  })

  test('uses the browser clipboard elsewhere, or when the native read fails', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: { readText: async () => 'browser' }, configurable: true })
    expect(await readClipboard()).toBe('browser')
    app({ InyonDevice: { readClipboardText: async () => Promise.reject(new Error('focus')) } })
    expect(await readClipboard()).toBe('browser')
  })
})
