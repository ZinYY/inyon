import { afterEach, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import { PageFrame } from './pages/shared.tsx'
import { framesIsolated, installNative } from '../lib/native.ts'
import { renderSettled as render } from '../test/render.tsx'

const flush = () => new Promise((r) => setTimeout(r, 0))
// a page under /v/ in real use; about:blank keeps the test DOM from fetching it
const SRC = 'about:blank'

/** The Android app, whose WebView does (or doesn't) keep Capacitor's bridge to the top frame. */
function android(isolated: boolean | 'no answer') {
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: {
      InyonDevice: { addListener: () => ({ remove() {} }), ...(isolated === 'no answer' ? {} : { framesIsolated: async () => ({ isolated }) }) },
      App: { addListener: () => ({ remove() {} }) },
    },
  }
}

afterEach(() => delete (window as any).Capacitor)

test('a browser shows an agent page in its sandboxed frame', async () => {
  expect(framesIsolated()).toBe(true)
  await render(<PageFrame src={SRC} title="Plan" />)
  expect(screen.getByTitle('Plan').tagName).toBe('IFRAME')
})

test("a WebView that keeps the bridge to the app's own frame shows it too", async () => {
  android(true)
  await act(async () => (installNative(), await flush()))
  await render(<PageFrame src={SRC} title="Plan" />)
  expect(screen.getByTitle('Plan').getAttribute('sandbox')).toBe('allow-scripts allow-forms allow-popups allow-downloads')
})

test('an older WebView, whose bridge every frame can call, opens the page in the browser instead', async () => {
  for (const answer of [false, 'no answer'] as const) {
    android(answer)
    await act(async () => (installNative(), await flush()))
    const { unmount } = await render(<PageFrame src={SRC} title="Plan" />)
    expect(screen.queryByTitle('Plan')).toBeNull()
    expect(screen.getByRole('link', { name: /Open in the browser/ }).getAttribute('href')).toBe(SRC)
    unmount()
  }
})
