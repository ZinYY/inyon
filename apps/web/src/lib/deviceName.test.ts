import { afterEach, expect, test } from 'bun:test'
import { deviceName, modelFromUserAgent, readableName } from './deviceName.ts'

afterEach(() => delete (window as any).Capacitor)

const NOTHING = 'Mozilla/5.0 (Linux; Android 16; A142 Build/BP2A.250605.031; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/141.0.0.0 Mobile Safari/537.36'

test('the model from the user agent leaves the build out', () => {
  expect(modelFromUserAgent(NOTHING)).toBe('A142')
  expect(modelFromUserAgent('Mozilla/5.0 (Linux; Android 14; 22081283G) AppleWebKit/537.36')).toBe('22081283G')
  // Chrome's reduced user agent names no model
  expect(modelFromUserAgent('Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36')).toBeUndefined()
  expect(modelFromUserAgent('Mozilla/5.0 (X11; Linux x86_64)')).toBeUndefined()
})

test('maker and model read as one name', () => {
  expect(readableName('Nothing', 'A142')).toBe('Nothing A142')
  expect(readableName('samsung', 'SM-S918B')).toBe('Samsung SM-S918B')
  expect(readableName('Google', 'Google Pixel 9')).toBe('Google Pixel 9')
  expect(readableName('', 'A142')).toBe('A142')
  expect(readableName('Xiaomi', '')).toBe('Xiaomi')
  expect(readableName()).toBeUndefined()
})

test("the phone app names itself from Android's maker and model", async () => {
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: { InyonDevice: { deviceName: async () => ({ manufacturer: 'Nothing', model: 'A142' }) } },
  }
  expect(await deviceName()).toBe('Nothing A142')
})

test('without the native call, the user agent or the fallback', async () => {
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: { InyonDevice: {} } }
  expect(await deviceName('Browser')).toBe(modelFromUserAgent(navigator.userAgent) ?? 'Browser')
})
