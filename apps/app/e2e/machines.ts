// bun apps/app/e2e/machines.ts — the Machines pages of the built app (apps/app/dist, from build.sh)
// in Chromium behind a stand-in hub: the page under the hub's own CSP for /next, the /ws2 overview
// and the machine endpoints, fed made-up data (demo.ts). Checks the list and a machine's page at
// phone and desktop sizes in light and dark, the right-click and long-press menus, adding a machine
// both ways, a new session, every tab (setup, config, uploads, integration and its sign-in),
// updating and removing a machine, the phone code, and Can't connect?, and saves a screenshot of
// each size and theme to $SHOTS (default: a temp folder). CHROME=/path/to/chrome uses a browser other
// than Playwright's own.
import { mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium, devices, type Page } from 'playwright'
import type { ServerWebSocket } from 'bun'
import { overview } from './demo.ts'

const root = path.join(import.meta.dir, '../../..')
const dist = path.join(import.meta.dir, '../dist')
const web = path.join(root, 'apps/web/public')
const shots = process.env.SHOTS ?? path.join(tmpdir(), 'inyon-app-shots')
mkdirSync(shots, { recursive: true })

// the policy the hub sends with the page, read from the hub's source so the two can't drift
const rs = await Bun.file(path.join(root, 'crates/inyon-hub/src/static_files.rs')).text()
const NEXT_CSP = /pub const NEXT_CSP: &str =\s*"([^"]+)"/.exec(rs)![1]!

const PASSWORD = 'correct horse'

// the demo's two machines, plus a server the hub reaches over SSH that is down right now, and the
// laptop a version behind
type Host = Record<string, any>
function machines(): Host[] {
  const now = Date.now()
  const [studio, laptop] = overview(now).hosts as Host[]
  laptop!.nodeUpdate = { latest: '0.23.2', supported: true }
  laptop!.signedIn = ['claude']
  laptop!.config.pairedAt = now - 86_400_000
  const rig: Host = {
    config: { id: 'gpu-rig', name: 'gpu-rig', kind: 'ssh', target: 'sam@gpu-rig.lan', port: 2222 },
    status: 'error',
    error: 'ssh: connect to host gpu-rig.lan port 2222: Connection refused',
    integration: 'unknown',
    tmux: 'ok',
    agents: [],
    sessions: [],
    updatedAt: now,
    lastSeen: now - 3 * 3600_000,
  }
  return [studio!, laptop!, rig]
}

const state = {
  authed: true,
  hosts: machines(),
  calls: [] as { method: string; path: string; body?: unknown }[],
  // the phone code needs a fresh sign-in once
  phoneFresh: false,
  config: '{\n  "model": "sonnet"\n}\n',
  signinPolls: 0,
}
type Socket = ServerWebSocket<{ subs: Set<string> }>
const sockets = new Set<Socket>()
let seq = 1
const EPOCH = 1
const snapshot = () => JSON.stringify({ t: 'snapshot', stream: 'overview', epoch: EPOCH, seq, rows: { hosts: state.hosts } })
/** The hub's machines changed: every socket gets the new overview. */
function publish() {
  seq++
  for (const ws of sockets) if (ws.data.subs.has('overview')) ws.send(snapshot())
}
const called = (method: string, p: string) => state.calls.filter((c) => c.method === method && c.path === p)

const setup = {
  os: 'Ubuntu 24.04',
  pkg: 'apt',
  sudo: false,
  checks: [
    { id: 'tmux', label: 'tmux 3.4', ok: true, detail: '/usr/bin/tmux' },
    { id: 'git', label: 'git', ok: false, detail: 'Not found', manual: 'sudo apt install -y git' },
  ],
  agents: [
    { kind: 'claude', installed: true, version: '2.1.296 (Claude Code)', signedIn: true, latest: '2.1.296', outdated: false },
    { kind: 'codex', installed: true, version: 'codex-cli 0.49.0', signedIn: false, latest: '0.50.1', outdated: true },
    { kind: 'opencode', installed: false },
  ],
  keepSession: true,
}

const server = Bun.serve({
  port: 0,
  async fetch(req, srv) {
    const url = new URL(req.url)
    const p = url.pathname
    const json = (body: unknown, status = 200) => Response.json(body, { status })
    if (p === '/ws2') {
      if (!state.authed) return new Response('Not signed in', { status: 401 })
      return srv.upgrade(req, { data: { subs: new Set<string>() } }) ? undefined : new Response('no upgrade', { status: 400 })
    }
    if (p === '/api/session') return json({ authed: state.authed, setup: false, setupCode: false })
    if (p.startsWith('/api/')) {
      const body = req.method === 'GET' || req.method === 'DELETE' ? undefined : await req.json().catch(() => undefined)
      state.calls.push({ method: req.method, path: p + url.search, body })
    }
    if (p === '/api/login' && req.method === 'POST') {
      if ((state.calls.at(-1)!.body as { password?: string }).password !== PASSWORD) return json({ error: 'Wrong password' }, 401)
      state.phoneFresh = true
      return json({ ok: true })
    }
    if (p === '/api/settings') return json({ customAgents: [{ id: 'reviewer', name: 'Reviewer', command: 'claude --agent reviewer', like: 'claude' }] })
    if (p === '/api/phones/pair' && req.method === 'POST') {
      if (!state.phoneFresh) return json({ error: 'Sign in again to pair a phone' }, 403)
      return json({ id: 'K7Q4MX2T', code: '493812', link: `inyon://hub?u=${encodeURIComponent(base)}&c=K7Q4MX2T%3A493812&h=3f9c1a7e2b6d4c80`, expiresAt: Date.now() + 10 * 60_000 })
    }
    if (p === '/api/hosts/pair' && req.method === 'POST') {
      const id = 'node-new'
      state.hosts.push({ config: { id, name: 'Work laptop', kind: 'node' }, status: 'connecting', integration: 'unknown', tmux: 'ok', agents: [], sessions: [], updatedAt: Date.now() })
      publish()
      // the machine runs the command a moment later
      setTimeout(() => {
        const h = state.hosts.find((x) => x.config.id === id)
        if (h) Object.assign(h, { status: 'online', config: { ...h.config, pairedAt: Date.now() } })
        publish()
      }, 1200)
      return json({ id, command: `curl -fsSL ${base}/join/abc | sh`, commands: { unix: `curl -fsSL ${base}/join/abc | sh`, windows: `irm ${base}/join/abc.ps1 | iex` } })
    }
    if (p === '/api/hosts' && req.method === 'POST') return json({ ok: true })
    const m = /^\/api\/hosts\/([^/]+)(\/.*)?$/.exec(p)
    if (m) {
      const id = decodeURIComponent(m[1]!)
      const rest = m[2] ?? ''
      if (rest === '' && req.method === 'DELETE') {
        state.hosts = state.hosts.filter((h) => h.config.id !== id)
        publish()
        return json({ ok: true })
      }
      if (rest === '' && req.method === 'PATCH') return json({ ok: true })
      if (rest === '/node/update') {
        const h = state.hosts.find((x) => x.config.id === id)!
        h.nodeUpdate = { ...h.nodeUpdate, state: 'updating' }
        publish()
        return json({ ok: true })
      }
      if (rest === '/launch') return json({ pane: '%31' })
      if (rest === '/setup' && req.method === 'GET') return json(setup)
      if (rest === '/setup') return json({ ok: true, version: '0.50.1' })
      if (rest === '/configs') {
        return json([
          { agent: 'claude', label: 'settings.json', path: '/home/sam/.claude/settings.json', format: 'json', exists: true, size: 412, mtime: Date.now() / 1000 - 3600 },
          { agent: 'codex', label: 'config.toml', path: '/home/sam/.codex/config.toml', format: 'toml', exists: false, size: 0 },
        ])
      }
      if (rest === '/config' && req.method === 'GET') return json({ content: url.searchParams.get('path')?.endsWith('settings.json') ? state.config : '' })
      if (rest === '/config' && req.method === 'PUT') return json({ ok: true })
      if (rest === '/uploads' && req.method === 'GET') return json({ dir: '/home/sam/.inyon/uploads', files: 14, bytes: 5_400_000, days: 7, expired: 3 })
      if (rest === '/uploads/clean') return json({ dir: '/home/sam/.inyon/uploads', files: 11, bytes: 4_100_000, days: 7, expired: 0 })
      if (rest === '/integration') {
        const h = state.hosts.find((x) => x.config.id === id)!
        h.integration = (state.calls.at(-1)!.body as { enable: boolean }).enable ? 'installed' : 'missing'
        publish()
        return json({ ok: true })
      }
      if (rest === '/signin' && req.method === 'POST') return json({ pane: '%40' })
      if (rest === '/signin/40' && req.method === 'GET') {
        state.signinPolls++
        const sent = called('POST', p).length > 0
        if (sent) return json({ lines: ['Signed in.'], links: [], exit: 0 })
        return json({ lines: ['Open this link to sign in:', 'https://auth.example/device'], links: ['https://auth.example/device'] })
      }
      if (rest === '/signin/40') return json({ ok: true })
    }
    if (p === '/theme.js' || p.startsWith('/agents/') || p === '/icon.svg') return new Response(Bun.file(path.join(web, p)))
    if (p === '/next' || p.startsWith('/next/')) {
      const rel = p.replace(/^\/next\/?/, '')
      const f = Bun.file(path.join(dist, rel))
      if (rel && (await f.exists())) return new Response(f, { headers: { 'content-security-policy': NEXT_CSP } })
      return new Response(Bun.file(path.join(dist, 'index.html')), { headers: { 'content-type': 'text/html;charset=utf-8', 'content-security-policy': NEXT_CSP } })
    }
    return new Response('Not found', { status: 404 })
  },
  websocket: {
    open(ws) {
      sockets.add(ws)
    },
    close(ws) {
      sockets.delete(ws)
    },
    message(ws, raw) {
      const msg = JSON.parse(String(raw))
      if (msg.t === 'hello') ws.send(JSON.stringify({ t: 'hello', v: 2 }))
      if (msg.t === 'sub' && msg.stream === 'overview') {
        ws.data.subs.add('overview')
        ws.send(snapshot())
      } else if (msg.t === 'sub') {
        ws.send(JSON.stringify({ t: 'snapshot', stream: msg.stream, epoch: EPOCH, seq: 1, rows: { items: [] }, state: { loaded: true, hasMore: false } }))
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

/** Waits until the stand-in hub has had `n` calls to a path. */
async function calls(method: string, p: string, n = 1, ms = 6000) {
  const end = Date.now() + ms
  while (called(method, p).length < n && Date.now() < end) await Bun.sleep(50)
  return called(method, p)
}

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
// the cold-start intro has its own check (settings.ts); here it would cover the first screen
const newContext = browser.newContext.bind(browser)
browser.newContext = async (o) => {
  const ctx = await newContext(o)
  await ctx.addInitScript(() => {
    try {
      sessionStorage.setItem('inyon.intro.seen', '1')
    } catch {}
  })
  return ctx
}
const errors: string[] = []
const watch = (page: Page) => {
  page.on('pageerror', (e) => errors.push(e.message))
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))
}
const phone = { ...devices['iPhone 13'], viewport: { width: 390, height: 844 }, reducedMotion: 'reduce' as const }
const desktop = { viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' as const }

try {
  // ---- the list and a machine's page, at both sizes in both themes ----
  for (const [size, opts] of [['390', phone], ['1440', desktop]] as const) {
    for (const scheme of ['light', 'dark'] as const) {
      const ctx = await browser.newContext({ ...opts, colorScheme: scheme })
      const page = await ctx.newPage()
      watch(page)
      await page.goto(`${base}/next/machines`)
      await page.getByRole('heading', { name: 'Machines' }).waitFor()
      await page.locator('.machine-item').nth(2).waitFor()
      check((await page.locator('.machine-item').count()) === 3, `list ${size} ${scheme}: three machines`)
      // online first, the one that's down last, its line saying why
      check((await page.locator('.machine-item').last().textContent())?.includes('gpu-rig'), `list ${size} ${scheme}: the offline machine last`)
      check(await page.getByText('Update available').isVisible(), `list ${size} ${scheme}: the laptop's update`)
      await page.screenshot({ path: path.join(shots, `machines-${size}-${scheme}.png`) })
      await layout(page, `machines ${size} ${scheme}`)
      await page.locator('.machine-item').first().click()
      await page.getByRole('radio', { name: 'Sessions' }).waitFor()
      check(page.url().endsWith('/next/machines/local'), `page ${size} ${scheme}: the address is the machine's (${page.url()})`)
      check(await page.getByText('This hub · Linux · responds in 38 ms').isVisible(), `page ${size} ${scheme}: the line under the title`)
      check((await page.locator('.pane-row').count()) === 2, `page ${size} ${scheme}: its two sessions`)
      await page.screenshot({ path: path.join(shots, `machine-${size}-${scheme}.png`) })
      await layout(page, `machine ${size} ${scheme}`)
      await ctx.close()
    }
  }

  // ---- menus: right click, and a long press on a touch screen ----
  {
    const ctx = await browser.newContext({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines`)
    const laptop = page.locator('.machine-item', { hasText: 'build-box' })
    await laptop.click({ button: 'right' })
    const items = await page.getByRole('menuitem').allTextContents()
    check(JSON.stringify(items) === JSON.stringify(['Open', 'New session', 'Open in new tab', 'Temp files', 'Rename…', 'Update to inyon 0.23.2', 'Remove…']), `menu: ${JSON.stringify(items)}`)
    await page.screenshot({ path: path.join(shots, 'machines-menu-1440-light.png') })
    await page.getByRole('menuitem', { name: 'Update to inyon 0.23.2' }).click()
    check((await calls('POST', '/api/hosts/build-box/node/update')).length === 1, 'menu: Update asks the hub')
    await page.getByText('Updating…').waitFor()
    // the menu opens with the keyboard's focus in it; Esc closes it
    await laptop.click({ button: 'right' })
    await page.keyboard.press('ArrowDown')
    check((await page.evaluate(() => document.activeElement?.textContent)) === 'New session', 'menu: ↓ moves to the next item')
    await page.keyboard.press('Escape')
    check((await page.getByRole('menu').count()) === 0, 'menu: Esc closes it')
    // Rename… from the menu opens the sheet for a machine that isn't on screen
    await laptop.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Rename…' }).click()
    await page.getByRole('dialog', { name: 'Edit build-box' }).waitFor()
    await page.getByLabel('Name').fill('Sam’s laptop')
    await page.getByRole('button', { name: 'Save' }).click()
    const patch = await calls('PATCH', '/api/hosts/build-box')
    check(JSON.stringify(patch[0]?.body) === JSON.stringify({ name: 'Sam’s laptop' }), `rename: ${JSON.stringify(patch[0]?.body)}`)
    await page.getByText('Machine updated').waitFor()
    await ctx.close()
  }
  {
    const ctx = await browser.newContext({ ...phone, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines`)
    const row = page.locator('.machine-item').first()
    await page.locator('.machine-item', { hasText: 'gpu-rig' }).waitFor()
    await page.evaluate(() => document.fonts.ready)
    // the list settles (fonts, the connection line) before the finger goes down on it
    let box = (await row.boundingBox())!
    for (let i = 0; i < 20; i++) {
      await Bun.sleep(100)
      const next = (await row.boundingBox())!
      if (next.y === box.y && next.height === box.height) break
      box = next
    }
    const cdp = await ctx.newCDPSession(page)
    const at = [{ x: box.x + 60, y: box.y + box.height / 2 }]
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: at })
    await Bun.sleep(700)
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
    // the lift's click lands on the menu's backdrop and must not close it
    await Bun.sleep(150)
    await page.getByRole('menu').waitFor({ timeout: 3000 }).catch(() => {})
    check(await page.getByRole('menuitem', { name: 'New session' }).isVisible(), 'long press: the menu opens')
    check(page.url().endsWith('/next/machines'), `long press: the row isn't opened as well (${page.url()})`)
    await page.screenshot({ path: path.join(shots, 'machines-menu-390-light.png') })
    // a later tap outside it closes it
    await Bun.sleep(700)
    await page.touchscreen.tap(20, 450)
    await page.getByRole('menu').waitFor({ state: 'detached', timeout: 3000 }).catch(() => {})
    check((await page.getByRole('menu').count()) === 0, 'long press: a tap outside closes the menu')
    await ctx.close()
  }

  // ---- adding a machine: a pairing command, and over SSH ----
  {
    const ctx = await browser.newContext({ ...desktop, colorScheme: 'dark' })
    await ctx.grantPermissions(['clipboard-read', 'clipboard-write'])
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines?add=1`)
    const sheet = page.getByRole('dialog', { name: 'Add a machine' })
    await sheet.waitFor()
    await sheet.getByLabel('Name').fill('Work laptop')
    await sheet.getByRole('button', { name: 'Create pairing command' }).click()
    await sheet.getByText('curl -fsSL').waitFor()
    check(JSON.stringify((await calls('POST', '/api/hosts/pair'))[0]?.body) === JSON.stringify({ name: 'Work laptop', origin: base }), `pair: ${JSON.stringify(called('POST', '/api/hosts/pair')[0]?.body)}`)
    await sheet.getByRole('radio', { name: 'Windows' }).click()
    check(await sheet.getByText('irm ').isVisible(), 'pair: the Windows command')
    await sheet.getByRole('button', { name: 'Copy command' }).click()
    await page.getByText('Command copied').waitFor()
    check((await page.evaluate(() => navigator.clipboard.readText())).startsWith('irm '), 'pair: the command is on the clipboard')
    await sheet.getByText('Work laptop is connected.').waitFor({ timeout: 8000 })
    await page.screenshot({ path: path.join(shots, 'add-pair-1440-dark.png') })
    await sheet.getByRole('button', { name: 'Done' }).click()
    await sheet.waitFor({ state: 'hidden' })
    check(page.url().endsWith('/next/machines'), `pair: closing drops ?add=1 (${page.url()})`)

    await page.getByRole('button', { name: 'Add machine' }).click()
    await sheet.getByRole('radio', { name: 'Reach it over SSH' }).click()
    await sheet.getByLabel('Name').fill('Work server')
    await sheet.getByLabel('SSH destination').fill('deploy@work-server')
    await sheet.getByLabel('Port').fill('22x22')
    check((await sheet.getByLabel('Port').inputValue()) === '2222', 'ssh: the port takes digits only')
    await page.screenshot({ path: path.join(shots, 'add-ssh-1440-dark.png') })
    await sheet.getByRole('button', { name: 'Add machine' }).click()
    const add = await calls('POST', '/api/hosts')
    check(JSON.stringify(add[0]?.body) === JSON.stringify({ name: 'Work server', target: 'deploy@work-server', port: 2222 }), `ssh: ${JSON.stringify(add[0]?.body)}`)
    await page.getByText('Connecting over SSH…').waitFor()
    await ctx.close()
  }
  state.hosts = machines()
  publish()

  // ---- a machine's tabs, a new session, removing ----
  {
    const ctx = await browser.newContext({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines/build-box`)
    await page.getByText('inyon 0.23.1 · update available').waitFor()
    await page.getByRole('button', { name: 'Update', exact: true }).click()
    await page.getByText('Updating inyon…').waitFor()

    // Setup
    await page.getByRole('radio', { name: 'Setup' }).click()
    await page.getByText('Ubuntu 24.04 · apt · no passwordless sudo').waitFor()
    check(await page.getByText('sudo apt install -y git').isVisible(), 'setup: the command to run by hand')
    check(await page.getByText('0.49.0 · not signed in').isVisible(), 'setup: an agent behind and signed out')
    await page.screenshot({ path: path.join(shots, 'machine-setup-1440-light.png') })
    await page.getByRole('button', { name: 'Update to 0.50.1' }).click()
    await page.getByText('Codex updated to 0.50.1').waitFor()
    check(JSON.stringify((await calls('POST', '/api/hosts/build-box/setup'))[0]?.body) === JSON.stringify({ action: 'update:codex' }), 'setup: Update runs update:codex')
    await page.getByRole('switch', { name: 'Keep a tmux session running' }).click()
    check(JSON.stringify((await calls('POST', '/api/hosts/build-box/setup', 2))[1]?.body) === JSON.stringify({ keepSession: false }), 'setup: the switch')

    // Config: JSON checked as you type, saved with PUT
    await page.getByRole('radio', { name: 'Config' }).click()
    const text = page.locator('.config-text')
    await text.waitFor()
    await page.getByText('Valid JSON').waitFor()
    await text.fill('{ "model": ')
    await page.getByText(/^Invalid JSON/).waitFor()
    check(await page.getByRole('button', { name: 'Save' }).isDisabled(), 'config: broken JSON doesn’t save')
    await text.fill('{ "model": "opus" }')
    await page.getByText('Unsaved changes').waitFor()
    await page.getByRole('button', { name: 'Save' }).click()
    await page.getByText('Saved', { exact: true }).first().waitFor()
    const put = await calls('PUT', '/api/hosts/build-box/config')
    check(JSON.stringify(put[0]?.body) === JSON.stringify({ path: '/home/sam/.claude/settings.json', content: '{ "model": "opus" }' }), `config: ${JSON.stringify(put[0]?.body)}`)
    await page.screenshot({ path: path.join(shots, 'machine-config-1440-light.png') })

    // Uploads
    await page.getByRole('radio', { name: 'Uploads' }).click()
    await page.getByText('5.1 MB · 14 files').waitFor()
    await page.getByRole('button', { name: 'Clean older than 7 days (3)' }).click()
    await page.getByText('Uploads cleaned').waitFor()
    check(JSON.stringify((await calls('POST', '/api/hosts/build-box/uploads/clean'))[0]?.body) === JSON.stringify({ olderThanDays: 7 }), 'uploads: clean older than the keep time')

    // Integration, and signing an agent in from there
    await page.getByRole('radio', { name: 'Integration' }).click()
    await page.getByRole('button', { name: 'Turn off' }).click()
    await page.getByRole('button', { name: 'Turn on' }).waitFor()
    check(JSON.stringify((await calls('POST', '/api/hosts/build-box/integration'))[0]?.body) === JSON.stringify({ enable: false }), 'integration: turned off')
    await page.locator('.agent-pill', { hasText: 'Codex' }).getByRole('button', { name: 'Sign in' }).click()
    const sign = page.getByRole('dialog', { name: 'Sign in to Codex' })
    await sign.getByRole('link', { name: 'Open the sign-in page' }).waitFor()
    await page.screenshot({ path: path.join(shots, 'signin-1440-light.png') })
    await sign.getByLabel('Code from the sign-in page').fill('ABCD-1234')
    await sign.getByRole('button', { name: 'Sign in' }).click()
    await sign.getByText('Codex is signed in on build-box.').waitFor({ timeout: 8000 })
    check(JSON.stringify((await calls('POST', '/api/hosts/build-box/signin/40'))[0]?.body) === JSON.stringify({ text: 'ABCD-1234' }), 'sign-in: the code goes to the agent')
    await sign.getByRole('button', { name: 'Done' }).click()

    // New session, with one of your own agents
    await page.getByRole('button', { name: 'New session' }).click()
    const ns = page.getByRole('dialog', { name: 'New session' })
    await ns.getByRole('radio', { name: /Reviewer/ }).click()
    check(await ns.getByRole('button', { name: 'Start Reviewer' }).isVisible(), 'new session: the button names the agent')
    // a click beside an open list closes it, and picking closes it too (a list inside a <label>
    // opened again on either)
    await ns.getByRole('button', { name: 'Machine' }).click()
    await page.getByRole('listbox', { name: 'Machine' }).waitFor()
    await page.mouse.click(900, 152)
    await Bun.sleep(300)
    check((await page.getByRole('listbox').count()) === 0, 'new session: a click beside the list closes it')
    await ns.getByRole('button', { name: 'Machine' }).click()
    await page.getByRole('option', { name: /studio/ }).click()
    await Bun.sleep(300)
    check((await page.getByRole('listbox').count()) === 0, 'new session: picking a machine closes the list')
    await ns.getByRole('button', { name: /docs-site/ }).waitFor({ state: 'detached' }).catch(() => {})
    await page.screenshot({ path: path.join(shots, 'new-session-1440-light.png') })
    await ns.getByRole('button', { name: 'Start Reviewer' }).click()
    const launch = await calls('POST', '/api/hosts/local/launch')
    check(JSON.stringify(launch[0]?.body) === JSON.stringify({ agent: 'claude', custom: 'reviewer', cwd: '~' }), `new session: ${JSON.stringify(launch[0]?.body)}`)
    await page.waitForURL(/\/next\/m\/local\/p\/31$/)

    // Remove, asked first
    await page.goto(`${base}/next/machines/gpu-rig`)
    await page.getByText('ssh sam@gpu-rig.lan -p 2222').waitFor()
    await page.getByRole('button', { name: 'Remove' }).click()
    const ask = page.getByRole('dialog', { name: 'Remove gpu-rig?' })
    await ask.waitFor()
    await page.screenshot({ path: path.join(shots, 'remove-1440-light.png') })
    await ask.getByRole('button', { name: 'Remove' }).click()
    await page.waitForURL(/\/next\/machines$/)
    check(called('DELETE', '/api/hosts/gpu-rig').length === 1, 'remove: the hub is asked')
    await ctx.close()
  }
  state.hosts = machines()
  publish()

  // ---- Can't connect?, and the phone's code ----
  {
    const ctx = await browser.newContext({ ...phone, colorScheme: 'dark' })
    await ctx.grantPermissions(['clipboard-read', 'clipboard-write'])
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines/gpu-rig`)
    await page.getByRole('button', { name: "Can't connect?" }).click()
    const help = page.getByRole('dialog', { name: "Can't connect?" })
    await help.getByText('Hub reaches it').waitFor()
    await help.locator('.check[data-state="fail"]').waitFor({ timeout: 15_000 })
    const states = await help.locator('.check').evaluateAll((els) => els.map((e) => `${e.querySelector('.check-title')?.textContent}:${e.getAttribute('data-state')}`))
    check(
      JSON.stringify(states) === JSON.stringify(['Network:pass', 'Address:pass', 'Hub answers:pass', 'Right address:pass', 'Signed in:pass', 'Live connection:pass', 'Hub reaches it:fail']),
      `connect: ${JSON.stringify(states)}`,
    )
    check(await help.locator('.check-fix', { hasText: "Check the hub's machine can ssh to sam@gpu-rig.lan without a password prompt." }).isVisible(), 'connect: what to do about it')
    await page.screenshot({ path: path.join(shots, 'connect-390-dark.png') })
    await help.getByRole('button', { name: 'Copy report' }).click()
    await page.getByText('Report copied').waitFor()
    const report = await page.evaluate(() => navigator.clipboard.readText())
    check(report.startsWith('Inyon connection report') && report.includes('[FAIL] Hub reaches it'), `connect: the report (${report.slice(0, 80)})`)
    await help.getByRole('button', { name: 'Close' }).click()
    await ctx.close()
  }
  {
    const ctx = await browser.newContext({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines`)
    await page.getByRole('button', { name: 'Phone' }).click()
    const sheet = page.getByRole('dialog', { name: 'Open on your phone' })
    await sheet.getByLabel('Password').fill(PASSWORD)
    await sheet.getByRole('button', { name: 'Continue' }).click()
    await sheet.getByRole('img', { name: 'Pairing code' }).waitFor()
    check(await sheet.getByText('K7Q4-MX2T').isVisible(), 'phone: the id, grouped')
    check(await sheet.getByText('493 812').isVisible(), 'phone: the code, grouped')
    check(await sheet.getByText('3f9c 1a7e 2b6d 4c80').isVisible(), 'phone: the hub id to compare')
    await page.screenshot({ path: path.join(shots, 'phone-1440-light.png') })
    await layout(page, 'phone sheet')
    await ctx.close()
  }
} finally {
  await browser.close()
  server.stop(true)
}

// the long press and the dialogs close sockets on purpose
const unexpected = errors.filter((e) => !/401|Unauthorized|WebSocket|403/.test(e))
check(!unexpected.length, `errors in the page: ${unexpected.join(' | ')}`)
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`ok: the Machines list and page at 390x844 and 1440x900 in light and dark, menus by right click and long press, adding by command and SSH, renaming, every tab, sign-in, new session, update, remove, Can't connect? and the phone code; screenshots in ${shots}`)
