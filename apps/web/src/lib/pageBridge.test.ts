import { describe, expect, mock, test } from 'bun:test'
import { bridgeFrame, fetchPageFile, virtualSrc, type PageFetch } from './pageBridge.ts'
import { TAG, VBASE } from '../pageview/protocol.ts'

const BASE = '/v/v1.tok.sig/'

/** The phone core, as far as the bridge sees it: every path it was asked for, and its answers by path. */
function core(answers: Record<string, () => Response> = {}) {
  const asked: string[] = []
  const fetch: PageFetch = async (path, method) => {
    asked.push(`${method} ${path}`)
    return answers[path]?.() ?? new Response('Not found', { status: 404, headers: { 'content-type': 'text/plain' } })
  }
  return { fetch, asked }
}

describe("a page's file, by its address under VBASE", () => {
  test("is asked for under the frame's own link, with its query and without its fragment", async () => {
    const c = core({ [`${BASE}mockups/screens/a.html?theme=dark`]: () => new Response('<p>A</p>', { headers: { 'content-type': 'text/html' } }) })
    const got = await fetchPageFile(BASE, `${VBASE}mockups/screens/a.html?theme=dark#top`, 'GET', c.fetch)
    expect(c.asked).toEqual([`GET ${BASE}mockups/screens/a.html?theme=dark`])
    expect(got).toMatchObject({ status: 200, type: 'text/html', url: `${VBASE}mockups/screens/a.html?theme=dark` })
    expect(new TextDecoder().decode(got.body)).toBe('<p>A</p>')
  })

  test("can't climb out of the link: dot segments are gone before the core is asked", async () => {
    const c = core()
    for (const u of [`${VBASE}../../api/hosts`, `${VBASE}a/%2e%2e/%2e%2e/api/hosts`, `${VBASE}a\\..\\..\\api/hosts`]) await fetchPageFile(BASE, u, 'GET', c.fetch)
    expect(c.asked).toEqual([`GET ${BASE}api/hosts`, `GET ${BASE}api/hosts`, `GET ${BASE}api/hosts`])
  })

  test('an address anywhere else gets a 404 and the core is never asked', async () => {
    const c = core()
    for (const u of ['https://example.com/', 'http://page.invalid/x', 'https://page.invalid.example.com/x', `${location.origin}/api/hosts`]) {
      expect((await fetchPageFile(BASE, u, 'GET', c.fetch)).status).toBe(404)
    }
    expect(c.asked).toEqual([])
  })

  test('a folder is followed to its slash, and the page told where it ended up', async () => {
    const c = core({
      [`${BASE}docs?x=1`]: () => new Response(null, { status: 301, headers: { location: `${BASE}docs/?x=1` } }),
      [`${BASE}docs/?x=1`]: () => new Response('<h1>Docs</h1>', { headers: { 'content-type': 'text/html' } }),
    })
    const got = await fetchPageFile(BASE, `${VBASE}docs?x=1`, 'GET', c.fetch)
    expect(got).toMatchObject({ status: 200, url: `${VBASE}docs/?x=1` })
  })

  test('a redirect anywhere but the link is passed back, not followed; a loop stops', async () => {
    const away = core({ [`${BASE}x`]: () => new Response(null, { status: 301, headers: { location: '/api/hosts' } }) })
    expect((await fetchPageFile(BASE, `${VBASE}x`, 'GET', away.fetch)).status).toBe(301)
    expect(away.asked).toEqual([`GET ${BASE}x`])
    const loop = core({ [`${BASE}x`]: () => new Response(null, { status: 301, headers: { location: `${BASE}x` } }) })
    expect((await fetchPageFile(BASE, `${VBASE}x`, 'GET', loop.fetch)).status).toBe(301)
    expect(loop.asked.length).toBe(5)
  })

  test('HEAD gives the status and type with no body', async () => {
    const c = core({ [`${BASE}matrix.html`]: () => new Response('<p>m</p>', { headers: { 'content-type': 'text/html' } }) })
    const got = await fetchPageFile(BASE, `${VBASE}matrix.html`, 'HEAD', c.fetch)
    expect(got.status).toBe(200)
    expect(got.body.byteLength).toBe(0)
    expect(c.asked).toEqual([`HEAD ${BASE}matrix.html`])
  })

  test("the frame's first address", () => {
    expect(virtualSrc(`${BASE}mockups/?theme=dark`, BASE)).toBe(`${VBASE}mockups/?theme=dark`)
    expect(virtualSrc(`${BASE}research/a.md?as=note&theme=light`, BASE)).toBe(`${VBASE}research/a.md?as=note&theme=light`)
  })
})

describe('the frames the app answers', () => {
  /** A stand-in for a window: its parent, and what was posted to it. */
  const win = (parent: unknown) => ({ parent, postMessage: mock((_m: unknown, _o: string, _t?: Transferable[]) => {}) })
  const send = (source: unknown, data: unknown) => window.dispatchEvent(new MessageEvent('message', { data, source: source as Window }))
  const until = async (f: () => boolean) => {
    for (let i = 0; i < 50 && !f(); i++) await new Promise((r) => setTimeout(r, 5))
  }

  test("the loader is told what to show; any document inside it gets the page's files; anything else nothing", async () => {
    const loader = win(window)
    const page = win(loader)
    const screen = win(page)
    const stranger = win(window)
    const c = core({ [`${BASE}progress.json`]: () => new Response('{"done":1}', { headers: { 'content-type': 'application/json' } }) })
    const stop = bridgeFrame({ win: loader as unknown as Window, base: BASE, url: `${VBASE}?theme=dark`, theme: 'dark', fetch: c.fetch })
    try {
      send(loader, { tag: TAG, t: 'ready' })
      expect(loader.postMessage.mock.calls[0]![0]).toEqual({ tag: TAG, t: 'open', url: `${VBASE}?theme=dark`, theme: 'dark' })

      // only the loader itself is told what to show
      send(page, { tag: TAG, t: 'ready' })
      expect(page.postMessage).not.toHaveBeenCalled()

      send(screen, { tag: TAG, t: 'get', id: 7, url: `${VBASE}progress.json` })
      await until(() => screen.postMessage.mock.calls.length > 0)
      const [reply, , transfer] = screen.postMessage.mock.calls[0]!
      expect(reply).toMatchObject({ tag: TAG, t: 'got', id: 7, status: 200, type: 'application/json', url: `${VBASE}progress.json` })
      expect(transfer).toEqual([(reply as { body: ArrayBuffer }).body])

      send(stranger, { tag: TAG, t: 'get', id: 8, url: `${VBASE}progress.json` })
      send(page, { tag: 'something-else', t: 'get', id: 9, url: `${VBASE}progress.json` })
      await new Promise((r) => setTimeout(r, 20))
      expect(stranger.postMessage).not.toHaveBeenCalled()
      expect(page.postMessage).not.toHaveBeenCalled()
      expect(c.asked).toEqual([`GET ${BASE}progress.json`])
    } finally {
      stop()
    }
    // once the frame is gone, nothing is answered
    send(screen, { tag: TAG, t: 'get', id: 10, url: `${VBASE}progress.json` })
    await new Promise((r) => setTimeout(r, 20))
    expect(screen.postMessage.mock.calls.length).toBe(1)
  })

  test('a file the core fails on is answered with why', async () => {
    const loader = win(window)
    const stop = bridgeFrame({ win: loader as unknown as Window, base: BASE, url: VBASE, fetch: async () => { throw new Error('Machine is not connected yet') } })
    try {
      send(loader, { tag: TAG, t: 'get', id: 1, url: `${VBASE}index.html` })
      await until(() => loader.postMessage.mock.calls.length > 0)
      expect(loader.postMessage.mock.calls[0]![0]).toEqual({ tag: TAG, t: 'failed', id: 1, error: 'Machine is not connected yet' })
    } finally {
      stop()
    }
  })
})
