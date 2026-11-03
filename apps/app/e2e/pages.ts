// bun apps/app/e2e/pages.ts — Pages in the built app (apps/app/dist, from build.sh) in Chromium behind
// a stand-in hub: the page under the hub's own CSP for /next, the /ws2 overview and pages streams, the
// page endpoints and made-up sites under /v/<token>/. Checks the list and the viewer at phone and
// desktop sizes in light and dark, the sandboxed frame and its theme, live progress, the changed
// marks and badges, the right-click, long-press and ⋯ menus, parts, notes and device sizes, revoking,
// removing with Undo, adding a folder, the Folder sheet, a machine's pages, a session's pages and the
// pages a reply links to, and saves a screenshot of each size and theme to $SHOTS (default: a temp
// folder). CHROME=/path/to/chrome uses a browser other than Playwright's own.
import { mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium, devices, type Page } from 'playwright'
import type { ServerWebSocket } from 'bun'
import { info, overview } from './demo.ts'

const root = path.join(import.meta.dir, '../../..')
const dist = path.join(import.meta.dir, '../dist')
const web = path.join(root, 'apps/web/public')
const shots = process.env.SHOTS ?? path.join(tmpdir(), 'inyon-app-shots')
mkdirSync(shots, { recursive: true })

// the policy the hub sends with the page, read from the hub's source so the two can't drift
const rs = await Bun.file(path.join(root, 'crates/inyon-hub/src/static_files.rs')).text()
const NEXT_CSP = /pub const NEXT_CSP: &str =\s*"([^"]+)"/.exec(rs)![1]!

type Entry = Record<string, any>
const HOUR = 3600_000
function pages(now: number): Entry[] {
  return [
    {
      host: 'local', slug: 'checkout-redesign', name: 'Checkout redesign', project: '/home/sam/code/storefront', by: 'claude', added: now - 26 * HOUR,
      folder: '/home/sam/code/storefront/private/plans/checkout-redesign', notes: ['Field notes.md', 'Competitors.md'],
      progress: { done: 3, total: 8, now: 'Wiring the payment step to the new cart', updated: new Date(now - 4 * 60_000).toISOString() },
    },
    { host: 'local', slug: 'passkeys', name: 'Passkeys rollout', project: '/home/sam/code/accounts', by: 'codex', added: now - 3 * HOUR, folder: '/home/sam/code/accounts/private/plans/passkeys' },
    {
      host: 'build-box', slug: 'release-notes', name: 'Release notes site', project: '/home/sam/code/docs-site', by: 'cli', added: now - 50 * HOUR, folder: '/home/sam/code/docs-site/private/plans/release-notes',
      progress: { done: 6, total: 6, updated: new Date(now - 2 * HOUR).toISOString() },
    },
  ]
}

const state = {
  hosts: overview(Date.now()).hosts as Entry[],
  pages: pages(Date.now()),
  calls: [] as { method: string; path: string; body?: unknown }[],
  minted: 0,
}
type Socket = ServerWebSocket<{ subs: Set<string> }>
const sockets = new Set<Socket>()
let seq = 1
const EPOCH = 1
const frame = (stream: string) =>
  JSON.stringify({ t: 'snapshot', stream, epoch: EPOCH, seq, rows: stream === 'overview' ? { hosts: state.hosts } : { pages: state.pages } })
/** The pages changed: every socket following them hears it. */
function publishPages() {
  seq++
  for (const ws of sockets) if (ws.data.subs.has('pages')) ws.send(frame('pages'))
}
const called = (method: string, p: string) => state.calls.filter((c) => c.method === method && c.path === p)

/** A reply that links to a page, as `inyon pages add` prints it. */
function chat(now: number) {
  const ts = (ago: number) => new Date(now - ago * 1000).toISOString()
  return [
    { kind: 'user', seq: 1, uuid: 'u1', ts: ts(300), text: 'Make a plan page for the checkout redesign.', images: 0 },
    { kind: 'assistant', seq: 2, uuid: 'u2', ts: ts(200), msgId: 'm2', block: { type: 'text', text: 'The plan is up. Open it in the app at /pages/local/checkout-redesign' } },
    { kind: 'turn_end', seq: 3, uuid: 'u3', ts: ts(199), durationMs: 41000 },
  ]
}

/** A made-up site: it starts in the theme its address asks for, asks the app for it, and keeps the answer. */
const site = (title: string) => `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title>
<style>body{font:16px system-ui;margin:32px;background:#fff;color:#111}[data-theme=dark] body{background:#111;color:#eee}</style></head>
<body><h1>${title}</h1><p id="theme"></p><script>
const show = (t) => { document.documentElement.dataset.theme = t; document.getElementById('theme').textContent = 'theme ' + t }
show(new URLSearchParams(location.search).get('theme') || 'light')
addEventListener('message', (e) => { if (e.data && e.data.type === 'theme') { show(e.data.value); document.body.dataset.answered = '1' } })
parent.postMessage({ type: 'theme?' }, '*')
</script></body></html>`

const server = Bun.serve({
  port: 0,
  async fetch(req, srv) {
    const url = new URL(req.url)
    const p = url.pathname
    const json = (body: unknown, status = 200) => Response.json(body, { status })
    if (p === '/ws2') return srv.upgrade(req, { data: { subs: new Set<string>() } }) ? undefined : new Response('no upgrade', { status: 400 })
    if (p === '/api/session') return json({ authed: true, setup: false, setupCode: false })
    if (p.startsWith('/api/')) {
      const body = req.method === 'GET' || req.method === 'DELETE' ? undefined : await req.json().catch(() => undefined)
      state.calls.push({ method: req.method, path: p + url.search, body })
    }
    if (p === '/api/settings') return json({})
    const m = /^\/api\/pages\/([^/]+)(?:\/([^/]+))?(\/open|\/revoke)?$/.exec(p)
    if (m) {
      const host = decodeURIComponent(m[1]!)
      const slug = m[2] && decodeURIComponent(m[2])
      if (!slug && req.method === 'POST') {
        const b = state.calls.at(-1)!.body as { path: string; name?: string; slug?: string }
        if (!b.path.endsWith('/plan') && !b.slug) return json({ error: 'No index.html in that folder' }, 400)
        const added = { host, slug: b.slug ?? 'new-plan', name: b.name ?? 'plan', project: '/home/sam/code/new', by: 'app', added: Date.now(), folder: b.path }
        state.pages.push(added)
        setTimeout(publishPages, 100)
        return json(added)
      }
      const page = state.pages.find((x) => x.host === host && x.slug === slug)
      if (!page) return json({ error: 'No such page' }, 404)
      if (m[3] === '/open') {
        state.minted++
        return json({ url: `/v/tok-${slug}-${state.minted}/`, expires: Date.now() + 12 * HOUR, ttl: 12 * HOUR })
      }
      if (m[3] === '/revoke') return json({ ok: true })
      if (req.method === 'DELETE') {
        state.pages = state.pages.filter((x) => x !== page)
        setTimeout(publishPages, 100)
        return json({ ok: true })
      }
    }
    const v = /^\/v\/tok-([a-z0-9-]+?)-\d+\/(.*)$/.exec(p)
    if (v) {
      const [slug, rest] = [v[1]!, v[2]!]
      const page = state.pages.find((x) => x.slug === slug)
      const html = (body: string) => new Response(req.method === 'HEAD' ? null : body, { headers: { 'content-type': 'text/html;charset=utf-8' } })
      if (!page) return new Response('gone', { status: 404 })
      if (rest === '') return html(site(page.name))
      if (rest === 'mockups/') return html(site(`${page.name}: mockups`))
      if (rest.startsWith('research/') && url.searchParams.get('as') === 'note') return html(site(`Note: ${decodeURIComponent(rest.slice(9))}`))
      return new Response('Not found', { status: 404 })
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
      if (msg.t === 'sub' && (msg.stream === 'overview' || msg.stream === 'pages')) {
        ws.data.subs.add(msg.stream)
        ws.send(frame(msg.stream))
      } else if (msg.t === 'sub') {
        const now = Date.now()
        ws.send(JSON.stringify({ t: 'snapshot', stream: msg.stream, epoch: EPOCH, seq: 1, rows: { items: chat(now) }, state: { info: info('local', '%11', 'claude', { title: 'Fix flaky checkout test' }), loaded: true, hasMore: false } }))
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

const menuItems = (page: Page) => page.getByRole('menu').locator('.menu-item').allTextContents()
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
const reset = () => {
  state.pages = pages(Date.now())
  publishPages()
}

try {
  // ---- phone: the list and the viewer, in both themes ----
  for (const scheme of ['light', 'dark'] as const) {
    reset()
    const ctx = await browser.newContext({ ...phone, colorScheme: scheme })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/pages`)
    await page.getByRole('heading', { name: 'Pages' }).waitFor()
    await page.locator('.page-card').nth(2).waitFor()
    await styled(page)
    const label = `phone ${scheme}`
    // the hub's machine first, then the others; inside a machine by project
    const headings = await page.locator('.machine-heading').allTextContents()
    check(headings[0]?.startsWith('studio') && headings[0]?.includes('hub') && headings[1]?.startsWith('build-box'), `${label}: machines in order (${headings.join(' | ')})`)
    check((await page.locator('.pages-project').allTextContents()).join(',') === 'accounts,storefront,docs-site', `${label}: grouped by project`)
    const card = page.locator('.page-card', { hasText: 'Checkout redesign' })
    check((await card.textContent())?.includes('3/8'), `${label}: the card's progress`)
    check(await card.getByText('Wiring the payment step to the new cart').isVisible(), `${label}: what it's working on`)
    check((await card.getAttribute('aria-label'))?.startsWith('Checkout redesign, changed, 3 of 8 done'), `${label}: the card's label`)
    check((await page.locator('.changed-dot').count()) === 3, `${label}: every page new here is marked`)
    check((await page.locator('.tab-badge-pages').textContent()) === '3', `${label}: the tab bar counts changed pages`)
    await page.screenshot({ path: path.join(shots, `pages-390-${scheme}.png`) })
    await layout(page, label)

    // live progress, from the stream
    state.pages[0]!.progress = { ...state.pages[0]!.progress, done: 4, updated: new Date().toISOString() }
    publishPages()
    check(await until(async () => (await card.textContent())?.includes('4/8') ?? false), `${label}: progress moves without a reload`)

    if (scheme === 'light') {
      // the ⋯ menu and a long press give the same items
      await card.getByRole('button', { name: 'More for Checkout redesign' }).click()
      check((await menuItems(page)).join('|').startsWith('Open|Open in browser'), `${label}: the ⋯ menu (${await menuItems(page)})`)
      check((await menuItems(page)).join('|').includes('Revoke links|Show folder|Remove'), `${label}: the ⋯ menu's other items`)
      await page.keyboard.press('Escape')
      await page.getByRole('menu').waitFor({ state: 'detached' })
      // Enter on ⋯ opens its menu and leaves the card where it is
      await card.getByRole('button', { name: 'More for Checkout redesign' }).focus()
      await page.keyboard.press('Enter')
      await page.getByRole('menu').waitFor()
      check(page.url().endsWith('/next/pages'), `${label}: Enter on ⋯ didn't open the page`)
      await page.keyboard.press('Escape')
      await page.getByRole('menu').waitFor({ state: 'detached' })
      const other = page.locator('.page-card', { hasText: 'Passkeys rollout' })
      const box = (await other.boundingBox())!
      const cdp = await ctx.newCDPSession(page)
      const pt = { x: box.x + 60, y: box.y + box.height - 12 }
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [pt] })
      await page.waitForTimeout(700)
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
      await page.getByRole('menu').waitFor()
      check((await menuItems(page))[0] === 'Open', `${label}: long press opens the menu`)
      check(page.url().endsWith('/next/pages'), `${label}: the long press didn't open the page`)
      await page.mouse.click(200, 120)
      await page.getByRole('menu').waitFor({ state: 'detached' })
    }

    // the viewer
    await card.click()
    await page.waitForURL(/\/next\/pages\/local\/checkout-redesign$/)
    const iframe = page.locator('iframe.page-frame')
    await iframe.waitFor()
    check(await page.getByText('Sandboxed page: never sign in here').isVisible(), `${label}: the sandbox line`)
    check((await iframe.getAttribute('sandbox')) === 'allow-scripts allow-forms allow-popups allow-downloads', `${label}: the frame's sandbox`)
    check((await iframe.getAttribute('referrerpolicy')) === 'no-referrer', `${label}: no referrer`)
    check(/^\/v\/tok-checkout-redesign-\d+\/\?theme=/.test((await iframe.getAttribute('src')) ?? ''), `${label}: the frame's address (${await iframe.getAttribute('src')})`)
    check(!(await page.locator('.tabbar').isVisible()), `${label}: the viewer has the screen to itself`)
    const inside = page.frameLocator('iframe.page-frame')
    await inside.getByRole('heading', { name: 'Checkout redesign' }).waitFor()
    check(await until(async () => (await inside.locator('body').getAttribute('data-answered')) === '1'), `${label}: the app answers the page's theme question`)
    check((await inside.locator('#theme').textContent()) === `theme ${scheme}`, `${label}: the page in the app's theme`)
    await page.screenshot({ path: path.join(shots, `pages-viewer-390-${scheme}.png`) })
    await layout(page, `${label} viewer`)
    // notes and parts from the header
    await page.getByRole('button', { name: 'Notes' }).click()
    await page.getByRole('menuitemradio', { name: 'Field notes' }).click()
    await inside.getByRole('heading', { name: 'Note: Field notes.md' }).waitFor()
    check(await page.locator('.note-line').getByText('Field notes').isVisible(), `${label}: the open note's line`)
    await page.getByRole('button', { name: 'Back to Plan' }).click()
    await inside.getByRole('heading', { name: 'Checkout redesign' }).waitFor()
    await page.getByRole('button', { name: 'More', exact: true }).click()
    const more = await menuItems(page)
    check(more.join('|') === 'Reload|Plan|Mockups|Open in browserA 12 h link. Never sign in there', `${label}: the More menu has the site's parts (${more.join('|')})`)
    await page.getByRole('menuitemradio', { name: 'Mockups' }).click()
    await inside.getByRole('heading', { name: 'Checkout redesign: mockups' }).waitFor()
    // back to the list: the page is no longer marked, here or in the badge, and that's kept
    await page.getByRole('button', { name: 'Back to Pages' }).click()
    await page.waitForURL(/\/next\/pages$/)
    await page.locator('.page-card').first().waitFor()
    check((await page.locator('.page-card', { hasText: 'Checkout redesign' }).locator('.changed-dot').count()) === 0, `${label}: an opened page loses its mark`)
    check((await page.locator('.tab-badge-pages').textContent()) === '2', `${label}: the badge counts one fewer`)
    const seen = await page.evaluate(() => JSON.parse(localStorage.getItem('inyon.pages.seen.v1') ?? '{}'))
    check(typeof seen['local/checkout-redesign'] === 'number', `${label}: the mark is kept with the web app's key`)
    await ctx.close()
  }

  // ---- desktop: the tree and the viewer, in both themes ----
  for (const scheme of ['light', 'dark'] as const) {
    reset()
    const ctx = await browser.newContext({ ...desktop, colorScheme: scheme })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/pages`)
    await styled(page)
    const label = `desktop ${scheme}`
    // nothing picked: the first changed page in the tree opens (the hub's machine, its projects by name)
    await page.waitForURL(/\/next\/pages\/local\/passkeys$/)
    await page.locator('.tree-row', { hasText: 'Checkout redesign' }).click()
    await page.waitForURL(/\/next\/pages\/local\/checkout-redesign$/)
    const inside = page.frameLocator('iframe.page-frame')
    await inside.getByRole('heading', { name: 'Checkout redesign' }).waitFor()
    check((await page.locator('.tree-row').count()) === 3, `${label}: three rows in the tree`)
    check((await page.locator('.tree-row-on').textContent())?.includes('Checkout redesign'), `${label}: the open page is lit`)
    check((await page.locator('.crumbs').textContent())?.replace(/\s+/g, '') === 'studio/storefront/Checkoutredesign', `${label}: where it is (${await page.locator('.crumbs').textContent()})`)
    // the site's parts: a plan and mockups, no matrix
    const parts = page.getByRole('radiogroup', { name: 'Part of the page' })
    await parts.waitFor()
    check((await parts.getByRole('radio').allTextContents()).join(',') === 'Plan,Mockups', `${label}: the parts it has`)
    check((await page.locator('.viewer-foot').textContent())?.includes('link lasts 11 h 59 m') || (await page.locator('.viewer-foot').textContent())?.includes('link lasts 12 h 0 m'), `${label}: how long the link lasts (${await page.locator('.viewer-foot').textContent()})`)
    await page.screenshot({ path: path.join(shots, `pages-1440-${scheme}.png`) })
    await layout(page, label)
    if (scheme === 'light') {
      await parts.getByRole('radio', { name: 'Mockups' }).click()
      await inside.getByRole('heading', { name: 'Checkout redesign: mockups' }).waitFor()
      // a device size shrinks the frame to fit
      await page.getByRole('radio', { name: 'Desktop 1440 × 900' }).click()
      check(await until(async () => /Desktop · 1440 × 900 · \d+%/.test((await page.locator('.viewer-size').textContent()) ?? '')), `${label}: the device size (${await page.locator('.viewer-size').textContent()})`)
      await page.getByRole('radio', { name: 'Phone 390 × 844' }).click()
      check(await until(async () => (await page.locator('.viewer-size').textContent())?.startsWith('Phone · 390 × 844') ?? false), `${label}: the phone size`)
      await page.screenshot({ path: path.join(shots, `pages-device-1440-${scheme}.png`) })
      await page.getByRole('radio', { name: 'Fit the pane' }).click()
      // notes
      await page.getByRole('button', { name: 'Notes' }).click()
      await page.getByRole('menuitemradio', { name: 'Competitors' }).click()
      await inside.getByRole('heading', { name: 'Note: Competitors.md' }).waitFor()
      check((await page.locator('iframe.page-frame').getAttribute('src'))?.includes('research/Competitors.md?as=note&theme=light'), `${label}: a note's address`)
      // Reload and clicking the open page again both mint a new link
      const before = state.minted
      await page.getByRole('button', { name: 'Reload' }).click()
      check(await until(() => state.minted === before + 1), `${label}: Reload mints a new link`)
      await page.locator('.tree-row-on').click()
      check(await until(() => state.minted === before + 2), `${label}: the open page clicked again mints a new link`)
      // "/" goes to the filter
      await page.locator('.crumbs').click()
      await page.keyboard.press('/')
      check(await page.evaluate(() => document.activeElement?.id === 'pages-filter'), `${label}: "/" focuses the filter`)
      await page.keyboard.type('release')
      check(await until(async () => (await page.locator('.tree-row').count()) === 1), `${label}: the filter narrows the tree`)
      await page.keyboard.press('Escape')
      check(await until(async () => (await page.locator('.tree-row').count()) === 3), `${label}: Esc clears the filter`)

      // revoke from the ⋯ menu, after asking
      await page.getByRole('button', { name: 'More for Checkout redesign' }).click()
      await page.getByRole('menuitem', { name: 'Revoke links' }).click()
      const ask = page.getByRole('dialog', { name: 'Revoke links to “Checkout redesign”?' })
      await ask.waitFor()
      const minted = state.minted
      await ask.getByRole('button', { name: 'Revoke links' }).click()
      check(await until(() => called('POST', '/api/pages/local/checkout-redesign/revoke').length === 1), `${label}: revoked`)
      check(await until(() => state.minted === minted + 1), `${label}: the open viewer gets a new link at once`)
      await page.getByText('Links revoked').waitFor()

      // the Folder sheet
      await page.locator('.tree-row', { hasText: 'Passkeys rollout' }).click({ button: 'right' })
      check((await menuItems(page)).join('|').startsWith('Open|Open in browser'), `${label}: a row's right-click menu`)
      await page.getByRole('menuitem', { name: 'Show folder' }).click()
      const folder = page.getByRole('dialog', { name: 'Folder' })
      await folder.waitFor()
      check(await folder.getByText('/home/sam/code/accounts/private/plans/passkeys').isVisible(), `${label}: the folder's path`)
      check(await folder.getByText('codex ·').isVisible(), `${label}: who added it`)
      await folder.getByRole('button', { name: 'Close' }).click()
      await folder.waitFor({ state: 'detached' })

      // remove, then Undo
      await page.locator('.tree-row', { hasText: 'Passkeys rollout' }).click({ button: 'right' })
      await page.getByRole('menuitem', { name: 'Remove' }).click()
      check(await until(async () => (await page.locator('.tree-row').count()) === 2), `${label}: removed from the tree`)
      check(called('DELETE', '/api/pages/local/passkeys').length === 1, `${label}: the hub was asked to remove it`)
      const toast = page.locator('.toast', { hasText: 'Removed “Passkeys rollout”' })
      await toast.waitFor()
      check(await toast.getByText('The folder stays on studio.').isVisible(), `${label}: the remove toast says the folder stays`)
      await toast.getByRole('button', { name: 'Undo' }).click()
      check(await until(() => called('POST', '/api/pages/local').length === 1), `${label}: Undo adds it back`)
      check(
        JSON.stringify(called('POST', '/api/pages/local')[0]?.body) === JSON.stringify({ path: '/home/sam/code/accounts/private/plans/passkeys', name: 'Passkeys rollout', slug: 'passkeys' }),
        `${label}: with its name and slug (${JSON.stringify(called('POST', '/api/pages/local')[0]?.body)})`,
      )
      check(await until(async () => (await page.locator('.tree-row').count()) === 3), `${label}: back in the tree`)

      // add a folder
      await page.getByRole('button', { name: 'Add a folder' }).first().click()
      const add = page.getByRole('dialog', { name: 'Add a folder' })
      await add.waitFor()
      await add.getByLabel('Folder').fill('./private/plans/plan')
      await add.getByRole('button', { name: 'Add', exact: true }).click()
      await add.getByText('Give the full path, starting with /.').waitFor()
      await add.getByLabel('Folder').fill('/home/sam/code/new/private/plans/plan')
      await add.getByLabel('Name').fill('New plan')
      await add.getByRole('button', { name: 'Add', exact: true }).click()
      await page.getByText('Added “New plan”').waitFor()
      check(await until(async () => (await page.locator('.tree-row').count()) === 4), `${label}: the new page is listed`)
      await layout(page, `${label} after the menus`)
    }
    await ctx.close()
  }

  // ---- the pages other screens list ----
  {
    reset()
    const ctx = await browser.newContext({ ...desktop, colorScheme: 'light' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/machines/local`)
    await styled(page)
    const section = page.locator('.machine-pages')
    await section.waitFor()
    check(await section.getByRole('heading', { name: 'Pages on this machine' }).isVisible(), 'machine: its pages section')
    check((await section.locator('.page-row').count()) === 2, 'machine: its two pages')
    await page.screenshot({ path: path.join(shots, 'machine-pages-1440-light.png'), fullPage: true })
    // a session: its project's pages behind a header button, and the pages its reply links to
    await page.goto(`${base}/next/m/local/p/11`)
    const reply = page.locator('.reply-pages')
    await reply.waitFor()
    check((await reply.locator('.page-row').textContent())?.includes('Checkout redesign'), 'chat: the page a reply links to')
    await page.getByRole('button', { name: 'Pages: 1' }).click()
    const sheet = page.getByRole('dialog', { name: 'Pages' })
    await sheet.waitFor()
    check((await sheet.locator('.page-row').count()) === 1, "session: the project's page")
    await page.screenshot({ path: path.join(shots, 'session-pages-1440-light.png') })
    await sheet.locator('.page-row').click()
    await page.waitForURL(/\/next\/pages\/local\/checkout-redesign$/)
    await ctx.close()
  }
} finally {
  await browser.close()
  server.stop(true)
}

// a site without a matrix answers its HEAD with a 404, which the browser logs
const unexpected = errors.filter((e) => !/WebSocket|status of 404/.test(e))
check(!unexpected.length, `errors in the page: ${unexpected.join(' | ')}`)
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`ok: Pages at 390x844 and 1440x900 in light and dark, the sandboxed frame and its theme, live progress, changed marks, menus by ⋯, right click and long press, parts, notes, devices, revoke, remove with Undo, add, the Folder sheet, and the pages on a machine, a session and a reply; screenshots in ${shots}`)
