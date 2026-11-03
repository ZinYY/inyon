import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'
import { checkAppUpdate, LATEST_URL, startAppUpdateCheck } from './appUpdate.ts'
import { useStore } from './store.ts'

const realFetch = globalThis.fetch
let appVersion = '0.6.3'
let release: unknown = {}
const fetched = mock((_url: string) => {})

/** latest.json on the website, as its build writes it (the APK also under its stable name) */
const apk = (version: string, name = `inyon-${version}.apk`) => ({
  version,
  tag: `v${version}`,
  date: '2026-10-13',
  files: [
    { name: 'inyon-linux-x64', size: 1, sha256: 'a'.repeat(64) },
    { name: 'inyon.apk', size: 2, sha256: 'b'.repeat(64) },
    { name, size: 2, sha256: 'b'.repeat(64) },
  ],
})

beforeEach(() => {
  appVersion = '0.6.3'
  release = apk('0.6.4')
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: { App: { getInfo: async () => ({ version: appVersion, build: '7' }) } } }
  globalThis.fetch = (async (url: string) => {
    fetched(String(url))
    return new Response(JSON.stringify(release), { status: 200 })
  }) as unknown as typeof fetch
  fetched.mockClear()
})
afterEach(() => {
  delete (window as any).Capacitor
  globalThis.fetch = realFetch
})

describe('the Android app looks for a newer APK', () => {
  test('finds the newer release and its APK on the website', async () => {
    expect(await checkAppUpdate(true)).toEqual({ current: '0.6.3', latest: '0.6.4', url: 'https://inyon.tech/download/inyon-0.6.4.apk' })
    expect(fetched.mock.calls[0][0]).toBe(LATEST_URL)
    expect(LATEST_URL).toBe('https://inyon.tech/download/latest.json')
  })

  test('takes the versioned APK, else another, and the stable name only when it is the only one', async () => {
    release = apk('0.6.4', 'inyon-release.apk')
    expect((await checkAppUpdate(true))?.url).toBe('https://inyon.tech/download/inyon-release.apk')
    release = { version: '0.6.4', files: [{ name: 'inyon.apk' }] }
    expect((await checkAppUpdate(true))?.url).toBe('https://inyon.tech/download/inyon.apk')
  })

  test('asks the website at most once a day unless Settings asks', async () => {
    const t = 1_000_000_000
    await checkAppUpdate(false, t)
    await checkAppUpdate(false, t + 60_000)
    await checkAppUpdate(false, t + 23 * 3600_000)
    expect(fetched).toHaveBeenCalledTimes(1)
    await checkAppUpdate(false, t + 25 * 3600_000)
    expect(fetched).toHaveBeenCalledTimes(2)
    await checkAppUpdate(true, t + 25 * 3600_000 + 1)
    expect(fetched).toHaveBeenCalledTimes(3)
  })

  test('the same or an older version, a prerelease, or a release without an APK is no update', async () => {
    release = apk('0.6.3')
    expect(await checkAppUpdate(true)).toEqual({ current: '0.6.3' })
    release = { version: '0.7.0', files: [{ name: 'get.sh', size: 1 }] }
    expect(await checkAppUpdate(true)).toEqual({ current: '0.6.3' })
    // the website only ever lists a release, but a prerelease named there still isn't one
    release = apk('0.7.0-rc.1')
    await expect(checkAppUpdate(true)).rejects.toThrow("didn't name a release")
  })

  test('a build from main between releases counts as its release', async () => {
    appVersion = '0.6.3-build.41'
    release = apk('0.6.3')
    expect((await checkAppUpdate(true))?.latest).toBeUndefined()
    release = apk('0.6.10')
    expect((await checkAppUpdate(true))?.latest).toBe('0.6.10')
  })

  test('outside the app it does nothing', async () => {
    delete (window as any).Capacitor
    expect(await checkAppUpdate(true)).toBeUndefined()
    expect(fetched).not.toHaveBeenCalled()
  })

  test('a failed check says so when asked for, and is quiet otherwise', async () => {
    globalThis.fetch = (async () => new Response('not found', { status: 404 })) as unknown as typeof fetch
    await expect(checkAppUpdate(true)).rejects.toThrow('inyon.tech answered 404')
    globalThis.fetch = (async () => new Response('<html>', { status: 200 })) as unknown as typeof fetch
    await expect(checkAppUpdate(true)).rejects.toThrow("inyon.tech didn't name a release")
    // a failed check on its own isn't repeated on every return to the app, only an hour later
    const t = 2_000_000_000
    localStorage.removeItem('inyon.app-update')
    await expect(checkAppUpdate(false, t)).rejects.toThrow()
    globalThis.fetch = (async (url: string) => (fetched(String(url)), Response.json(apk('0.6.4')))) as unknown as typeof fetch
    expect(await checkAppUpdate(false, t + 30 * 60_000)).toEqual({ current: '0.6.3' })
    expect(fetched).not.toHaveBeenCalled()
    expect((await checkAppUpdate(false, t + 61 * 60_000))?.latest).toBe('0.6.4')
  })

  test('one gentle notice per new version', async () => {
    const wait = () => new Promise((r) => setTimeout(r, 20))
    startAppUpdateCheck()
    await wait()
    expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['Inyon 0.6.4 is available'])
    expect(useStore.getState().toasts[0].href).toBe('/settings#updates')
    useStore.setState({ toasts: [] })
    // back to the app the next day, the same version: no second notice
    localStorage.removeItem('inyon.app-update')
    document.dispatchEvent(new Event('visibilitychange'))
    await wait()
    expect(fetched).toHaveBeenCalledTimes(2)
    expect(useStore.getState().toasts).toEqual([])
  })
})

test('checks that start together share one request', async () => {
  localStorage.clear()
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: { App: { getInfo: async () => ({ version: '0.6.3' }) } } }
  let n = 0
  globalThis.fetch = (async () => (n++, Response.json(apk('0.6.4')))) as unknown as typeof fetch
  await Promise.all([checkAppUpdate(), checkAppUpdate(), checkAppUpdate()])
  expect(n).toBe(1)
  delete (window as any).Capacitor
  globalThis.fetch = realFetch
})
