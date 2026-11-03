import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'
import { haptic } from './haptics.ts'
import { useStore } from './store.ts'

const flush = () => new Promise((r) => setTimeout(r, 0))
const realVibrate = Object.getOwnPropertyDescriptor(navigator, 'vibrate')
const realUserAgent = navigator.userAgent
const vibrate = mock((_p: number | number[]) => true)

// the shell's plugins as Capacitor exposes them: plain objects of promise-returning methods
const app = (plugins: Record<string, Record<string, (...a: any[]) => any>>) => {
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: plugins }
}
const capacitorHaptics = () => ({
  impact: mock(async (_o: unknown) => {}),
  selectionChanged: mock(async () => {}),
  notification: mock(async (_o: unknown) => {}),
})
const userAgent = (ua: string) => Object.defineProperty(navigator, 'userAgent', { value: ua, configurable: true })

beforeEach(() => {
  vibrate.mockClear()
  Object.defineProperty(navigator, 'vibrate', { value: vibrate, configurable: true })
})

afterEach(() => {
  delete (window as any).Capacitor
  if (realVibrate) Object.defineProperty(navigator, 'vibrate', realVibrate)
  else delete (navigator as any).vibrate
  userAgent(realUserAgent)
})

describe('haptic', () => {
  test('the Android app plays it natively', async () => {
    const device = { haptic: mock(async (_o: unknown) => ({ played: true })) }
    const haptics = capacitorHaptics()
    app({ InyonDevice: device, Haptics: haptics })
    haptic('event')
    await flush()
    expect(device.haptic).toHaveBeenCalledWith({ kind: 'event' })
    expect(haptics.notification).not.toHaveBeenCalled()
    expect(vibrate).not.toHaveBeenCalled()
  })

  test('falls back to Capacitor Haptics when the native call is refused', async () => {
    const device = {
      haptic: mock(async () => {
        throw new Error('not implemented')
      }),
    }
    const haptics = capacitorHaptics()
    app({ InyonDevice: device, Haptics: haptics })
    haptic('long')
    await flush()
    expect(haptics.impact).toHaveBeenCalledWith({ style: 'HEAVY' })
    expect(vibrate).not.toHaveBeenCalled()
  })

  test('older app builds without the native method use Capacitor Haptics right away', () => {
    const haptics = capacitorHaptics()
    app({ InyonDevice: { scanQr: async () => ({}) }, Haptics: haptics })
    haptic('select')
    expect(haptics.selectionChanged).toHaveBeenCalledTimes(1)
    haptic()
    expect(haptics.impact).toHaveBeenCalledWith({ style: 'LIGHT' })
  })

  test('falls back to the Vibration API when the app has neither', async () => {
    app({ InyonDevice: { haptic: async () => Promise.reject(new Error('nope')) } })
    haptic('error')
    await flush()
    expect(vibrate).toHaveBeenCalledWith([28, 50, 28, 50, 28])
  })

  test('a browser uses the Vibration API', () => {
    haptic('event')
    expect(vibrate).toHaveBeenCalledWith([60, 100, 90])
  })

  test('iOS Safari toggles a hidden switch instead', () => {
    userAgent('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)')
    let clicks = 0
    const count = () => clicks++
    document.addEventListener('click', count, { capture: true })
    haptic('tap')
    document.removeEventListener('click', count, { capture: true })
    expect(vibrate).not.toHaveBeenCalled()
    expect(document.querySelector('input[switch]')).toBeTruthy()
    expect(clicks).toBeGreaterThan(0)
  })

  test('nothing when haptics are off in settings', async () => {
    useStore.setState({ settings: { ...useStore.getState().settings, haptics: false } })
    const device = { haptic: mock(async () => ({})) }
    app({ InyonDevice: device })
    haptic('success')
    await flush()
    expect(device.haptic).not.toHaveBeenCalled()
    expect(vibrate).not.toHaveBeenCalled()
  })
})
