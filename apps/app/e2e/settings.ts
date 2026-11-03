// bun apps/app/e2e/settings.ts — Settings and the rest of the built app (apps/app/dist, from build.sh)
// in Chromium behind a stand-in hub: the page under the hub's own CSP for /next, the /ws2 overview
// and the settings endpoints, fed made-up data (demo.ts). Checks Settings at phone and desktop sizes
// in light and dark, the theme and the other app settings (kept with the web app's own keys), the
// hub's settings, custom agents with their menus and Undo, the companion (start, its sidebar item and
// Overview line, stop), pairing a companion app with a fresh sign-in, a companion's details,
// changing the password, the hub update, "Inyon was updated", notices as toasts, ending a session,
// Temp files (pick, delete, a folder's files, a preview) and the cold-start intro, and saves a
// screenshot of each to $SHOTS (default: a temp folder). REACT=1 also serves the web app
// (apps/web/dist, from its build) at / and saves its Settings and Temp files beside them, to compare.
// CHROME=/path/to/chrome uses a browser other than Playwright's own.
import { existsSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium, devices, type BrowserContext, type Page } from 'playwright'
import type { ServerWebSocket } from 'bun'
import { overview } from './demo.ts'

const root = path.join(import.meta.dir, '../../..')
const dist = path.join(import.meta.dir, '../dist')
const web = path.join(root, 'apps/web/public')
const react = path.join(root, 'apps/web/dist')
const withReact = !!process.env.REACT && existsSync(react)
const shots = process.env.SHOTS ?? path.join(tmpdir(), 'inyon-app-shots')
mkdirSync(shots, { recursive: true })

// the policy the hub sends with the page, read from the hub's source so the two can't drift
const rs = await Bun.file(path.join(root, 'crates/inyon-hub/src/static_files.rs')).text()
const NEXT_CSP = /pub const NEXT_CSP: &str =\s*"([^"]+)"/.exec(rs)![1]!
// the version the page is built as: the hub serving it runs the same
const cargo = await Bun.file(path.join(root, 'Cargo.toml')).text()
const VERSION = /^version = "([^"]+)"/m.exec(cargo)![1]!
const PASSWORD = 'correct horse'
const HOUR = 3600_000
const DAY = 24 * HOUR

type Entry = Record<string, any>
function temp(now: number) {
  return {
    roots: [
      { id: 'claude', label: 'Claude Code scratch', size: 5_400_000, exists: true },
      { id: 'uploads', label: 'Uploads', size: 820_000, exists: true },
      { id: 'empty', label: 'Codex', size: 0, exists: true },
    ],
    items: [
      { root: 'claude', path: '/tmp/claude-1000/storefront', name: 'storefront', kind: 'dir', size: 4_200_000, files: 6, modified: now - 12 * 60_000, recent: true, images: ['/tmp/claude-1000/storefront/shot-1.png'] },
      { root: 'claude', path: '/tmp/claude-1000/accounts', name: 'accounts', kind: 'dir', size: 1_200_000, files: 2, modified: now - 9 * DAY, recent: false, images: [] },
      { root: 'uploads', path: '/home/sam/.inyon/uploads/2026-10-20', name: '2026-10-20', kind: 'dir', size: 820_000, files: 3, modified: now - 13 * DAY, recent: false, images: [] },
    ],
  }
}

const state = {
  hosts: overview(Date.now()).hosts as Entry[],
  settings: { uploads: { retentionDays: 7, maxMB: 100, maxTotalMB: 1024, keepMe: 1 }, autoResume: false, machinesSend: false, customAgents: [] as Entry[], futureKey: 'kept' } as Entry,
  sleep: [{ scope: 'all', minutes: 60 }] as Entry[],
  phones: [{ id: 'ph1', name: 'Pixel 9', pairedAt: Date.now() - 20 * DAY, lastSeen: Date.now() - 5 * 60_000 }] as Entry[],
  companions: [{ id: 'c1', name: 'Legato on desk', app: 'Legato for Windows', scopes: ['overview', 'chat', 'answer', 'send'], contractVersion: 1, pairedAt: Date.now() - 3 * DAY, lastSeen: Date.now() - 2 * HOUR }] as Entry[],
  companion: { agent: null as string | null, host: 'local', pane: null as string | null },
  // pairing a companion app needs a fresh sign-in once
  pairFresh: false,
  logoutFails: false,
  // the next reads of the companion list that fail, whether making a code fails, and how long one lasts
  companionsFail: 0,
  pairFails: false,
  pairTtl: 5 * 60_000,
  // whether listing temp files fails, and how late the next listings answer
  tempFails: false,
  tempLag: [] as number[],
  endFails: false,
  endLag: 0,
  // the pane closes on its own while the question is open: the end then finds nothing (404)
  endGone: false,
  // how late the next sleep changes are taken
  sleepLag: [] as number[],
  // whether reading the phones fails, and the next reads of the hub's settings that fail
  phonesFail: false,
  settingsReadFails: 0,
  // temp paths a delete can't remove
  tempKeep: [] as string[],
  // how late the next settings saves are taken, and whether saving fails
  settingsLag: [] as number[],
  settingsFail: false,
  // how long the machine takes to report a stopped companion's pane gone
  reportLag: 50,
  commit: 'aaa1111',
  update: { current: VERSION, latest: '0.25.1', canUpdate: true, running: false } as Entry,
  temp: temp(Date.now()),
  calls: [] as { method: string; path: string; body?: any }[],
}
const called = (method: string, p: string) => state.calls.filter((c) => c.method === method && c.path === p)

type Socket = ServerWebSocket<{ subs: Set<string>; v1: boolean }>
const sockets = new Set<Socket>()
let seq = 1
const frame = () => JSON.stringify({ t: 'snapshot', stream: 'overview', epoch: 1, seq, rows: { hosts: state.hosts } })
function publish() {
  seq++
  for (const ws of sockets) {
    if (ws.data.v1) ws.send(JSON.stringify({ t: 'overview', hosts: state.hosts }))
    else if (ws.data.subs.has('overview')) ws.send(frame())
  }
}
const hub = (msg: Entry) => {
  for (const ws of sockets) if (!ws.data.v1) ws.send(JSON.stringify(msg))
}

/** The companion's pane on the hub's machine, as the overview marks it. */
function companionPane(kind: string) {
  const now = Date.now()
  return {
    id: '$30', name: 'companion', attached: 0,
    windows: [{ id: '@30', index: 1, name: 'companion', active: true, activity: Math.floor(now / 1000), panes: [
      { id: '%30', index: 0, active: true, command: kind, cwd: '/home/sam', title: 'Companion', width: 80, height: 40, dead: false, companion: true, agent: { pid: 4100, startedAt: now, mappedBy: 'hook', kind, state: 'idle' } },
    ] }],
  }
}

const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

const server = Bun.serve({
  port: 0,
  async fetch(req, srv) {
    const url = new URL(req.url)
    const p = url.pathname
    const json = (body: unknown, status = 200) => Response.json(body, { status })
    if (p === '/ws2') return srv.upgrade(req, { data: { subs: new Set<string>(), v1: false } }) ? undefined : new Response('no upgrade', { status: 400 })
    if (p === '/ws') return srv.upgrade(req, { data: { subs: new Set<string>(), v1: true } }) ? undefined : new Response('no upgrade', { status: 400 })
    if (p === '/api/session') return json({ authed: true, setup: false, setupCode: false, version: VERSION, commit: state.commit })
    let body: any
    if (p.startsWith('/api/')) {
      body = req.method === 'GET' || req.method === 'DELETE' ? undefined : await req.json().catch(() => undefined)
      state.calls.push({ method: req.method, path: p + url.search, body })
    }
    if (p === '/api/settings') {
      if (req.method === 'PUT') {
        if (state.settingsFail) return json({ error: 'The disk is full' }, 500)
        // a slow save is also taken late, after a quicker one that came in behind it
        const lag = state.settingsLag.shift() ?? 0
        if (lag) return new Promise<Response>((r) => setTimeout(() => r(json((state.settings = body))), lag))
        state.settings = body
      }
      if (state.settingsReadFails > 0) {
        state.settingsReadFails--
        return json({ error: 'No settings here' }, 404)
      }
      return json(state.settings)
    }
    if (p === '/api/organize') return json({ v: 2, pins: [], parked: [], sleep: state.sleep })
    if (p === '/api/organize/sleep') {
      // a slow change is taken when it arrives, late
      const lag = state.sleepLag.shift() ?? 0
      if (lag) await Bun.sleep(lag)
      state.sleep = body.minutes ? [{ scope: 'all', minutes: body.minutes }] : []
      return json(state.sleep)
    }
    if (p === '/api/password') return body?.current === PASSWORD ? json({ ok: true }) : json({ error: 'That password is wrong' }, 403)
    if (p === '/api/logout') return state.logoutFails ? json({ error: 'The hub is restarting' }, 503) : json({ ok: true })
    if (p === '/api/login') {
      if (body?.password !== PASSWORD) return json({ error: 'Wrong password' }, 401)
      state.pairFresh = true
      return json({ ok: true })
    }
    if (p === '/api/hub/update') {
      if (req.method === 'POST') state.update = { ...state.update, running: true }
      return json(state.update)
    }
    if (p === '/api/push') return json({ ok: true })
    if (p === '/api/push/test') return json({ sent: 1 })
    if (p === '/api/phones') return state.phonesFail ? json({ error: 'The hub is busy' }, 500) : json(state.phones)
    if (p.startsWith('/api/phones/') && req.method === 'DELETE') {
      state.phones = state.phones.filter((x) => x.id !== decodeURIComponent(p.slice(12)))
      return json({ ok: true })
    }
    if (p === '/api/companions') {
      if (state.companionsFail > 0) {
        state.companionsFail--
        return json({ error: 'The hub is busy' }, 500)
      }
      return json(state.companions)
    }
    if (p === '/api/companions/pair') {
      if (!state.pairFresh) return json({ error: 'Sign in again to pair an app' }, 403)
      if (state.pairFails) return json({ error: 'The hub is busy' }, 500)
      return json({ code: 'K7PM3X', expiresAt: Date.now() + state.pairTtl })
    }
    if (p.startsWith('/api/companions/') && req.method === 'DELETE') {
      state.companions = state.companions.filter((x) => x.id !== decodeURIComponent(p.slice(16)))
      return json({ ok: true })
    }
    if (p === '/api/companion') {
      if (req.method === 'POST') {
        state.companion = { agent: body.agent, host: 'local', pane: '%30' }
        state.hosts[0]!.sessions.push(companionPane(body.agent))
        setTimeout(publish, 50)
      } else if (req.method === 'DELETE') {
        state.companion = { agent: null, host: 'local', pane: null }
        state.hosts[0]!.sessions = state.hosts[0]!.sessions.filter((s: Entry) => s.name !== 'companion')
        setTimeout(publish, state.reportLag)
      }
      return json(state.companion)
    }
    const host = /^\/api\/hosts\/([^/]+)\/(.*)$/.exec(p)
    if (host) {
      const rest = host[2]!
      if (rest === 'uploads') return json({ dir: '/home/sam/.inyon/uploads', files: 12, bytes: 18_400_000, days: 7 })
      if (rest === 'temp') {
        if (state.tempFails) return json({ error: 'The machine is busy' }, 500)
        // the listing as it is now, answered after `lag`
        const now = json(structuredClone(state.temp))
        const lag = state.tempLag.shift() ?? 0
        return lag ? new Promise<Response>((r) => setTimeout(() => r(now), lag)) : now
      }
      if (rest === 'temp/files') {
        const dir = url.searchParams.get('path')!
        return json({ files: [
          { path: `${dir}/shot-1.png`, rel: 'shot-1.png', size: 210_000, modified: Date.now() - 12 * 60_000, kind: 'image' },
          { path: `${dir}/notes.md`, rel: 'notes.md', size: 1_200, modified: Date.now() - 40 * 60_000, kind: 'text' },
        ], truncated: false })
      }
      if (rest === 'temp/preview') {
        const file = url.searchParams.get('path')!
        return file.endsWith('.png') ? json({ mime: 'image/png', size: 210_000, base64: png }) : json({ mime: 'text/markdown', size: 1_200, text: '# Notes\n\nThe checkout test waits on the cart.' })
      }
      if (rest === 'temp/delete') {
        const gone: string[] = body.paths.filter((x: string) => !state.tempKeep.includes(x))
        const failed = body.paths.filter((x: string) => state.tempKeep.includes(x)).map((x: string) => [x, 'Permission denied'])
        const items = state.temp.items.filter((i) => gone.includes(i.path))
        state.temp.items = state.temp.items.filter((i) => !gone.includes(i.path))
        return json({ removed: gone, freed: items.reduce((n, i) => n + i.size, 0), failed })
      }
      const end = /^panes\/(\d+)\/end$/.exec(rest)
      if (end && state.endFails) return json({ error: 'tmux is busy' }, 500)
      if (end && state.endGone) {
        state.endGone = false
        for (const h of state.hosts) h.sessions = h.sessions.filter((s: Entry) => !s.windows.some((w: Entry) => w.panes.some((x: Entry) => x.id === `%${end[1]}`)))
        publish()
        await Bun.sleep(300)
        return json({ error: 'No pane %' + end[1] }, 404)
      }
      if (end && state.endLag) {
        const lag = state.endLag
        state.endLag = 0
        await Bun.sleep(lag)
      }
      if (end) {
        // as tmux does: a window without panes closes, and a session without windows
        for (const h of state.hosts) {
          for (const s of h.sessions) {
            for (const w of s.windows) w.panes = w.panes.filter((x: Entry) => x.id !== `%${end[1]}`)
            s.windows = s.windows.filter((w: Entry) => w.panes.length)
          }
          h.sessions = h.sessions.filter((s: Entry) => s.windows.length)
        }
        setTimeout(publish, 50)
        return json({ ok: true })
      }
    }
    if (p === '/theme.js' || p.startsWith('/agents/') || p === '/icon.svg' || p.endsWith('.png')) {
      const f = Bun.file(path.join(web, p))
      return (await f.exists()) ? new Response(f) : new Response('Not found', { status: 404 })
    }
    if (p === '/next' || p.startsWith('/next/')) {
      const rel = p.replace(/^\/next\/?/, '')
      const f = Bun.file(path.join(dist, rel))
      if (rel && (await f.exists())) return new Response(f, { headers: { 'content-security-policy': NEXT_CSP } })
      return new Response(Bun.file(path.join(dist, 'index.html')), { headers: { 'content-type': 'text/html;charset=utf-8', 'content-security-policy': NEXT_CSP } })
    }
    if (withReact) {
      const f = Bun.file(path.join(react, p))
      if (p !== '/' && (await f.exists())) return new Response(f)
      return new Response(Bun.file(path.join(react, 'index.html')), { headers: { 'content-type': 'text/html;charset=utf-8' } })
    }
    return new Response('Not found', { status: 404 })
  },
  websocket: {
    open(ws) {
      sockets.add(ws)
      if (ws.data.v1) ws.send(JSON.stringify({ t: 'overview', hosts: state.hosts }))
    },
    close(ws) {
      sockets.delete(ws)
    },
    message(ws, raw) {
      const msg = JSON.parse(String(raw))
      if (ws.data.v1) return
      if (msg.t === 'hello') ws.send(JSON.stringify({ t: 'hello', v: 2 }))
      if (msg.t === 'sub' && msg.stream === 'overview') {
        ws.data.subs.add('overview')
        ws.send(frame())
      }
      if (msg.t === 'ping') ws.send(JSON.stringify({ t: 'pong' }))
    },
  },
})
const base = `http://127.0.0.1:${server.port}`

const problems: string[] = []
const check = (ok: unknown, what: string) => {
  if (!ok) problems.push(what)
}

/** Text smaller than 12px, or a page wider than its window. */
async function layout(page: Page, label: string) {
  const small: string[] = await page.evaluate(() => {
    const out: string[] = []
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const el = n.parentElement
      if (!el || !n.textContent?.trim() || !el.getClientRects().length) continue
      const size = parseFloat(getComputedStyle(el).fontSize)
      if (size < 12) out.push(`${n.textContent.trim().slice(0, 30)} (${size}px)`)
    }
    return out
  })
  check(!small.length, `${label}: text under 12px: ${small.join(', ')}`)
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
  check(wide <= 0, `${label}: scrolls sideways by ${wide}px`)
}

async function until(ok: () => boolean | Promise<boolean>, ms = 6000) {
  const end = Date.now() + ms
  while (!(await ok()) && Date.now() < end) await Bun.sleep(50)
  return ok()
}

/** The app's stylesheet is in: Dioxus adds it as the app starts, so the first screen can come before it. */
const styled = (page: Page) => page.waitForFunction(() => getComputedStyle(document.querySelector('.shell') ?? document.body).display === 'flex')
const toast = (page: Page, title: string) => page.locator('.toast', { hasText: title }).first()
const menuItems = (page: Page) => page.getByRole('menu').locator('.menu-item').allTextContents()
/** The whole of a page that scrolls inside `.scroll`, a window's height at a time: name-1.png, name-2.png… */
async function tall(page: Page, file: string) {
  const { height } = page.viewportSize()!
  const total = await page.evaluate(() => document.querySelector('.page .scroll')?.scrollHeight ?? 0)
  for (let i = 0, top = 0; top < total && i < 12; i++, top += height - 80) {
    await page.evaluate((y) => document.querySelector('.page .scroll')?.scrollTo(0, y), top)
    await Bun.sleep(150)
    await page.screenshot({ path: path.join(shots, file.replace('.png', `-${i + 1}.png`)) })
  }
  await page.evaluate(() => document.querySelector('.page .scroll')?.scrollTo(0, 0))
}
const saved = (page: Page) => page.evaluate(() => JSON.parse(localStorage.getItem('inyon.settings.v2') ?? '{}'))

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const errors: string[] = []
// every page opened, so a failure can show what each one had on screen
const opened: Page[] = []
const watch = (page: Page) => {
  opened.push(page)
  page.on('pageerror', (e) => errors.push(e.message))
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))
}
const phone = { ...devices['iPhone 13'], viewport: { width: 390, height: 844 }, reducedMotion: 'reduce' as const }
const desktop = { viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' as const }
/** A context that has seen the intro this session, so it doesn't cover what's checked. */
async function context(opts: Parameters<typeof browser.newContext>[0]): Promise<BrowserContext> {
  const ctx = await browser.newContext(opts)
  await ctx.addInitScript(() => {
    try {
      sessionStorage.setItem('inyon.intro.seen', '1')
    } catch {}
  })
  return ctx
}

try {
  // ---- a browser whose language isn't a valid locale tag (Chromium under LANG=C says en-US@posix) ----
  {
    const ctx = await context({ ...desktop, locale: 'en-US@posix' })
    const page = await ctx.newPage()
    const thrown: string[] = []
    page.on('pageerror', (e) => thrown.push(e.message))
    await page.goto(`${base}/next/settings`)
    await page.getByText('Update available: 0.25.1').waitFor()
    await page.getByText('Pixel 9').waitFor()
    check(!thrown.length, `posix locale: the page threw (${thrown.join(' | ')})`)
    const line = (await page.locator('.set-device', { hasText: 'Pixel 9' }).textContent()) ?? ''
    check(/paired \S*\d/.test(line), `posix locale: the phone has its paired date (${line})`)
    await ctx.close()
  }

  // ---- Settings at both sizes, in both themes ----
  for (const [size, opts] of [['390', phone], ['1440', desktop]] as const) {
    for (const scheme of ['light', 'dark'] as const) {
      const ctx = await context({ ...opts, colorScheme: scheme })
      const page = await ctx.newPage()
      watch(page)
      await page.goto(`${base}/next/settings`)
      await page.getByRole('heading', { name: 'Settings', exact: true }).waitFor()
      await styled(page)
      await page.getByText('Update available: 0.25.1').waitFor()
      await page.getByText('Legato on desk').waitFor()
      const label = `settings ${size} ${scheme}`
      for (const h of ['Appearance', 'Agents', 'Companion', 'Live numbers', 'Uploads and clipboard', 'Notifications', 'Machines', 'Your other devices', 'Paired devices', 'Account', 'Updates', 'About']) {
        check(await page.getByRole('heading', { name: h, exact: true }).isVisible(), `${label}: the ${h} section`)
      }
      check((await page.locator('.settings-nav').isVisible()) === (size === '1440'), `${label}: the side list only on a wide screen`)
      check(await page.getByRole('radio', { name: '1 hour' }).getAttribute('aria-checked') === 'true', `${label}: the sleep time from the hub`)
      check((await page.locator('.tab-active, .nav-active').first().textContent())?.includes('Settings'), `${label}: Settings lit in the nav`)
      await tall(page, `settings-${size}-${scheme}.png`)
      await layout(page, label)
      await ctx.close()
    }
  }

  // ---- what the settings do (desktop, light) ----
  {
    const ctx = await context({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.addInitScript(() => localStorage.setItem('inyon.settings.v2', JSON.stringify({ theme: 'system', sidePanelWidth: 420 })))
    // reads that fail say so, keep what was there, and try again
    state.phonesFail = true
    state.settingsReadFails = 1
    await page.goto(`${base}/next/settings`)
    await styled(page)
    await page.getByText("Couldn't read the hub's settings: No settings here").waitFor()
    await page.getByText("Couldn't read the paired phones: The hub is busy").waitFor()
    check(!(await page.getByText('No phones paired.').count()), 'phones: a failed read is not "no phones"')
    state.phonesFail = false
    await page.locator('.stale-note', { hasText: 'paired phones' }).getByRole('button', { name: 'Try again' }).click()
    await page.locator('.set-device', { hasText: 'Pixel 9' }).waitFor()
    await page.locator('.stale-note', { hasText: "hub's settings" }).getByRole('button', { name: 'Try again' }).click()
    await page.locator('.stale-note').first().waitFor({ state: 'detached' })
    await page.getByText('Update available: 0.25.1').waitFor()

    // the theme turns at once, and is kept where the web app keeps it, with the keys this app doesn't know
    await page.getByRole('radiogroup', { name: 'Theme' }).getByRole('radio', { name: 'Dark' }).click()
    check(await until(() => page.evaluate(() => document.documentElement.dataset.theme === 'dark')), 'theme: dark at once')
    const s1 = await saved(page)
    check(s1.theme === 'dark' && s1.sidePanelWidth === 420, `theme: saved with the other keys (${JSON.stringify(s1)})`)
    await page.getByRole('radiogroup', { name: 'Theme' }).getByRole('radio', { name: 'Auto' }).click()
    await page.getByRole('switch', { name: 'Finished notices' }).click()
    check((await saved(page)).notifyDone === false, 'notices: finished notices off')
    await page.getByRole('switch', { name: 'Finished notices' }).click()

    // the hub's settings go back whole, with what this app doesn't read
    await page.getByRole('radio', { name: '30 days' }).click()
    check(await until(() => called('PUT', '/api/settings').length > 0), 'uploads: saved to the hub')
    const put = called('PUT', '/api/settings').at(-1)?.body
    check(put?.uploads?.retentionDays === 30 && put?.uploads?.maxMB === 100 && put?.uploads?.keepMe === 1 && put?.futureKey === 'kept', `uploads: the whole settings back (${JSON.stringify(put)})`)
    await page.getByRole('switch', { name: 'Machines can message agents' }).click()
    check(await until(() => state.settings.machinesSend === true), 'machines: inyon send turned on')
    // off and on again quickly, the first save taken slowly: the hub ends up with the last
    const send = page.getByRole('switch', { name: 'Machines can message agents' })
    state.settingsLag = [1200]
    await send.click()
    await send.click()
    await Bun.sleep(2000)
    check(state.settings.machinesSend === true && (await send.getAttribute('aria-checked')) === 'true', `settings: saved in order (${state.settings.machinesSend})`)
    // a save that fails shows what the hub has again
    state.settingsFail = true
    await send.click()
    await toast(page, 'Could not save').waitFor()
    check(await until(async () => (await send.getAttribute('aria-checked')) === 'true'), 'settings: a failed save goes back to the hub\'s value')
    state.settingsFail = false
    await page.getByRole('radio', { name: '30 min' }).click()
    check(await until(() => called('PUT', '/api/organize/sleep').at(-1)?.body?.minutes === 30), 'sleep: 30 minutes')
    // two quick choices, the first taken slowly: one at a time, and the last one stays
    state.sleepLag = [1200]
    await page.getByRole('radio', { name: '2 hours' }).click()
    await page.getByRole('radio', { name: 'Off' }).click()
    check(await page.getByRole('radio', { name: 'Off' }).getAttribute('aria-checked') === 'true', 'sleep: shows the latest choice at once')
    check(await until(() => called('PUT', '/api/organize/sleep').at(-1)?.body?.minutes === 120), 'sleep: the first goes out')
    check(!called('PUT', '/api/organize/sleep').some((c) => c.body?.minutes === 0), 'sleep: the second waits for the first')
    check(await until(() => state.sleep.length === 0, 4000), `sleep: the last choice is the one kept (${JSON.stringify(state.sleep)})`)
    await Bun.sleep(300)
    check(await page.getByRole('radio', { name: 'Off' }).getAttribute('aria-checked') === 'true', 'sleep: the row stays on the last choice')

    // custom agents: add, the menus, remove and Undo
    await page.getByRole('button', { name: 'Add an agent' }).click()
    const sheet = page.getByRole('dialog', { name: 'Add an agent' })
    await sheet.waitFor()
    await sheet.getByRole('button', { name: 'Add', exact: true }).click()
    check(await sheet.getByText('Give it a name and a command.').isVisible(), 'custom: asks for a name and a command')
    await sheet.getByLabel('Name').fill('Claude YOLO')
    await sheet.getByLabel('Command').fill('claude --dangerously-skip-permissions')
    await page.screenshot({ path: path.join(shots, 'custom-agent-1440-light.png') })
    await sheet.getByRole('button', { name: 'Add', exact: true }).click()
    await toast(page, 'Added Claude YOLO').waitFor()
    check(state.settings.customAgents?.[0]?.like === 'claude', 'custom: saved as Claude Code')
    const row = page.locator('.set-custom', { hasText: 'Claude YOLO' })
    await row.click({ button: 'right' })
    check((await menuItems(page)).join('|') === 'Edit|Remove', `custom: right-click menu (${await menuItems(page)})`)
    await page.keyboard.press('Escape')
    await row.getByRole('button', { name: 'Options for Claude YOLO' }).click()
    await page.getByRole('menuitem', { name: 'Edit' }).click()
    await page.getByRole('dialog', { name: 'Edit Claude YOLO' }).waitFor()
    await page.getByRole('dialog', { name: 'Edit Claude YOLO' }).getByRole('button', { name: 'Cancel' }).click()
    await row.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Remove' }).click()
    const removed = toast(page, 'Removed Claude YOLO')
    await removed.waitFor()
    check(await until(() => (state.settings.customAgents ?? []).length === 0), 'custom: removed at the hub')
    await removed.getByRole('button', { name: 'Undo' }).click()
    check(await until(() => (state.settings.customAgents ?? []).length === 1), 'custom: Undo puts it back')
    await row.waitFor()

    // two removes and a Machines change in a row, the first save taken slowly: nothing is lost
    state.settings.customAgents = [...state.settings.customAgents, { id: 'fast', name: 'Codex fast', command: 'codex --yolo', like: 'codex' }]
    await page.reload()
    await styled(page)
    const fast = page.locator('.set-custom', { hasText: 'Codex fast' })
    await fast.waitFor()
    state.settingsLag = [1200]
    await row.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Remove' }).click()
    await fast.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Remove' }).click()
    await send.click()
    check(
      await until(() => (state.settings.customAgents ?? []).length === 0 && state.settings.machinesSend === false, 6000),
      `settings: both removed and the Machines change kept (${JSON.stringify(state.settings.customAgents)}, ${state.settings.machinesSend})`,
    )
    check((await send.getAttribute('aria-checked')) === 'false', 'settings: the Machines switch stays as set')
    await send.click()
    check(await until(() => state.settings.machinesSend === true), 'settings: Machines back on')

    // the companion: start, its sidebar item, stop
    await page.getByRole('radio', { name: 'Codex' }).click()
    await page.locator('#companion').getByRole('button', { name: 'Start', exact: true }).click()
    await toast(page, 'Companion started').waitFor()
    check(called('POST', '/api/companion').at(-1)?.body?.agent === 'codex', 'companion: started with Codex')
    await page.locator('#companion').getByRole('button', { name: 'Open' }).waitFor()
    check(await page.locator('.sidebar').getByRole('link', { name: /Companion/ }).isVisible(), 'companion: in the sidebar')
    await page.screenshot({ path: path.join(shots, 'companion-1440-light.png') })
    // stopped is stopped at once, before the machine's next report drops the pane
    state.reportLag = 5000
    await page.locator('#companion').getByRole('button', { name: 'Stop' }).click()
    await toast(page, 'Companion stopped').waitFor()
    await page.locator('#companion').getByRole('button', { name: 'Start', exact: true }).waitFor({ timeout: 2000 })
    state.reportLag = 50
    // once the report has dropped it, a companion back in the same pane (started elsewhere) shows
    await page.locator('.sidebar').getByRole('link', { name: /Companion/ }).waitFor({ state: 'detached', timeout: 8000 })
    state.companion = { agent: 'codex', host: 'local', pane: '%30' }
    state.hosts[0]!.sessions.push(companionPane('codex'))
    publish()
    await page.locator('#companion').getByRole('button', { name: 'Open' }).waitFor({ timeout: 4000 })
    await page.locator('#companion').getByRole('button', { name: 'Stop' }).click()
    await page.locator('#companion').getByRole('button', { name: 'Start', exact: true }).waitFor()

    // a companion app's details, from its menu
    await page.locator('.set-device', { hasText: 'Legato on desk' }).getByRole('button', { name: 'Options for Legato on desk' }).click()
    await page.getByRole('menuitem', { name: 'What it can do' }).click()
    const details = page.getByRole('dialog', { name: 'Legato on desk' })
    await details.waitFor()
    check(await details.getByText('Interrupt, terminal, files, settings').isVisible(), 'companion app: what the hub refuses')
    check((await details.locator('.set-scope-row').count()) === 5, 'companion app: four scopes and the refusals')
    await details.getByRole('button', { name: 'Done' }).click()

    // pairing a companion app: a fresh sign-in, the code, then the app claims it
    await page.getByRole('button', { name: 'Pair a companion app' }).click()
    const pair = page.getByRole('dialog', { name: 'Pair a companion app' })
    await pair.getByText('Pairing an app needs a fresh sign-in').waitFor()
    await pair.getByLabel('Password').fill(PASSWORD)
    await pair.getByRole('button', { name: 'Continue' }).click()
    await pair.getByRole('img', { name: 'Pairing code K 7 P M 3 X' }).waitFor()
    check(/Works once · changes in 4:[0-5]\d|5:00/.test((await pair.locator('.pair-left').textContent()) ?? ''), 'pairing: the countdown')
    await page.screenshot({ path: path.join(shots, 'pair-companion-1440-light.png') })
    state.companions.push({ id: 'c2', name: 'Legato on laptop', app: 'Legato for macOS', scopes: ['overview', 'chat', 'answer', 'send'], contractVersion: 1, pairedAt: Date.now(), lastSeen: Date.now() })
    await pair.getByText('Legato on laptop is paired').waitFor({ timeout: 6000 })
    await pair.getByRole('button', { name: 'Done' }).click()
    await page.locator('.set-device', { hasText: 'Legato on laptop' }).waitFor()

    // the list read when the sheet opens fails: the apps already paired aren't taken for the new one
    state.companionsFail = 1
    await page.getByRole('button', { name: 'Pair a companion app' }).click()
    await pair.getByRole('img', { name: 'Pairing code K 7 P M 3 X' }).waitFor()
    await Bun.sleep(2600)
    check(!(await pair.getByText(/is paired/).isVisible()), 'pairing: a failed list read takes no old app for the new one')
    await pair.getByRole('button', { name: 'Done' }).click()
    await pair.waitFor({ state: 'detached' })

    // the code runs out and no new one can be made: an app that claimed it just before is still found
    state.pairTtl = 1500
    await page.getByRole('button', { name: 'Pair a companion app' }).click()
    await pair.getByRole('img', { name: 'Pairing code K 7 P M 3 X' }).waitFor()
    state.pairFails = true
    await pair.getByText('The hub is busy').waitFor({ timeout: 6000 })
    state.companions.push({ id: 'c3', name: 'Legato on tablet', app: 'Legato for iPad', scopes: ['overview', 'chat', 'answer', 'send'], contractVersion: 1, pairedAt: Date.now(), lastSeen: Date.now() })
    await pair.getByText('Legato on tablet is paired').waitFor({ timeout: 6000 })
    state.pairFails = false
    state.pairTtl = 5 * 60_000
    await page.keyboard.press('Escape')
    await pair.waitFor({ state: 'detached' })

    // a new password: the form's own checks, then the hub's
    await page.getByRole('button', { name: 'Change password' }).click()
    await page.getByPlaceholder('Current password').fill(PASSWORD)
    await page.getByPlaceholder('New password (8+ characters)').fill('short')
    await page.getByPlaceholder('New password again').fill('short')
    await page.locator('.set-password').getByRole('button', { name: 'Change password' }).click()
    check(await page.getByText('Use at least 8 characters.').isVisible(), 'password: too short')
    await page.getByPlaceholder('New password (8+ characters)').fill('battery staple')
    await page.getByPlaceholder('New password again').fill('battery staple')
    await page.locator('.set-password').getByRole('button', { name: 'Change password' }).click()
    await toast(page, 'Password changed').waitFor()

    // the hub update
    await page.getByRole('button', { name: 'Update', exact: true }).click()
    await page.getByText('Updating the hub…').waitFor()
    check(called('POST', '/api/hub/update').length === 1, 'update: started')
    // it ends with nothing to install, and no restart: the row says so
    state.update = { current: VERSION, canUpdate: true, running: false }
    check(await until(() => page.getByText('Up to date').isVisible(), 7000), 'update: up to date when there was nothing to install')
    check(!(await page.getByText('Updating the hub…').count()), 'update: no longer updating')

    // a notice from the hub is a toast
    hub({ t: 'notify', host: 'build-box', pane: '%13', kind: 'done', title: 'Claude Code finished', body: 'Release notes are ready' })
    await toast(page, 'Claude Code finished').waitFor()

    // the hub restarts on new code: "Inyon was updated", once
    state.commit = 'bbb2222'
    state.update = { current: VERSION, canUpdate: true, running: false }
    for (const ws of sockets) ws.close()
    const prompt = page.getByText('Inyon was updated')
    await prompt.waitFor({ timeout: 15000 })
    check(await until(() => page.getByText('Up to date').isVisible(), 8000), 'update: up to date after the restart')
    await page.screenshot({ path: path.join(shots, 'update-prompt-1440-light.png') })
    await page.getByRole('button', { name: 'Not now' }).click()
    await prompt.waitFor({ state: 'detached' })
    await page.reload()
    await styled(page)
    await page.getByText('Up to date').waitFor()
    await Bun.sleep(800)
    check(!(await prompt.isVisible()), 'update: offered once per build')

    // sign out goes back to the form
    // a sign-out the hub didn't take stays signed in, or a reload would sign back in
    state.logoutFails = true
    await page.getByRole('button', { name: 'Sign out' }).click()
    await toast(page, 'Could not sign out').waitFor()
    check(await page.getByRole('button', { name: 'Sign out' }).isVisible(), 'account: still signed in after a failed sign-out')
    state.logoutFails = false
    await page.getByRole('button', { name: 'Sign out' }).click()
    await page.getByRole('button', { name: /Sign in|Continue/ }).first().waitFor()
    check(called('POST', '/api/logout').length === 2, 'account: signed out at the hub')
    await ctx.close()
  }

  // ---- notifications while Inyon is closed: the browser's push service, faked ----
  {
    const ctx = await context({ ...desktop, colorScheme: 'light' })
    await ctx.addInitScript(() => {
      // allowed already (the headless browser turns notifications down whatever it's granted)
      ;(window as any).Notification = class {
        static permission = 'granted'
        static requestPermission = async () => 'granted'
        close() {}
      }
      // a push service that hands out one subscription for whatever key it's given
      let current: any = null
      const reg = {
        pushManager: {
          getSubscription: async () => current,
          subscribe: async (o: { applicationServerKey: Uint8Array }) => {
            const key = o.applicationServerKey.slice().buffer
            current = { endpoint: 'https://push.example/sub1', options: { applicationServerKey: key }, toJSON: () => ({ endpoint: 'https://push.example/sub1', keys: { p256dh: 'p', auth: 'a' } }), unsubscribe: async () => ((current = null), true) }
            return current
          },
        },
      }
      Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { register: async () => reg, ready: Promise.resolve(reg), addEventListener() {} } })
      ;(window as any).PushManager = function () {}
    })
    const page = await ctx.newPage()
    watch(page)
    const pushes = () => called('POST', '/api/push')
    const before = pushes().length
    await page.goto(`${base}/next/settings`)
    await styled(page)
    check(await until(() => pushes().length > before, 5000), 'push: the subscription goes to the hub at start')
    const body = pushes().at(-1)?.body
    check(body?.subscription?.endpoint === 'https://push.example/sub1' && body?.vapid?.subject === 'https://inyon.tech/' && !!body?.vapid?.privateJwk?.d && body?.done === true, `push: what the hub gets ${JSON.stringify(body)}`)
    check(!!(await page.evaluate(() => localStorage.getItem('inyon.push-vapid'))), 'push: the key is kept where the web app keeps it')
    await page.getByText('On for this device, also while Inyon is closed').waitFor()
    await page.getByRole('button', { name: 'Send a test' }).click()
    await toast(page, 'Test sent').waitFor()
    check(called('POST', '/api/push/test').at(-1)?.body?.endpoint === 'https://push.example/sub1', 'push: the test goes through the push service')
    // a switch change goes to the hub, with the same subscription and key
    const n = pushes().length
    await page.getByRole('switch', { name: 'Finished notices' }).click()
    check(await until(() => pushes().length > n && pushes().at(-1)?.body?.done === false), 'push: finished notices off at the hub too')
    check(pushes().at(-1)?.body?.vapid?.privateJwk?.d === body?.vapid?.privateJwk?.d, 'push: the same key every time')
    await page.getByRole('switch', { name: 'Finished notices' }).click()
    // About says how the terminal draws (the page, until a terminal opens)
    const drawing = page.locator('.set-row', { hasText: 'Terminal drawing' })
    check((await drawing.textContent())?.includes('Page'), `about: terminal drawing (${await drawing.textContent()})`)
    await ctx.close()
  }

  // ---- the companion on the Overview, on a phone ----
  {
    const ctx = await context({ ...phone, colorScheme: 'dark' })
    const page = await ctx.newPage()
    watch(page)
    state.companion = { agent: 'claude', host: 'local', pane: '%30' }
    state.hosts[0]!.sessions.push(companionPane('claude'))
    await page.goto(`${base}/next/`)
    await styled(page)
    const line = page.locator('.companion-line')
    await line.waitFor()
    check((await line.textContent())?.includes('Ask about your sessions'), 'overview: the companion line')
    check(!(await page.locator('.card', { hasText: 'Companion' }).count()), 'overview: the companion is not a card')
    await page.screenshot({ path: path.join(shots, 'companion-overview-390-dark.png') })
    await line.click()
    await page.waitForURL(/\/next\/m\/local\/p\/30$/)
    state.hosts[0]!.sessions = state.hosts[0]!.sessions.filter((s: Entry) => s.name !== 'companion')
    state.companion = { agent: null, host: 'local', pane: null }
    await ctx.close()
  }

  // ---- ending a session, from its menu ----
  {
    const ctx = await context({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines/build-box`)
    await styled(page)
    const row = page.locator('.pane-row', { hasText: 'Tidy the billing API types' })
    await row.waitFor()
    await row.click({ button: 'right' })
    check((await menuItems(page)).at(-1) === 'End session…', `end: the session's menu (${await menuItems(page)})`)
    await page.getByRole('menuitem', { name: 'End session…' }).click()
    const ask = page.getByRole('dialog', { name: 'End this session?' })
    await ask.waitFor()
    check((await ask.textContent())?.includes('OpenCode exits, then its tmux pane closes'), 'end: says what goes')
    await page.screenshot({ path: path.join(shots, 'end-session-1440-light.png') })
    await ask.getByRole('button', { name: 'End session' }).click()
    await toast(page, 'Session ended').waitFor()
    check(called('POST', '/api/hosts/build-box/panes/14/end').length === 1, 'end: asked the hub')
    await row.waitFor({ state: 'detached' })

    // from the session's own screen: a failure says so and stays, a success goes to the Overview
    state.endFails = true
    await page.goto(`${base}/next/m/build-box/p/13`)
    await styled(page)
    await page.getByRole('button', { name: 'More' }).click()
    await page.getByRole('menuitem', { name: 'End session…' }).click()
    await ask.getByRole('button', { name: 'End session' }).click()
    await toast(page, 'Could not end the session').waitFor()
    check(!(await toast(page, 'Session ended').count()), "end: no \"Session ended\" for one that wasn't")
    check(/\/m\/build-box\/p\/13$/.test(page.url()), `end: a failure stays on the session (${page.url()})`)
    await page.getByRole('button', { name: 'More' }).waitFor()
    state.endFails = false
    await page.getByRole('button', { name: 'More' }).click()
    await page.getByRole('menuitem', { name: 'End session…' }).click()
    await ask.getByRole('button', { name: 'End session' }).click()
    await toast(page, 'Session ended').waitFor()
    await page.waitForURL(/\/next\/?$/)

    // gone somewhere else while the machine ends it: stays there
    await page.goto(`${base}/next/m/local/p/12`)
    await styled(page)
    state.endLag = 1500
    await page.getByRole('button', { name: 'More' }).click()
    await page.getByRole('menuitem', { name: 'End session…' }).click()
    await ask.getByRole('button', { name: 'End session' }).click()
    await page.locator('.sidebar').getByRole('link', { name: 'Pages' }).click()
    await page.waitForURL(/\/next\/pages/)
    await toast(page, 'Session ended').waitFor()
    await Bun.sleep(300)
    check(/\/next\/pages/.test(page.url()), `end: stays where you went meanwhile (${page.url()})`)
    state.endLag = 0

    // the pane closed by itself while the question was open: it has ended all the same
    await page.goto(`${base}/next/m/local/p/11`)
    await styled(page)
    state.endGone = true
    await page.getByRole('button', { name: 'More' }).click()
    await page.getByRole('menuitem', { name: 'End session…' }).click()
    await ask.getByRole('button', { name: 'End session' }).click()
    await toast(page, 'Session ended').waitFor()
    check(!(await toast(page, 'Could not end the session').count()), 'end: no failure for a pane already gone')
    await ctx.close()
  }

  // ---- Temp files ----
  for (const [size, opts] of [['390', phone], ['1440', desktop]] as const) {
    for (const scheme of ['light', 'dark'] as const) {
      state.temp = temp(Date.now())
      const ctx = await context({ ...opts, colorScheme: scheme })
      const page = await ctx.newPage()
      watch(page)
      await page.goto(`${base}/next/temp/local`)
      await styled(page)
      await page.getByRole('heading', { name: 'Temp files' }).waitFor()
      await page.locator('.temp-row').nth(2).waitFor()
      const label = `temp ${size} ${scheme}`
      check((await page.locator('.temp-group-head h2').allTextContents()).join('|') === 'Claude Code scratch|Uploads', `${label}: folders with something in them`)
      check(await page.locator('.temp-row').first().getByText('recent').isVisible(), `${label}: recent marked`)
      await tall(page, `temp-${size}-${scheme}.png`)
      await layout(page, label)
      if (size === '1440' && scheme === 'light') {
        await page.getByRole('button', { name: 'Older than a week' }).click()
        await page.getByText('2 selected').waitFor()
        await page.getByRole('button', { name: 'Clear selection' }).click()
        await page.getByRole('checkbox', { name: 'Select accounts' }).click()
        // one that can't be deleted stays picked, for another try
        state.tempKeep = ['/tmp/claude-1000/accounts']
        await page.getByRole('region', { name: 'Selection' }).getByRole('button', { name: 'Delete' }).click()
        const ask = page.getByRole('dialog', { name: 'Delete accounts?' })
        await ask.waitFor()
        await ask.getByRole('button', { name: 'Delete' }).click()
        await toast(page, "Couldn't delete accounts").waitFor()
        check(await page.getByText('1 selected').isVisible(), 'temp: a failed delete keeps the selection')
        state.tempKeep = []
        await page.getByRole('region', { name: 'Selection' }).getByRole('button', { name: 'Delete' }).click()
        await ask.waitFor()
        await ask.getByRole('button', { name: 'Delete' }).click()
        await toast(page, 'Freed').waitFor()
        check(called('POST', '/api/hosts/local/temp/delete').at(-1)?.body?.paths?.[0] === '/tmp/claude-1000/accounts', 'temp: deleted the one picked')
        await page.locator('.temp-row', { hasText: 'accounts' }).waitFor({ state: 'detached' })
        // a folder's files, and a file's preview
        await page.locator('.temp-row', { hasText: 'storefront' }).locator('.temp-open').click()
        const files = page.getByRole('dialog', { name: 'storefront' })
        await files.getByText('notes.md').waitFor()
        await files.locator('.temp-file', { hasText: 'notes.md' }).click()
        const preview = page.getByRole('dialog', { name: 'notes.md' })
        await preview.getByText('The checkout test waits on the cart.').waitFor()
        await page.screenshot({ path: path.join(shots, 'temp-preview-1440-light.png') })
        // the row's menu
        await page.keyboard.press('Escape')
        await page.keyboard.press('Escape')
        await page.locator('.temp-row', { hasText: 'storefront' }).click({ button: 'right' })
        check((await menuItems(page)).join('|') === 'Open|Select|Copy path|Delete…', `temp: the row's menu (${await menuItems(page)})`)
        await page.keyboard.press('Escape')
        // a refresh that fails keeps the list and says so
        state.tempFails = true
        await page.getByRole('button', { name: 'Refresh' }).click()
        await page.getByText("Couldn't refresh the list: The machine is busy").waitFor()
        check(await page.locator('.temp-row', { hasText: 'storefront' }).isVisible(), 'temp: the list stays after a failed refresh')
        state.tempFails = false
        // two refreshes, the first answering last: the newer listing stays
        state.tempLag = [1500]
        await page.getByRole('button', { name: 'Refresh' }).click()
        state.temp.items = state.temp.items.filter((i) => i.name !== 'storefront')
        await page.getByRole('button', { name: 'Refresh' }).click()
        await page.locator('.temp-row', { hasText: 'storefront' }).waitFor({ state: 'detached' })
        await Bun.sleep(1800)
        check(!(await page.locator('.temp-row', { hasText: 'storefront' }).count()), 'temp: a late answer to an earlier refresh is dropped')
        check(!(await page.getByText("Couldn't refresh the list").count()), 'temp: a good refresh clears the failure')
        // and over an empty list too
        state.temp.items = []
        await page.getByRole('button', { name: 'Refresh' }).click()
        await page.getByText('Nothing here').waitFor()
        state.tempFails = true
        await page.getByRole('button', { name: 'Refresh' }).click()
        await page.getByText("Couldn't refresh the list: The machine is busy").waitFor()
        state.tempFails = false
      }
      await ctx.close()
    }
  }

  // ---- the cold-start intro: once per tab session, and it hands over to the header's mark ----
  {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/`)
    const intro = page.getByTestId('intro')
    await intro.waitFor()
    check(await page.evaluate(() => document.documentElement.classList.contains('intro-running')), 'intro: the header mark waits')
    // its own size from the first paint, stylesheet or not
    const width = await page.locator('.intro-mark').evaluate((e) => e.getBoundingClientRect().width)
    check(width <= 161, `intro: the mark is 160px, not ${width}px`)
    await Bun.sleep(250)
    await page.screenshot({ path: path.join(shots, 'intro-1440-light.png') })
    await intro.waitFor({ state: 'detached', timeout: 5000 })
    check(!(await page.evaluate(() => document.documentElement.classList.contains('intro-running'))), 'intro: done, the header mark shows')
    await page.reload()
    await styled(page)
    await Bun.sleep(300)
    check(!(await intro.count()), 'intro: not again in the same session')
    await ctx.close()
  }

  // ---- the web app's screens with the same data, to compare ----
  if (withReact) {
    for (const [size, opts] of [['390', phone], ['1440', desktop]] as const) {
      for (const scheme of ['light', 'dark'] as const) {
        state.temp = temp(Date.now())
        const ctx = await context({ ...opts, colorScheme: scheme })
        const page = await ctx.newPage()
        await page.goto(`${base}/settings`)
        await page.getByText('Agent marks belong to their owners').waitFor({ timeout: 15000 }).catch(() => {})
        await Bun.sleep(800)
        await page.screenshot({ path: path.join(shots, `react-settings-${size}-${scheme}.png`), fullPage: true })
        await page.goto(`${base}/temp/local`)
        await Bun.sleep(1500)
        await page.screenshot({ path: path.join(shots, `react-temp-${size}-${scheme}.png`), fullPage: true })
        await ctx.close()
      }
    }
  }
} catch (e) {
  // what the open pages showed, and what they logged, for a failure on a machine you can't look at
  for (const [i, page] of opened.filter((p) => !p.isClosed()).entries()) {
    await page.screenshot({ path: path.join(shots, `failed-${i + 1}.png`) }).catch(() => {})
  }
  if (errors.length) console.error(`errors in the page: ${errors.join(' | ')}`)
  throw e
} finally {
  await browser.close()
  server.stop(true)
}

// a socket the test closes on purpose is logged by the browser
// the pairing code's 403 asks for a fresh sign-in, on purpose
const unexpected = errors.filter((e) => !/WebSocket|status of 404|status of 403|status of 500|status of 503/.test(e))
check(!unexpected.length, `errors in the page: ${unexpected.join(' | ')}`)
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`ok: Settings at 390x844 and 1440x900 in light and dark, app and hub settings, custom agents, the companion and pairing an app, the password, the hub update and "Inyon was updated", notices and Web Push, ending a session, Temp files and the intro; screenshots in ${shots}`)
