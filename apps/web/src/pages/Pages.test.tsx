import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test'
import { act, fireEvent, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import type { PageEntry } from '@inyon/protocol'
import { PagesPage } from './Pages.tsx'
import { AppShell } from '../components/AppShell.tsx'
import { PreviewTab } from '../components/session/Panels.tsx'
import { useStore } from '../lib/store.ts'
import { loadPages, usePages, pageId, type Page } from '../lib/pages.ts'
import { updatedLine } from '../components/pages/shared.tsx'
import { backHandled } from '../lib/back.ts'
import { api } from '../test/fakes.ts'
import { ApiError } from '../lib/api.ts'
import { host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const NOW = Date.now()
const entry = (slug: string, more: Partial<PageEntry> = {}): Page => {
  const e: PageEntry = { host: 'local', slug, name: slug, project: '/home/me/code/custom-tmux', by: 'claude', added: NOW - 86_400_000, folder: `/home/me/code/custom-tmux/private/plans/${slug}`, ...more }
  return { ...e, id: pageId(e.host, e.slug) }
}
const PLAN = entry('ghostex', { name: 'Inyon plan', progress: { done: 41, total: 68, now: 'R2 step 2c: the HTTP layer', updated: new Date(NOW - 3 * 60_000).toISOString() } })
const HANDY = entry('handy', { name: 'Handy redesign', project: '/home/me/code/Handy', added: NOW - 3_600_000 })
const LAPTOP = entry('win-paths', { host: 'laptop', name: 'Windows path test', project: 'C:\\demo', progress: { done: 5, total: 9 } })

const viewport = (width: number, height: number) => (window as any).happyDOM.setViewport({ width, height })
function Where() {
  return <span data-testid="where">{useLocation().pathname}</span>
}
const at = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="pages" element={<PagesPage />} />
        <Route path="pages/:host/:slug" element={<PagesPage />} />
        <Route path="*" element={null} />
      </Routes>
      <Where />
    </MemoryRouter>,
  )

// the frames must never load anything: happy-dom would fetch their address for real
const settings = (window as any).happyDOM.settings
const loadFrames = settings.disableIframePageLoading
beforeAll(() => void (settings.disableIframePageLoading = true))
afterAll(() => void (settings.disableIframePageLoading = loadFrames))

beforeEach(() => {
  useStore.setState({ hosts: [host('local', { config: { id: 'local', kind: 'local', name: 'abhash-arm' } }), host('laptop', { config: { id: 'laptop', kind: 'node', name: 'abhash-laptop' }, status: 'error' })], hostsLoaded: true })
  usePages.setState({ pages: [PLAN, HANDY, LAPTOP], outdated: [], error: undefined, seen: { [HANDY.id]: NOW }, unreachable: [], forgotten: [] })
  useStore.setState({ resolvedTheme: 'light' })
})
afterEach(() => viewport(1024, 768))

describe('the list on a phone', () => {
  beforeEach(() => viewport(390, 844))

  test('grouped by machine, then project; each card has its bar, its now line and when it last moved', async () => {
    await at('/pages')
    const headings = screen.getAllByText(/^abhash-(arm|laptop)$/).map((e) => e.textContent)
    expect(headings).toEqual(['abhash-arm', 'abhash-laptop'])
    expect(screen.getByText('hub')).toBeTruthy()
    // a screen reader hears what the card shows: changed, progress, the now line, when it last moved
    const card = screen.getByRole('link', { name: 'Inyon plan, changed, 41 of 68 done, R2 step 2c: the HTTP layer, updated 3 min ago' })
    expect(within(card).getByRole('progressbar', { name: '41 of 68 done' })).toBeTruthy()
    expect(within(card).getByText('41/68')).toBeTruthy()
    expect(within(card).getByText('R2 step 2c: the HTTP layer')).toBeTruthy()
    expect(within(card).getByText('updated 3 min ago')).toBeTruthy()
    // never opened: marked changed; Handy was opened since it last changed
    expect(within(card).getByLabelText('changed')).toBeTruthy()
    expect(within(card).getByRole('img', { name: 'changed' })).toBeTruthy()
    expect(within(screen.getByRole('link', { name: 'Handy redesign, added 1 h ago' })).queryByLabelText('changed')).toBeNull()
    // the laptop is offline: its card says so
    expect(within(screen.getByRole('link', { name: 'Windows path test, changed, 5 of 9 done, machine offline' })).getByText('Machine offline')).toBeTruthy()
    // projects by their folder's name
    expect(screen.getByText('custom-tmux')).toBeTruthy()
    expect(screen.getByText('demo')).toBeTruthy()
  })

  test("'updated' is when the board last moved, not when the folder was added", () => {
    const p = entry('late', { added: NOW, progress: { done: 1, total: 2, updated: new Date(NOW - 2 * 3_600_000).toISOString() } })
    expect(updatedLine(p, NOW)).toBe('updated 2 h ago')
    // no board, or a date that can't be read: when it was added
    expect(updatedLine(entry('bare', { added: NOW - 60_000 }), NOW)).toBe('added 1 min ago')
    expect(updatedLine(entry('odd', { added: NOW - 60_000, progress: { done: 0, total: 1, updated: 'soon' } }), NOW)).toBe('added 1 min ago')
  })

  test('one menu on a card: open, open in browser with a 12 h link, revoke, show folder, remove', async () => {
    const user = userEvent.setup()
    await at('/pages')
    await user.click(screen.getByRole('button', { name: 'More for Inyon plan' }))
    const items = screen.getAllByRole('menuitem').map((m) => m.textContent)
    expect(items).toEqual(['Open', 'Open in browserA 12 h link. Never sign in there', 'Revoke links', 'Show folder', 'Remove'])
    await user.click(screen.getByRole('menuitem', { name: /Open$/ }))
    expect(screen.getByTestId('where').textContent).toBe('/pages/local/ghostex')
  })

  test('Remove takes it off the list at once and Undo adds the same folder back', async () => {
    const user = userEvent.setup()
    await at('/pages')
    await user.click(screen.getByRole('button', { name: 'More for Inyon plan' }))
    await user.click(screen.getByRole('menuitem', { name: 'Remove' }))
    await flush()
    expect(api.removePage).toHaveBeenCalledWith('local', 'ghostex')
    expect(screen.queryByRole('link', { name: 'Inyon plan' })).toBeNull()
    const toast = useStore.getState().toasts.at(-1)!
    expect([toast.title, toast.body, toast.action?.label]).toEqual(['Removed “Inyon plan”', 'The folder stays on abhash-arm.', 'Undo'])
    toast.action!.onSelect()
    expect(api.addPage).toHaveBeenCalledWith('local', { path: PLAN.folder, name: 'Inyon plan', slug: 'ghostex' })
  })

  test('no pages yet: the command an agent runs, and a way to add a folder', async () => {
    usePages.setState({ pages: [] })
    await at('/pages')
    expect(screen.getByText('No pages yet')).toBeTruthy()
    expect(screen.getByText(/^inyon pages add /)).toBeTruthy()
    expect(screen.getByRole('button', { name: /Add a folder/ })).toBeTruthy()
  })

  test('a computer whose inyon is too old to serve its pages is named, with what to do', async () => {
    usePages.setState({ pages: [], outdated: ['laptop'] })
    await at('/pages')
    expect(screen.getByText('Update inyon on abhash-laptop to open its pages here.')).toBeTruthy()
    expect(screen.getByText('No pages yet')).toBeTruthy()
  })
})

describe('the viewer', () => {
  beforeEach(() => {
    api.openPage.mockResolvedValue({ url: '/v/v1.tok.sig/', expires: Date.now() + 12 * 3_600_000 } as never)
    globalThis.fetch = mock(async () => new Response(null, { status: 404 })) as any
  })

  test('the frame is sandboxed like the hub serves it, without same-origin rights, and starts in the app theme', async () => {
    viewport(390, 844)
    useStore.setState({ resolvedTheme: 'dark' })
    const { container } = await at('/pages/local/ghostex')
    await flush()
    const frame = container.querySelector('iframe')!
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts allow-forms allow-popups allow-downloads')
    expect(frame.getAttribute('sandbox')).not.toContain('allow-same-origin')
    expect(frame.getAttribute('referrerpolicy')).toBe('no-referrer')
    expect(frame.getAttribute('allow')).toBe('')
    expect(frame.getAttribute('src')).toBe('/v/v1.tok.sig/?theme=dark')
    expect(api.openPage).toHaveBeenCalledWith('local', 'ghostex')
    // opening it clears its changed mark
    expect(usePages.getState().seen[PLAN.id]).toBeGreaterThan(0)
  })

  test('a link with less than an hour left is renewed when the app comes back into view, not before', async () => {
    api.openPage.mockResolvedValue({ url: '/v/v1.old.sig/', expires: Date.now() + 11 * 3_600_000 } as never)
    const { container } = await at('/pages/local/ghostex')
    await flush()
    expect(api.openPage).toHaveBeenCalledTimes(1)
    await act(async () => void window.dispatchEvent(new Event('focus')))
    expect(api.openPage).toHaveBeenCalledTimes(1)
    // the same link, later: 50 minutes left
    api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000 } as never)
    const realNow = Date.now
    Date.now = () => realNow() + 10 * 3_600_000 + 10 * 60_000
    try {
      await act(async () => void window.dispatchEvent(new Event('focus')))
      await flush()
    } finally {
      Date.now = realNow
    }
    expect(api.openPage).toHaveBeenCalledTimes(2)
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
  })

  test("the app answers only its frame's messages, and only the kinds it knows", async () => {
    const { container } = await at('/pages/local/ghostex')
    await flush()
    // frames don't load here, so the frame's window is a stand-in
    const post = mock(() => {})
    const win = { postMessage: post } as unknown as Window
    Object.defineProperty(container.querySelector('iframe')!, 'contentWindow', { value: win })
    const send = (source: unknown, data: unknown) => act(async () => void window.dispatchEvent(new MessageEvent('message', { data, source: source as Window })))
    await send(window, { type: 'theme?' })
    await send(win, { type: 'navigate', url: '/api/session' })
    await send(win, 'theme?')
    expect(post).not.toHaveBeenCalled()
    await send(win, { type: 'theme?' })
    expect(post).toHaveBeenCalledWith({ type: 'theme', value: 'light' }, '*')
    // a theme change in the app goes in as a message, without reloading the frame
    post.mockClear()
    await act(async () => void useStore.setState({ resolvedTheme: 'dark' }))
    expect(post).toHaveBeenCalledWith({ type: 'theme', value: 'dark' }, '*')
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/?theme=light')
  })

  test("a page on a computer paired with the phone opens in the app's own viewer, in the same sandbox, with no way out to a browser", async () => {
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'web',
      Plugins: { InyonDevice: { framesIsolated: async () => ({ isolated: true }), addListener: () => {}, takeLaunchHref: async () => ({}) }, App: { addListener: () => {} } },
    }
    try {
      const { installNative } = await import('../lib/native.ts')
      installNative()
      await flush()
      viewport(390, 844)
      useStore.setState({ hosts: [host('laptop', { config: { id: 'laptop', kind: 'link', name: 'abhash-laptop' }, status: 'online' })] })
      api.openPage.mockResolvedValue({ url: '/v/v1.phone.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      const { container } = await at('/pages/laptop/win-paths')
      await flush()
      const frame = container.querySelector('iframe')!
      expect(frame.getAttribute('src')).toBe('/pageview.html')
      expect(frame.getAttribute('sandbox')).toBe('allow-scripts allow-forms allow-popups allow-downloads')
      expect(api.openPage).toHaveBeenCalledWith('laptop', 'win-paths')
      // its parts are asked of the phone core, which answers the link
      expect(api.pageFile).toHaveBeenCalledWith('/v/v1.phone.sig/mockups/', 'HEAD')
      await userEvent.click(screen.getByRole('button', { name: 'More' }))
      expect(screen.getByRole('menuitem', { name: /Reload/ })).toBeTruthy()
      expect(screen.queryByRole('menuitem', { name: /Open in browser/ })).toBeNull()
    } finally {
      delete (window as any).Capacitor
    }
  })

  test('on a phone, back closes the viewer instead of walking back through the frame', async () => {
    viewport(390, 844)
    await at('/pages/local/ghostex')
    await flush()
    let handled = false
    await act(async () => void (handled = backHandled()))
    expect(handled).toBe(true)
    expect(screen.getByTestId('where').textContent).toBe('/pages')
  })

  test('the phone menu offers Plan, Mockups and Matrix only when the site has them', async () => {
    viewport(390, 844)
    globalThis.fetch = mock(async (url: string) => new Response(null, { status: String(url).endsWith('mockups/') ? 200 : 404 })) as any
    const user = userEvent.setup()
    await at('/pages/local/ghostex')
    await flush()
    await flush()
    await user.click(screen.getByRole('button', { name: 'More' }))
    expect(screen.getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Reload', 'Plan', 'Mockups', 'Open in browserA 12 h link. Never sign in there'])
    await user.click(screen.getByRole('menuitem', { name: 'Mockups' }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/mockups/?theme=light')
  })

  test("on a phone, a site's research notes open from the Notes button, and Back returns to the plan", async () => {
    viewport(390, 844)
    usePages.setState({ pages: [{ ...PLAN, notes: ['ghostex features.md', 'plan.md'] }, HANDY, LAPTOP] })
    const user = userEvent.setup()
    const first = await at('/pages/local/ghostex')
    await flush()
    await user.click(screen.getByRole('button', { name: 'Notes' }))
    expect(screen.getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['ghostex features', 'plan'])
    await user.click(screen.getByRole('menuitem', { name: 'ghostex features' }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/research/ghostex%20features.md?as=note&theme=light')
    expect(screen.getByRole('button', { name: 'Notes: ghostex features open' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'Back to Plan' }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/?theme=light')
    // a page without notes has no Notes button
    first.unmount()
    await at('/pages/local/handy')
    await flush()
    expect(screen.queryByRole('button', { name: 'Notes' })).toBeNull()
  })

  test('on a desktop, a note takes the place of the part, and a part brings the page back', async () => {
    globalThis.fetch = mock(async (url: string) => new Response(null, { status: String(url).endsWith('mockups/') ? 200 : 404 })) as any
    usePages.setState({ pages: [{ ...PLAN, notes: ['plan.md'] }, HANDY, LAPTOP] })
    const user = userEvent.setup()
    await at('/pages/local/ghostex')
    await flush()
    await flush()
    await user.click(screen.getByRole('button', { name: 'Notes' }))
    await user.click(screen.getByRole('menuitem', { name: 'plan' }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/research/plan.md?as=note&theme=light')
    expect(screen.getByRole('radio', { name: 'Plan' }).getAttribute('aria-checked')).toBe('false')
    await user.click(screen.getByRole('radio', { name: 'Mockups' }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/mockups/?theme=light')
    // a note no longer listed (deleted from the folder) gives way to the page
    await user.click(screen.getByRole('button', { name: 'Notes' }))
    await user.click(screen.getByRole('menuitem', { name: 'plan' }))
    act(() => usePages.setState({ pages: [{ ...PLAN, notes: [] }, HANDY, LAPTOP] }))
    expect(document.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.tok.sig/mockups/?theme=light')
  })

  test('on a desktop, the list and the viewer sit side by side, with device sizes', async () => {
    const user = userEvent.setup()
    const { container } = await at('/pages/local/ghostex')
    await flush()
    expect(screen.getByRole('navigation', { name: 'Page location' }).textContent).toBe('abhash-arm/custom-tmux/Inyon plan')
    expect(screen.getByRole('link', { name: /^Handy redesign/ }).getAttribute('href')).toBe('/pages/local/handy')
    await user.click(screen.getByRole('radio', { name: 'Phone 390 × 844' }))
    const box = container.querySelector('iframe')!.parentElement as HTMLElement
    expect([box.style.width, box.style.height]).toEqual(['390px', '844px'])
    expect(screen.getByText(/^Phone · 390 × 844/)).toBeTruthy()
  })

  test("a page shows its sandbox on the phone too, so it can't pass itself off as the app asking for a password", async () => {
    viewport(390, 844)
    await at('/pages/local/ghostex')
    await flush()
    expect(screen.getByRole('banner').textContent).toContain('Sandboxed page: never sign in here')
  })

  test('picking another page while the first is still opening shows the one picked, whatever answers last', async () => {
    viewport(390, 844)
    let answerFirst!: (v: unknown) => void
    api.openPage.mockImplementationOnce((() => new Promise((r) => (answerFirst = r))) as never)
    api.openPage.mockImplementationOnce((async () => ({ url: '/v/v1.handy.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 })) as never)
    const user = userEvent.setup()
    const { container } = await at('/pages/local/ghostex')
    await flush()
    await user.click(screen.getByRole('button', { name: 'Machine: abhash-arm' }))
    await user.click(screen.getByRole('menuitem', { name: 'Handy redesign' }))
    await flush()
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.handy.sig/?theme=light')
    // the first page's link arrives late: it's dropped
    await act(async () => answerFirst({ url: '/v/v1.plan.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 }))
    await flush()
    expect(screen.getByTestId('where').textContent).toBe('/pages/local/handy')
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.handy.sig/?theme=light')
  })

  test('a page left on screen renews its link by a timer an hour before it runs out, with no focus event', async () => {
    const timers: { fn: () => void; ms: number }[] = []
    const realSet = globalThis.setTimeout
    // long timers are kept here instead of run; short ones (React, the test library) run as usual
    globalThis.setTimeout = ((fn: () => void, ms?: number, ...a: unknown[]) => ((ms ?? 0) > 60_000 ? (timers.push({ fn, ms: ms! }), 0) : realSet(fn, ms, ...a))) as typeof setTimeout
    try {
      api.openPage.mockResolvedValue({ url: '/v/v1.old.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      const { container } = await at('/pages/local/ghostex')
      await flush()
      const renew = timers.find((t) => Math.abs(t.ms - 11 * 3_600_000) < 60_000)
      expect(renew).toBeTruthy()
      api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      await act(async () => renew!.fn())
      await flush()
      expect(api.openPage).toHaveBeenCalledTimes(2)
      expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
    } finally {
      globalThis.setTimeout = realSet
    }
  })

  test('a timed renewal that fails is tried again after 1 min, then longer up to 10 min, while the page is on screen', async () => {
    let timers: { fn: () => void; ms: number }[] = []
    const realSet = globalThis.setTimeout
    // timers of a minute or more are kept here instead of run; short ones (React, the test library) run as usual
    globalThis.setTimeout = ((fn: () => void, ms?: number, ...a: unknown[]) => ((ms ?? 0) >= 60_000 ? (timers.push({ fn, ms: ms! }), 0) : realSet(fn, ms, ...a))) as typeof setTimeout
    const fire = async (ms: number) => {
      const t = timers.find((x) => Math.abs(x.ms - ms) < 60_000)
      expect([ms, !!t]).toEqual([ms, true])
      timers = []
      await act(async () => t!.fn())
      await flush()
    }
    try {
      api.openPage.mockResolvedValue({ url: '/v/v1.old.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      const { container } = await at('/pages/local/ghostex')
      await flush()
      api.openPage.mockRejectedValue(new Error("Couldn't reach the hub") as never)
      await fire(11 * 3_600_000)
      // each failure waits longer: 1, 2, 4, 8 min, then 10 min from there on
      for (const wait of [60_000, 120_000, 240_000, 480_000, 600_000, 600_000]) await fire(wait)
      expect(api.openPage).toHaveBeenCalledTimes(8)
      // the old link stays on screen meanwhile
      expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.old.sig/?theme=light')
      api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      await fire(600_000)
      expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
      // the new link's own renewal is on the clock again, and no retry is left over
      expect(timers.map((t) => Math.round(t.ms / 3_600_000))).toEqual([11])
      // out of view the timer doesn't try at all: coming back into view renews it
      api.openPage.mockRejectedValue(new Error("Couldn't reach the hub") as never)
      const hidden = Object.getOwnPropertyDescriptor(Document.prototype, 'visibilityState')
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
      try {
        await fire(11 * 3_600_000)
        expect(timers).toEqual([])
        expect(api.openPage).toHaveBeenCalledTimes(9)
      } finally {
        delete (document as any).visibilityState
        if (hidden) Object.defineProperty(Document.prototype, 'visibilityState', hidden)
      }
    } finally {
      globalThis.setTimeout = realSet
    }
  })

  test('a renewal on coming back into view that fails is tried again after 1 min, then longer, without waiting for another focus', async () => {
    let timers: { fn: () => void; ms: number }[] = []
    const realSet = globalThis.setTimeout
    const realNow = Date.now
    globalThis.setTimeout = ((fn: () => void, ms?: number, ...a: unknown[]) => ((ms ?? 0) >= 60_000 ? (timers.push({ fn, ms: ms! }), 0) : realSet(fn, ms, ...a))) as typeof setTimeout
    const fire = async (ms: number) => {
      const t = timers.find((x) => x.ms === ms)
      expect([ms, !!t]).toEqual([ms, true])
      timers = []
      await act(async () => t!.fn())
      await flush()
    }
    try {
      api.openPage.mockResolvedValue({ url: '/v/v1.old.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      const { container } = await at('/pages/local/ghostex')
      await flush()
      timers = []
      // back after 11 h 10 min (the timer slept with the device), and the hub doesn't answer
      Date.now = () => realNow() + 11 * 3_600_000 + 10 * 60_000
      api.openPage.mockRejectedValue(new Error("Couldn't reach the hub") as never)
      await act(async () => {
        window.dispatchEvent(new Event('focus'))
        document.dispatchEvent(new Event('visibilitychange'))
      })
      await flush()
      // both events, one try
      expect(api.openPage).toHaveBeenCalledTimes(2)
      for (const wait of [60_000, 120_000, 240_000, 480_000, 600_000]) await fire(wait)
      expect(api.openPage).toHaveBeenCalledTimes(7)
      api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      await fire(600_000)
      expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
      expect(timers.map((t) => Math.round(t.ms / 3_600_000))).toEqual([11])
    } finally {
      globalThis.setTimeout = realSet
      Date.now = realNow
    }
  })

  test("renewal goes by how long the hub says the link lasts, not by the hub's clock", async () => {
    // the hub's clock is two hours ahead of this device's
    api.openPage.mockResolvedValue({ url: '/v/v1.old.sig/', expires: Date.now() + 14 * 3_600_000, ttl: 12 * 3_600_000 } as never)
    const { container } = await at('/pages/local/ghostex')
    await flush()
    api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 14 * 3_600_000, ttl: 12 * 3_600_000 } as never)
    const realNow = Date.now
    // 11 h 10 min later here: 50 minutes left, though the hub's expiry looks almost 3 h away
    Date.now = () => realNow() + 11 * 3_600_000 + 10 * 60_000
    try {
      await act(async () => void window.dispatchEvent(new Event('focus')))
      await flush()
    } finally {
      Date.now = realNow
    }
    expect(api.openPage).toHaveBeenCalledTimes(2)
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
  })

  test('Reload asks for a new link every time, so a link revoked from another device comes back', async () => {
    viewport(390, 844)
    const user = userEvent.setup()
    const { container } = await at('/pages/local/ghostex')
    await flush()
    api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
    await user.click(screen.getByRole('button', { name: 'More' }))
    await user.click(screen.getByRole('menuitem', { name: 'Reload' }))
    await flush()
    expect(api.openPage).toHaveBeenCalledTimes(2)
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
  })

  test('on a desktop, clicking the open page in the list again opens it with a new link', async () => {
    const user = userEvent.setup()
    const { container } = await at('/pages/local/ghostex')
    await flush()
    api.openPage.mockResolvedValue({ url: '/v/v1.new.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
    await user.click(screen.getByRole('link', { name: /^Inyon plan/ }))
    await flush()
    expect(api.openPage).toHaveBeenCalledTimes(2)
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('/v/v1.new.sig/?theme=light')
  })

  test('Open in browser, from the phone viewer or the desktop one, mints a fresh 12 h link', async () => {
    const tab = { opener: {}, location: { href: '' }, close: mock(() => {}) }
    const realOpen = window.open
    window.open = mock(() => tab) as never
    try {
      viewport(390, 844)
      const user = userEvent.setup()
      await at('/pages/local/ghostex')
      await flush()
      api.openPage.mockResolvedValue({ url: '/v/v1.fresh.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      await user.click(screen.getByRole('button', { name: 'More' }))
      await user.click(screen.getByRole('menuitem', { name: /^Open in browser/ }))
      await flush()
      expect(api.openPage).toHaveBeenCalledTimes(2)
      expect(tab.location.href).toBe('http://localhost:5173/v/v1.fresh.sig/')
      expect(tab.opener).toBeNull()

      viewport(1024, 768)
      api.openPage.mockResolvedValue({ url: '/v/v1.desk.sig/', expires: Date.now() + 12 * 3_600_000, ttl: 12 * 3_600_000 } as never)
      await act(async () => window.dispatchEvent(new Event('resize')))
      await flush()
      const calls = api.openPage.mock.calls.length
      await user.click(screen.getByRole('button', { name: 'Open in new tab' }))
      await flush()
      expect(api.openPage.mock.calls.length).toBe(calls + 1)
      expect(tab.location.href).toBe('http://localhost:5173/v/v1.desk.sig/')
    } finally {
      window.open = realOpen
    }
  })

  test('the Plan control loads the plan again after the page moved on by its own links', async () => {
    globalThis.fetch = mock(async (url: string) => new Response(null, { status: String(url).endsWith('mockups/') ? 200 : 404 })) as any
    const user = userEvent.setup()
    const { container } = await at('/pages/local/ghostex')
    await flush()
    await flush()
    // the frame went to the mockups by a link inside it; the app still says Plan
    const before = container.querySelector('iframe')!
    expect(screen.getByRole('radio', { name: 'Plan' }).getAttribute('aria-checked')).toBe('true')
    await user.click(screen.getByRole('radio', { name: 'Plan' }))
    const after = container.querySelector('iframe')!
    expect(after).not.toBe(before)
    expect(after.getAttribute('src')).toBe('/v/v1.tok.sig/?theme=light')
  })

  test("a hub that misses a poll keeps the page on screen, marked out of date, and never says it's gone", async () => {
    viewport(390, 844)
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 }]))
    const far = entry('far', { host: 'hub:h1/local', name: 'Far plan' })
    usePages.setState({ pages: [far], unreachable: [] })
    const { container } = await at('/pages/hub%3Ah1%2Flocal/far')
    await flush()
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('http://arm:4777/v/v1.tok.sig/?theme=light')
    await act(async () => usePages.setState({ unreachable: ['h1'] }))
    expect(container.querySelector('iframe')).toBeTruthy()
    expect(screen.getByRole('status').textContent).toContain("Can't reach")
    expect(screen.queryByText('This page is gone')).toBeNull()
    // opened while its hub is out of reach and not in the list: out of reach, not gone
    await act(async () => usePages.setState({ pages: [] }))
    expect(screen.getByText('The hub is out of reach')).toBeTruthy()
    expect(screen.queryByText('This page is gone')).toBeNull()
  })

  test('a hub that forgot this phone says so in the list and over its page, which is not called gone', async () => {
    viewport(390, 844)
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 }]))
    api.hubPages.mockRejectedValue(new ApiError(401, 'Unauthorized') as never)
    usePages.setState({ pages: [entry('far', { host: 'hub:h1/local', name: 'Far plan' })] })
    try {
      const viewer = await at('/pages/hub%3Ah1%2Flocal/far')
      await act(() => loadPages())
      expect(usePages.getState()).toMatchObject({ pages: [], forgotten: ['h1'], unreachable: [] })
      expect(screen.getByText('arm forgot this phone')).toBeTruthy()
      expect(screen.queryByText('This page is gone')).toBeNull()
      expect(screen.queryByText('The hub is out of reach')).toBeNull()
      viewer.unmount()
      const list = await at('/pages')
      await flush()
      expect(screen.getByRole('status').textContent).toBe('arm forgot this phone. Pair it again from the hub to see its pages here.')
      list.unmount()
      viewport(1280, 800)
      await at('/pages/hub%3Ah1%2Flocal/far')
      await flush()
      expect(screen.getByText('arm forgot this phone')).toBeTruthy()
    } finally {
      delete (window as any).Capacitor
    }
  })

  test('on a desktop, /pages opens the page that changed last', async () => {
    await at('/pages')
    await flush()
    expect(screen.getByTestId('where').textContent).toBe('/pages/local/ghostex')
  })
})

describe('the shell', () => {
  test('the phone tab bar has five places, Pages second, with the count of changed pages', async () => {
    viewport(390, 844)
    api.pages.mockResolvedValue({ pages: [PLAN, HANDY] } as never)
    await render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<div />} />
          </Route>
        </Routes>
      </MemoryRouter>,
    )
    await flush()
    const bar = screen.getAllByRole('navigation').find((n) => n.className.includes('grid-cols-5'))!
    expect(within(bar).getAllByRole('link').map((a) => a.textContent)).toEqual(['Overview', '1Pages', 'Machines', 'Settings'])
    expect(within(bar).getByLabelText('1 changed')).toBeTruthy()
  })

  test('a page open on a phone hides the tab bar', async () => {
    viewport(390, 844)
    api.openPage.mockResolvedValue({ url: '/v/t/', expires: Date.now() + 1e7 } as never)
    await render(
      <MemoryRouter initialEntries={['/pages/local/ghostex']}>
        <Routes>
          <Route element={<AppShell />}>
            <Route path="pages/:host/:slug" element={<div />} />
          </Route>
        </Routes>
      </MemoryRouter>,
    )
    expect(screen.getAllByRole('navigation').find((n) => n.className.includes('grid-cols-5'))).toBeUndefined()
  })
})

describe('the Preview tab', () => {
  test("refuses to frame Inyon's own address", async () => {
    localStorage.setItem('inyon.preview.box:%1', 'http://localhost:5173/settings')
    const { container } = await render(<PreviewTab storageKey="box:%1" suggestions={[]} />)
    expect(container.querySelector('iframe')).toBeNull()
    expect(screen.getByText(/That address is Inyon itself/)).toBeTruthy()
    fireEvent.change(screen.getByRole('textbox', { name: 'Preview address' }), { target: { value: 'http://localhost:3000' } })
    fireEvent.submit(screen.getByRole('textbox', { name: 'Preview address' }).closest('form')!)
    expect(container.querySelector('iframe')!.getAttribute('src')).toBe('http://localhost:3000')
  })
})
