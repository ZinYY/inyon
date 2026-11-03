// Runs the built web app (dist/) in Chromium against the demo world in demo.ts: the hub's API and
// WebSocket are answered here, so no hub, machine or account is involved. Clicks that would change
// something (answer, send, end) get the reply a real hub would send.
import path from 'node:path'
import { chromium, type Browser, type BrowserContext, type Page } from 'playwright'
import type { HostState } from '@inyon/protocol'
import { chat, followUps, hosts, PANES, terminal } from './demo.ts'

export interface Device {
  name: string
  width: number
  height: number
  touch: boolean
  /** how the picture frames it */
  frame: 'browser' | 'phone' | 'tablet'
}
export const DESKTOP: Device = { name: 'desktop', width: 1440, height: 900, touch: false, frame: 'browser' }
export const TABLET: Device = { name: 'tablet', width: 1180, height: 820, touch: true, frame: 'tablet' }
export const PHONE: Device = { name: 'phone', width: 390, height: 844, touch: true, frame: 'phone' }

/** Chrome, or the one CHROME names, with any flags in CHROME_ARGS. */
export const launch = () => chromium.launch({ ...(process.env.CHROME ? { executablePath: process.env.CHROME } : {}), args: process.env.CHROME_ARGS?.split(' ') ?? [] })

export function serve(dist: string) {
  const server = Bun.serve({
    port: 0,
    async fetch(req) {
      const p = new URL(req.url).pathname
      const f = Bun.file(path.join(dist, p))
      // the app routes every other path itself
      return new Response((await f.exists()) && !p.endsWith('/') ? f : Bun.file(path.join(dist, 'index.html')))
    },
  })
  return { base: `http://127.0.0.1:${server.port}`, stop: () => server.stop(true) }
}

type Socket = { send: (m: string) => void }

/** The demo world one page sees, and what a hub would push when it changes. */
export class World {
  hosts: HostState[] = hosts()
  sockets = new Set<Socket>()
  chats = new Map<string, ReturnType<typeof chat>>()

  chatOf(pane: string) {
    if (!this.chats.has(pane)) this.chats.set(pane, chat(pane))
    return this.chats.get(pane)!
  }
  broadcast(msg: unknown) {
    for (const s of this.sockets) s.send(JSON.stringify(msg))
  }
  pushOverview() {
    this.broadcast({ t: 'overview', hosts: this.hosts })
  }
  pane(id: string) {
    for (const h of this.hosts) for (const s of h.sessions) for (const w of s.windows) for (const p of w.panes) if (p.id === id) return { h, s, w, p }
  }
  key(pane: string) {
    return `${this.pane(pane)!.h.config.id}:${pane}`
  }
  append(pane: string, items: any[]) {
    this.chatOf(pane).items.push(...items)
    this.broadcast({ t: 'chat.append', key: this.key(pane), items })
  }
  screen(pane: string, screen: any) {
    this.chatOf(pane).screen = screen
    this.broadcast({ t: 'chat.screen', key: this.key(pane), screen })
  }
  info(pane: string, patch: any) {
    const c = this.chatOf(pane)
    c.info = { ...c.info, ...patch }
    this.broadcast({ t: 'chat.info', key: this.key(pane), info: c.info })
  }
  /** what a hub does after a tap or a message */
  async act(method: string, url: URL, body: any): Promise<unknown> {
    const m = /\/api\/hosts\/([^/]+)\/panes\/([^/]+)\/(\w+)$/.exec(url.pathname)
    if (!m) return {}
    const pane = '%' + decodeURIComponent(m[2])
    const verb = m[3]
    const found = this.pane(pane)
    if (verb === 'answer' && found?.p.agent) {
      setTimeout(() => {
        found.p.agent = { ...found.p.agent!, state: 'working', waitingFor: undefined, options: undefined, activity: 'Running bun test src/auth…' }
        this.pushOverview()
        this.screen(pane, { working: true, inputVisible: true, activity: 'Running bun test src/auth…', width: 120 })
      }, 500)
      setTimeout(() => {
        this.append(pane, followUps.passkeysRan())
        found.p.agent = { ...found.p.agent!, state: 'idle', activity: undefined }
        this.screen(pane, { working: false, inputVisible: true, width: 120 })
        this.pushOverview()
      }, 2600)
    }
    if (verb === 'send') {
      const text = String(body?.text ?? '')
      const items = followUps.reply(text)
      setTimeout(() => {
        this.info(pane, { sends: [{ id: body?.requestId ?? 'r1', text, at: Date.now(), state: 'delivered', uuid: items[0].uuid }] })
        this.append(pane, [items[0]])
        this.screen(pane, { working: true, inputVisible: true, activity: 'Thinking… (3s)', width: 120 })
      }, 400)
      setTimeout(() => {
        this.append(pane, items.slice(1))
        this.screen(pane, { working: true, inputVisible: true, activity: 'Writing the test… (9s)', width: 120 })
      }, 2200)
    }
    if (verb === 'end' && found) {
      setTimeout(() => {
        found.w.panes = found.w.panes.filter((p) => p.id !== pane)
        found.s.windows = found.s.windows.filter((w) => w.panes.length)
        found.h.sessions = found.h.sessions.filter((s) => s.windows.length)
        this.pushOverview()
      }, 700)
      return { closed: 'pane' }
    }
    return {}
  }
  /** the checkout run finishes (for walkthroughs that wait on it) */
  finishCheckout() {
    const pane = PANES.checkout
    this.append(pane, followUps.checkoutDone())
    const f = this.pane(pane)!
    f.p.agent = { ...f.p.agent!, state: 'idle', activity: undefined }
    this.screen(pane, { working: false, inputVisible: true, width: 120 })
    this.pushOverview()
  }
}

export async function open(browser: Browser, base: string, device: Device, scheme: 'light' | 'dark', scale = 2) {
  const ctx: BrowserContext = await browser.newContext({
    viewport: { width: device.width, height: device.height },
    deviceScaleFactor: scale,
    isMobile: device.frame === 'phone',
    hasTouch: device.touch,
    colorScheme: scheme,
    reducedMotion: 'no-preference',
    userAgent: device.frame === 'phone'
      ? 'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Mobile Safari/537.36'
      : undefined,
  })
  const world = new World()
  await ctx.addInitScript(() => {
    // the first-run tips and the notification ask would cover the screen
    try {
      localStorage.setItem('inyon.install-dismissed', '1')
    } catch {}
  })
  await ctx.route(/\/api\//, async (r) => {
    const url = new URL(r.request().url())
    const method = r.request().method()
    if (url.pathname === '/api/session') return r.fulfill({ json: { authed: true, version: '0.10.0' } })
    if (url.pathname.endsWith('/commands') || url.pathname.endsWith('/files')) return r.fulfill({ json: url.pathname.endsWith('/files') ? { cwd: '', files: [] } : [] })
    let body: any
    try {
      body = r.request().postDataJSON()
    } catch {}
    return r.fulfill({ json: method === 'GET' ? {} : await world.act(method, url, body) })
  })
  await ctx.routeWebSocket(/\/ws(\?|$)/, (ws) => {
    world.sockets.add(ws)
    ws.onClose(() => world.sockets.delete(ws))
    ws.onMessage((raw) => {
      const msg = JSON.parse(String(raw))
      if (msg.t === 'sub.overview') ws.send(JSON.stringify({ t: 'overview', hosts: world.hosts }))
      if (msg.t === 'sub.chat') {
        const c = world.chatOf(msg.pane)
        const key = `${msg.host}:${msg.pane}`
        ws.send(JSON.stringify({ t: 'chat.reset', key, info: c.info, items: c.items, hasMore: false }))
        ws.send(JSON.stringify({ t: 'chat.screen', key, screen: c.screen }))
      }
      if (msg.t === 'term.open') ws.send(JSON.stringify({ t: 'term.data', id: msg.id, d: terminal(msg.cols) }))
      if (msg.t === 'ping') ws.send(JSON.stringify({ t: 'pong' }))
    })
  })
  const page: Page = await ctx.newPage()
  return { ctx, page, world }
}

/** Waits for the app to settle and clears anything that only shows on a first visit. */
export async function settle(page: Page, ms = 1200) {
  await page.waitForTimeout(ms)
  for (const name of ['Not now', 'Dismiss', 'Later']) {
    const b = page.getByRole('button', { name, exact: true })
    if (await b.count()) await b.first().click().catch(() => {})
  }
  await page.waitForTimeout(200)
}

export const paneUrl = (base: string, host: string, pane: string, view?: 'terminal') => `${base}/m/${host}/p/${pane.slice(1)}${view ? `?view=${view}` : ''}`
