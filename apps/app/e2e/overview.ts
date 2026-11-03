// bun apps/app/e2e/overview.ts — the built app (apps/app/dist, from build.sh) in Chromium behind a
// stand-in hub: the page under the hub's own CSP for /next, the sign-in endpoints, the pane actions
// and the /ws2 streams (protocol v2), fed made-up data (demo.ts). Signs in, checks the Overview and
// a session's screen at phone and desktop sizes in light and dark, answers a waiting agent from a
// card and from the chat, sends a message, loads older messages, opens the terminal (typing, an IME,
// the key bar, its menu, resting while hidden, closing, reconnecting), folds a machine, rides out a
// dropped socket, and saves a screenshot of each size and theme to $SHOTS (default: a temp folder).
// CHROME=/path/to/chrome uses a browser other than Playwright's own.
import { mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium, devices, type Page } from 'playwright'
import type { ServerWebSocket } from 'bun'
import { accounts, overview, storefront, twoQuestions } from './demo.ts'

const root = path.join(import.meta.dir, '../../..')
const dist = path.join(import.meta.dir, '../dist')
const web = path.join(root, 'apps/web/public')
const shots = process.env.SHOTS ?? path.join(tmpdir(), 'inyon-app-shots')
mkdirSync(shots, { recursive: true })

// the policy the hub sends with the page, read from the hub's source so the two can't drift
const rs = await Bun.file(path.join(root, 'crates/inyon-hub/src/static_files.rs')).text()
const NEXT_CSP = /pub const NEXT_CSP: &str =\s*"([^"]+)"/.exec(rs)![1]!

const PASSWORD = 'correct horse'
const state = { setup: false, setupCode: false, authed: false, refuse: false, answers: [] as unknown[], sends: [] as unknown[], drafts: [] as unknown[], frames: [] as unknown[], slow: 0, controls: [] as Record<string, unknown>[] }
type Socket = ServerWebSocket<{ subs: Set<string> }>
const sockets = new Set<Socket>()

// the chats the stand-in hub has, as /ws2 streams: rows, single values and a seq per change
type Stream = { seq: number; items: Record<string, unknown>[]; state: Record<string, unknown>; older?: Record<string, unknown>[] }
const streams = new Map<string, Stream>()
function resetChats() {
  streams.clear()
  const s = storefront()
  streams.set('local:%11', { seq: 1, items: s.items, state: s.state, older: s.older })
  const a = accounts()
  streams.set('local:%12', { seq: 1, items: a.items, state: a.state })
}
resetChats()
const EPOCH = 1

function snapshot(stream: string) {
  if (stream === 'overview') return { t: 'snapshot', stream, epoch: EPOCH, seq: 1, rows: { hosts: overview().hosts } }
  const c = streams.get(stream)
  if (!c) return { t: 'snapshot', stream, epoch: EPOCH, seq: 1, rows: { items: [] }, state: { loaded: true, hasMore: false } }
  return { t: 'snapshot', stream, epoch: EPOCH, seq: c.seq, rows: { items: c.items, live: [], queue: [] }, state: c.state }
}

/** A change to a chat, sent to every socket that follows it. */
function change(stream: string, parts: { append?: Record<string, unknown>[]; state?: Record<string, unknown> }) {
  const c = streams.get(stream)!
  const from = c.seq++
  if (parts.append) c.items.push(...parts.append)
  if (parts.state) Object.assign(c.state, parts.state)
  const frame = JSON.stringify({ t: 'delta', stream, epoch: EPOCH, from, seq: c.seq, ...(parts.append ? { append: { items: parts.append } } : {}), ...(parts.state ? { state: parts.state } : {}) })
  for (const ws of sockets) if (ws.data.subs.has(stream)) ws.send(frame)
}

/** The key an answer names its question by (inyon-protocol's prompt_key). */
const promptKey = (p: { body: string[]; options: { n: number; label: string }[] }) => [...p.body, ...p.options.map((o) => `${o.n}.${o.label}`)].join('\n').replace(/\s+/g, '')

const server = Bun.serve({
  port: 0,
  async fetch(req, srv) {
    const url = new URL(req.url)
    const p = url.pathname
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) => Response.json(body, { status, headers })
    if (p === '/ws2') {
      if (!state.authed || state.refuse) return new Response('Not signed in', { status: 401 })
      return srv.upgrade(req, { data: { subs: new Set<string>() } }) ? undefined : new Response('no upgrade', { status: 400 })
    }
    if (p === '/api/session') return json({ authed: state.authed, setup: state.setup, setupCode: state.setupCode })
    if (p === '/api/login' && req.method === 'POST') {
      const { password } = await req.json()
      if (password !== PASSWORD) return json({ error: 'Wrong password' }, 401)
      state.authed = true
      return json({ ok: true }, 200, { 'set-cookie': 'inyon_session=test; Path=/; HttpOnly; SameSite=Strict' })
    }
    if (p === '/api/setup' && req.method === 'POST') {
      const { password, code } = await req.json()
      if (state.setupCode && code !== 'ab12-cd34') return json({ error: 'That setup code is wrong' }, 403)
      if (password.length < 8) return json({ error: 'Too short' }, 400)
      state.setup = false
      state.authed = true
      return json({ ok: true })
    }
    // the session menu's calls: exit to the shell, Remote Control, Codex's rewind, ultracode, fork
    const control = /^\/api\/hosts\/([^/]+)\/panes\/([^/]+)\/(exit|rc|rewind|ultracode|fork)$/.exec(p)
    if (control && req.method === 'POST') {
      const body = await req.json().catch(() => ({}))
      state.controls.push({ pane: control[2], what: control[3], ...body })
      return json(control[3] === 'fork' ? { pane: '%99' } : { ok: true })
    }
    const action = /^\/api\/hosts\/([^/]+)\/panes\/([^/]+)\/(answer|send)$/.exec(p)
    if (action && req.method === 'POST') {
      const host = decodeURIComponent(action[1]!)
      const pane = action[2]!
      const body = await req.json()
      const stream = `${host}:%${pane}`
      if (action[3] === 'send') {
        state.sends.push({ host, pane, ...body })
        // the agent's record picks the message up a moment later
        setTimeout(() => change(stream, { append: [{ kind: 'user', seq: 1000 + state.sends.length, uuid: `sent${state.sends.length}`, ts: new Date().toISOString(), text: body.text, images: 0 }] }), 400)
        return json({ ok: true })
      }
      state.answers.push({ host, pane, ...body })
      const c = streams.get(stream)
      const asked = (c?.state.screen as { prompt?: { body: string[]; options: { n: number; label: string }[] } } | undefined)?.prompt
      if (c && asked) {
        // the hub presses keys only for the question on screen
        if (body.question !== undefined && body.question !== promptKey(asked)) return json({ error: 'That question is no longer on screen' }, 409)
        // the first answer brings the next question; the second lets the agent go on
        const next = asked.body[0] === twoQuestions.body[0] ? { working: true, activity: 'Writing the migration…', model: 'GPT-6-Astra', effort: 'medium', width: 120 } : { working: false, model: 'GPT-6-Astra', effort: 'medium', width: 120, prompt: twoQuestions }
        setTimeout(() => change(stream, { state: { screen: next } }), 300)
      }
      return json({ ok: true })
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
    message(ws, m) {
      const msg = JSON.parse(String(m))
      state.frames.push(msg)
      if (msg.t === 'hello') ws.send(JSON.stringify({ t: 'hello', v: 2 }))
      if (msg.t === 'sub') {
        ws.data.subs.add(msg.stream)
        // the overview and a chat that hasn't changed since `at`: up to date already
        const c = streams.get(msg.stream)
        const seq = msg.stream === 'overview' ? 1 : (c?.seq ?? 1)
        if (msg.at?.epoch === EPOCH && msg.at.seq === seq) ws.send(JSON.stringify({ t: 'delta', stream: msg.stream, epoch: EPOCH, from: seq, seq }))
        else {
          // `slow` holds the lists back, as a hub that's slow to answer would
          const send = () => ws.send(JSON.stringify(snapshot(msg.stream)))
          if (state.slow) setTimeout(send, state.slow)
          else send()
        }
      }
      if (msg.t === 'unsub') ws.data.subs.delete(msg.stream)
      if (msg.t === 'more') {
        const c = streams.get(msg.stream)
        ws.send(JSON.stringify({ t: 'more', stream: msg.stream, items: c?.older ?? [], hasMore: false }))
        if (c) c.state.hasMore = false
      }
      if (msg.t === 'draft') state.drafts.push(msg)
      if (msg.t === 'ping') ws.send(JSON.stringify({ t: 'pong' }))
      if (msg.t.startsWith('term.')) terminal(ws, msg)
    },
  },
})
const base = `http://127.0.0.1:${server.port}`

// the pane's tmux window, as the hub streams it: a shell that echoes what's typed, prints a prompt
// on Enter, and lets go when `exit` is typed
const PROMPT = '\x1b[1mstorefront\x1b[0m on \x1b[32mmain\x1b[0m $ '
const lines = new Map<string, string>()
function terminal(ws: Socket, msg: { t: string; id: string; d?: string }) {
  const data = (d: string) => ws.send(JSON.stringify({ t: 'term.data', id: msg.id, d }))
  if (msg.t === 'term.open') {
    lines.set(msg.id, '')
    data('Last login: Fri Oct 31 18:02 on ttys002\r\n\x1b[34msrc\x1b[0m  \x1b[34mtests\x1b[0m  package.json  README.md  bun.lock\r\n' + PROMPT)
  }
  if (msg.t === 'term.in' && lines.has(msg.id)) {
    for (const c of msg.d ?? '') {
      if (c === '\r') {
        if (lines.get(msg.id) === 'exit') {
          lines.delete(msg.id)
          return ws.send(JSON.stringify({ t: 'term.exit', id: msg.id, reason: 'The pane closed' }))
        }
        lines.set(msg.id, '')
        data('\r\n' + PROMPT)
      } else if (c >= ' ') {
        lines.set(msg.id, lines.get(msg.id) + c)
        data(c)
      }
    }
  }
  if (msg.t === 'term.close') lines.delete(msg.id)
}
type Frame = { t: string; [k: string]: unknown }
/** What the app has sent lately, for a failed check to show. */
const trail = () => JSON.stringify((state.frames.slice(-24) as Frame[]).filter((f) => f.t !== 'ping').map((f) => `${f.t} ${f.id ?? f.stream ?? ''}`))
/** The app's messages of one kind from the `from`th on, once there are `n` of them. */
async function frames(t: string, n = 1, from = 0, ms = 8000): Promise<Frame[]> {
  const end = Date.now() + ms
  for (;;) {
    const got = (state.frames.slice(from) as Frame[]).filter((f) => f.t === t)
    if (got.length >= n || Date.now() > end) return got
    await Bun.sleep(50)
  }
}

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
try {
  // ---- sign in, as a returning user ----
  {
    const ctx = await browser.newContext({ ...devices['iPhone 13'], colorScheme: 'light', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/`)
    await page.getByRole('heading', { name: 'Welcome back' }).waitFor()
    await page.screenshot({ path: path.join(shots, 'signin-390-light.png') })
    await layout(page, 'sign-in')
    const pw = page.getByLabel('Password', { exact: true })
    await pw.fill('wrong one')
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.getByRole('alert').filter({ hasText: 'Wrong password' }).waitFor()
    await pw.fill(PASSWORD)
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.getByRole('heading', { name: 'Overview' }).waitFor()
    check(await page.getByText('1 session needs you.').isVisible(), 'sign-in: the overview after signing in')
    await ctx.close()
  }

  // ---- the first password on a new hub, with its setup code ----
  {
    Object.assign(state, { authed: false, setup: true, setupCode: true })
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'dark', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next`)
    await page.getByRole('heading', { name: 'Create your password' }).waitFor()
    const submit = page.getByRole('button', { name: 'Create password' })
    await page.getByLabel('New password').fill('long enough')
    await page.getByLabel('Type the password again').fill('long enougj')
    check(await submit.isDisabled(), 'setup: submit waits for the setup code')
    await page.getByLabel('Setup code').fill('ab12-cd34')
    await submit.click()
    await page.getByRole('alert').filter({ hasText: "The two passwords don't match." }).waitFor()
    await page.getByLabel('Type the password again').fill('long enough')
    await page.screenshot({ path: path.join(shots, 'setup-1440-dark.png') })
    await submit.click()
    await page.getByRole('heading', { name: 'Overview' }).waitFor()
    await ctx.close()
  }

  // ---- the overview at both sizes, light and dark ----
  state.authed = true
  for (const [w, h] of [[390, 844], [1440, 900]] as const)
    for (const scheme of ['light', 'dark'] as const) {
      const label = `overview ${w}x${h} ${scheme}`
      const phone = w < 600
      const ctx = await browser.newContext({ ...(phone ? devices['iPhone 13'] : {}), viewport: { width: w, height: h }, colorScheme: scheme, reducedMotion: 'reduce' })
      const page = await ctx.newPage()
      watch(page)
      await page.goto(`${base}/next/`)
      await page.getByRole('heading', { name: 'Overview' }).waitFor()
      await page.locator('.card').first().waitFor()
      check((await page.evaluate(() => document.documentElement.dataset.theme)) === scheme, `${label}: the theme follows the system`)
      check((await page.locator('.card').count()) === 4, `${label}: four session cards`)
      check(await page.getByRole('button', { name: /studio/ }).isVisible(), `${label}: sessions grouped by machine`)
      check(await page.getByText('Plan limits').isVisible(), `${label}: the plan limits`)
      check(await page.locator(phone ? '.machine-list' : '.machine-table').isVisible(), `${label}: the machines as a ${phone ? 'list' : 'table'}`)
      check(await page.locator(phone ? '.tabbar' : '.sidebar').isVisible(), `${label}: the ${phone ? 'tab bar' : 'sidebar'}`)
      check((await page.locator('a[href="/next/m/local/p/11"]').count()) === 1, `${label}: a card opens its session in this app`)
      await layout(page, label)
      await page.screenshot({ path: path.join(shots, `overview-${w}-${scheme}.png`) })
      // the page scrolls inside the shell, so the lower half is a second picture
      await page.locator('.scroll').evaluate((el) => el.scrollTo(0, el.scrollHeight))
      await page.screenshot({ path: path.join(shots, `overview-${w}-${scheme}-end.png`) })
      await ctx.close()
    }

  // ---- a session: the chat at both sizes, light and dark ----
  resetChats()
  for (const [w, h] of [[390, 844], [1440, 900]] as const)
    for (const scheme of ['light', 'dark'] as const) {
      const label = `session ${w}x${h} ${scheme}`
      const phone = w < 600
      const ctx = await browser.newContext({ ...(phone ? devices['iPhone 13'] : {}), viewport: { width: w, height: h }, colorScheme: scheme, reducedMotion: 'reduce' })
      const page = await ctx.newPage()
      watch(page)
      await page.goto(`${base}/next/m/local/p/11`)
      await page.getByRole('heading', { name: 'Fix flaky checkout test' }).waitFor()
      await page.getByText('All 20 runs pass now.').waitFor()
      check(await page.getByText('Claude Code · studio · storefront:1').isVisible(), `${label}: where the pane is`)
      check(await page.locator('.working-verb').filter({ hasText: 'Running the whole suite…' }).isVisible(), `${label}: the working line`)
      check(await page.getByRole('button', { name: 'Queue follow-up' }).count() + (await page.getByRole('button', { name: 'Stop' }).count()) === 1, `${label}: one send/stop button`)
      check(await page.locator('.composer-controls').filter({ hasText: 'Sonnet 5.5 · medium' }).isVisible(), `${label}: the model and effort under the box`)
      check(phone === (await page.getByRole('link', { name: 'Back' }).isVisible()), `${label}: a back link on phones only`)
      check((await page.getByRole('link', { name: /Terminal/ }).getAttribute('href')) === '/next/m/local/p/11?view=terminal', `${label}: the terminal opens in this app`)
      // only the rows near the screen are in the page
      const drawn = await page.locator('.vrow').count()
      check(drawn > 3 && drawn < 40, `${label}: ${drawn} of 64 rows drawn`)
      await layout(page, label)
      await page.screenshot({ path: path.join(shots, `session-${w}-${scheme}.png`) })
      // the reply's code block, open tool details and the indented message, from closer up
      await page.getByRole('button', { name: /Edited|Edit/ }).last().click()
      await page.getByText('expect(total).toBe(4200)').scrollIntoViewIfNeeded()
      await page.waitForTimeout(400)
      await page.screenshot({ path: path.join(shots, `session-${w}-${scheme}-history.png`) })

      await page.goto(`${base}/next/m/local/p/12`)
      await page.getByText('Would you like to run the following command?').waitFor()
      check(!(await page.locator('.composer').count()), `${label}: the question takes the box's place`)
      check(await page.locator('.prompt-why').isVisible(), `${label}: one line says where the model controls went`)
      const likely = await page.waitForFunction(() => document.activeElement?.textContent === '1Yes, proceed', null, { timeout: 3000 }).then(() => true, () => false)
      check(likely, `${label}: focus on the likely answer`)
      await layout(page, `${label} question`)
      await page.screenshot({ path: path.join(shots, `session-${w}-${scheme}-question.png`) })
      await ctx.close()
    }

  // ---- a session: send, load older messages, answer two questions ----
  {
    resetChats()
    state.answers = []
    const ctx = await browser.newContext({ ...devices['iPhone 13'], colorScheme: 'light', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    // from the Overview's card, without loading the page again
    await page.goto(`${base}/next/`)
    await page.locator('a[href="/next/m/local/p/11"]').click()
    await page.getByRole('heading', { name: 'Fix flaky checkout test' }).waitFor()
    check(await page.evaluate(() => performance.getEntriesByType('navigation').length === 1 && location.pathname === '/next/m/local/p/11'), 'route: the card opens the session in place')

    // a message's own indentation and the code block's survive (Ghostex #252)
    const bubble = await page.locator('.bubble').filter({ hasText: 'expect(total)' }).evaluate((el) => ({ text: el.textContent, ws: getComputedStyle(el).whiteSpace }))
    check(bubble.text?.includes('\n    expect(total).toBe(4200)\n      at checkout.test.ts:88') && bubble.ws === 'pre-wrap', `indent: the message keeps its spaces (${JSON.stringify(bubble)})`)
    const code = await page.locator('.code-body').first().textContent()
    check(code?.includes('\n  const discounted = await applyDiscount(cart)\n  return discounted.items.reduce(\n    (sum, item)'), `indent: the code block keeps its spaces (${JSON.stringify(code)})`)

    // older messages: reading back to the top fetches them (a wheel, as a reader's own scroll)
    await page.mouse.move(195, 400)
    for (let i = 0; i < 12; i++) await page.mouse.wheel(0, -1500)
    // reaching the top fetches them by itself; the button is there for a reader who stops short
    const earlier = page.getByRole('button', { name: 'Earlier messages' })
    if (await earlier.isVisible()) await earlier.click()
    // they come in above the row being read, which stays put: scroll up to them
    await page.waitForTimeout(500)
    for (let i = 0; i < 4; i++) await page.mouse.wheel(0, -1500)
    await page.getByText('Start the cart refactor: one map for the items').waitFor()
    check(state.frames.some((f) => JSON.stringify(f) === JSON.stringify({ t: 'more', stream: 'local:%11', before: 1 })), 'more: asks for what came before the first message')

    // typing is the draft, sending posts the message and it lands in the conversation
    const box = page.getByRole('textbox', { name: 'Message Claude Code' })
    await box.fill('Commit it as "Wait for the discount before totalling"')
    await page.waitForTimeout(700)
    check(state.drafts.some((d) => JSON.stringify(d) === JSON.stringify({ t: 'draft', host: 'local', pane: '%11', text: 'Commit it as "Wait for the discount before totalling"' })), `draft: sent to the hub (${JSON.stringify(state.drafts)})`)
    await page.getByRole('button', { name: 'Queue follow-up' }).click()
    await page.locator('.bubble').filter({ hasText: 'Commit it as' }).first().waitFor()
    const sent = state.sends[0] as { text?: string; requestId?: string } | undefined
    check(sent?.text === 'Commit it as "Wait for the discount before totalling"' && !!sent.requestId, `send: posted ${JSON.stringify(state.sends)}`)
    check((await box.inputValue()) === '', 'send: the box empties')
    await page.waitForTimeout(1200)
    check((await page.locator('.bubble').filter({ hasText: 'Commit it as' }).count()) === 1, 'send: the message shows once after it lands')
    check(!(await page.locator('.bubble-pending').count()), 'send: no longer pending once the record has it')
    await page.screenshot({ path: path.join(shots, 'session-390-light-sent.png') })

    // the agent asks: keys pick, and the answer names its question (Ghostex #251). The keys go
    // in as soon as the card shows: the card holds focus from its first frame, before its likely
    // choice can take it (that gap lost the keys on slow CI runners)
    await page.goto(`${base}/next/m/local/p/12`)
    await page.getByText('Would you like to run the following command?').waitFor()
    check(await page.evaluate(() => !!document.activeElement?.closest('.prompt')), 'questions: the card has focus as soon as it shows')
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('Enter')
    await page.getByText('Which store should the passkeys live in?').waitFor()
    const first = state.answers[0] as Record<string, unknown> | undefined
    check(first?.option === 2 && first.question === promptKey(accounts().state.screen.prompt), `answer: ${JSON.stringify(first)}`)
    check((await page.getByRole('tab').count()) === 3, 'questions: one tab each')
    const moved = await page.waitForFunction(() => (document.activeElement?.textContent ?? '').startsWith('1In the accounts table'), null, { timeout: 3000 }).then(() => true, () => false)
    check(moved, 'questions: focus moves to the new question')
    await page.screenshot({ path: path.join(shots, 'session-390-light-questions.png') })
    // the free-text choice opens a box for the answer
    await page.keyboard.press('3')
    const free = page.getByRole('textbox', { name: 'Your answer' })
    await free.waitFor()
    await page.getByRole('button', { name: 'Back' }).click()
    await page.keyboard.press('2')
    await page.locator('.composer').waitFor()
    const second = state.answers[1] as Record<string, unknown> | undefined
    check(second?.option === 2 && second.question === promptKey(twoQuestions), `answer: ${JSON.stringify(second)}`)
    check(await page.locator('.composer-controls').filter({ hasText: 'GPT-6-Astra · medium' }).isVisible(), 'answer: the model controls are back')
    await ctx.close()
  }

  // ---- the questions on a desktop, dark ----
  {
    resetChats()
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'dark', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/m/local/p/12`)
    await page.getByText('Would you like to run the following command?').waitFor()
    await page.getByRole('option', { name: /Yes, proceed/ }).click()
    await page.getByText('Which store should the passkeys live in?').waitFor()
    await layout(page, 'questions 1440 dark')
    await page.screenshot({ path: path.join(shots, 'session-1440-dark-questions.png') })
    await ctx.close()
  }

  // ---- a session's terminal: typing, IME, the key bar, the menu, resting, closing, reconnecting ----
  {
    resetChats()
    state.frames = []
    const ctx = await browser.newContext({ ...devices['iPhone 13'], colorScheme: 'light', reducedMotion: 'reduce' })
    await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: base })
    const page = await ctx.newPage()
    watch(page)
    await page.clock.install()
    // an agent's pane waits to learn what runs in it, rather than opening a terminal its chat replaces
    state.slow = 1500
    await page.goto(`${base}/next/m/local/p/11`)
    await page.getByRole('link', { name: 'Terminal' }).waitFor()
    state.slow = 0
    check(!(state.frames as Frame[]).some((f) => f.t.startsWith('term.')), `terminal: a chat pane opens no terminal while it loads (${trail()})`)
    // the header's Terminal, from the chat
    await page.getByRole('link', { name: 'Terminal' }).click()
    await page.waitForURL(/\/next\/m\/local\/p\/11\?view=terminal$/)
    const [open] = await frames('term.open')
    check(open?.host === 'local' && open.pane === '%11' && Number(open.cols) >= 30 && Number(open.rows) >= 10, `terminal: opened ${JSON.stringify(open)}`)
    const id = String(open?.id)
    await page.locator('.xterm').waitFor()
    const renderer = await page.locator('#term-screen').getAttribute('data-renderer')
    check(renderer === 'webgl' || renderer === 'dom', `terminal: drawn by ${renderer}`)
    check(await page.getByRole('toolbar', { name: 'Keys' }).isVisible(), 'terminal: the key bar shows on a phone')
    check(await page.getByRole('button', { name: 'Keyboard' }).isVisible(), 'terminal: a phone gets the keyboard key')
    await page.waitForTimeout(400)
    await layout(page, 'terminal 390 light')
    await page.screenshot({ path: path.join(shots, 'terminal-390-light.png') })

    // typing: a tap brings the keyboard up, keys go to tmux one by one
    let at = state.frames.length
    await page.locator('#term-screen').click()
    await page.keyboard.type('ls')
    await page.keyboard.press('Enter')
    const typed = (await frames('term.in', 3, at)).map((f) => f.d).join('')
    check(typed === 'ls\r', `terminal: typed ${JSON.stringify(typed)}`)
    check((await frames('term.in', 1, at)).every((f) => f.id === id), `terminal: typing goes to its own terminal (${trail()})`)

    // an IME: nothing goes while composing, the chosen text goes once it's committed
    at = state.frames.length
    const cdp = await ctx.newCDPSession(page)
    await cdp.send('Input.imeSetComposition', { text: 'にほん', selectionStart: 3, selectionEnd: 3 })
    await page.waitForTimeout(200)
    check(!(await frames('term.in', 1, at, 300)).length, 'terminal: nothing is sent while composing')
    await cdp.send('Input.insertText', { text: '日本' })
    const ime = (await frames('term.in', 1, at)).map((f) => f.d).join('')
    check(ime === '日本', `terminal: IME committed ${JSON.stringify(ime)}`)
    await page.keyboard.insertText('語')
    check((await frames('term.in', 2, at)).at(-1)?.d === '語', 'terminal: inserted text goes as typed')

    // the key bar: sticky Ctrl for the next key, then up again; keys a phone lacks
    at = state.frames.length
    const ctrl = page.getByRole('button', { name: 'Ctrl (for the next key)' })
    await ctrl.click()
    check((await ctrl.getAttribute('aria-pressed')) === 'true', 'terminal: Ctrl stays down')
    await page.keyboard.type('c')
    await page.getByRole('button', { name: 'Escape' }).click()
    await page.getByRole('button', { name: 'Up', exact: true }).click()
    const keys = (await frames('term.in', 3, at)).map((f) => f.d)
    check(JSON.stringify(keys) === JSON.stringify(['\x03', '\x1b', '\x1b[A']), `terminal: key bar sent ${JSON.stringify(keys)}`)
    check((await ctrl.getAttribute('aria-pressed')) === 'false', 'terminal: Ctrl lets go after one key')

    // larger text: fewer columns, told to tmux, and kept for next time (the web app's setting)
    at = state.frames.length
    await page.getByRole('button', { name: 'Larger text' }).click()
    const [bigger] = await frames('term.resize', 1, at)
    check(bigger?.id === id && Number(bigger.cols) < Number(open?.cols), `terminal: larger text resized to ${JSON.stringify(bigger)}`)
    const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('inyon.settings.v2') ?? '{}').termFontSize)
    check(saved === 14, `terminal: text size saved as ${saved}`)

    // the right-click menu: Paste and Select all, and Copy once something is selected
    const screen = page.locator('#term-screen')
    await screen.click({ button: 'right' })
    const items = async () => (await page.getByRole('menuitem').allTextContents()).map((t) => t.trim())
    check(JSON.stringify(await items()) === JSON.stringify(['Paste', 'Select all']), `terminal menu: ${JSON.stringify(await items())}`)
    await page.getByRole('menuitem', { name: 'Select all' }).click()
    await screen.click({ button: 'right' })
    check((await items())[0] === 'Copy', `terminal menu with a selection: ${JSON.stringify(await items())}`)
    await page.getByRole('menuitem', { name: 'Copy' }).click()
    const copied = await page.evaluate(() => navigator.clipboard.readText())
    check(copied.includes('storefront on main $ ls'), `terminal: copied ${JSON.stringify(copied.slice(0, 80))}`)
    await page.evaluate(() => navigator.clipboard.writeText('echo pasted'))
    at = state.frames.length
    await screen.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Paste' }).click()
    const pasted = (await frames('term.in', 1, at)).map((f) => f.d).join('')
    check(pasted === 'echo pasted', `terminal: pasted ${JSON.stringify(pasted)}`)
    await page.screenshot({ path: path.join(shots, 'terminal-390-light-typed.png') })

    // nobody watching: after a while hidden it lets go of the window (so it rests at 200×50), and
    // takes it back when the page is shown
    const visibility = (v: string) =>
      page.evaluate((v) => {
        Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => v })
        document.dispatchEvent(new Event('visibilitychange'))
      }, v)
    at = state.frames.length
    await visibility('hidden')
    await page.clock.fastForward(5_000)
    check(!(await frames('term.close', 1, at, 300)).length, 'terminal: a short look away keeps the window')
    await page.clock.fastForward(11_000)
    const [rested] = await frames('term.close', 1, at)
    check(rested?.id === id, `terminal: let go while hidden ${JSON.stringify(rested)}`)
    await visibility('visible')
    const [back] = await frames('term.open', 1, at)
    check(!!back && back.id !== id && back.pane === '%11', `terminal: opened again when shown ${JSON.stringify(back)}`)

    // tmux lets go: it says so, and Reconnect opens it again
    at = state.frames.length
    await page.keyboard.type('exit')
    await page.keyboard.press('Enter')
    await page.getByText('The pane closed').waitFor()
    await page.screenshot({ path: path.join(shots, 'terminal-390-light-closed.png') })
    await page.getByRole('button', { name: 'Reconnect' }).click()
    check((await frames('term.open', 1, at)).length === 1, 'terminal: Reconnect opens it again')

    // the hub restarting: the socket comes back and the terminal opens on it again
    at = state.frames.length
    for (const ws of sockets) ws.close()
    const [again] = await frames('term.open', 1, at, 15_000)
    check(!!again, 'terminal: open again after the socket came back')

    // back to the chat: the terminal lets go
    at = state.frames.length
    await page.getByRole('link', { name: 'Chat' }).click()
    await page.waitForURL(/\/next\/m\/local\/p\/11$/)
    check((await frames('term.close', 1, at)).length === 1, 'terminal: closed when the chat shows')
    await ctx.close()
  }

  // ---- the terminal at each size and theme; a window resized; the drawing without WebGL ----
  for (const [w, h] of [[390, 844], [1440, 900]] as const) {
    for (const scheme of ['dark', 'light'] as const) {
      if (w === 390 && scheme === 'light') continue
      const phone = w < 600
      const ctx = await browser.newContext({ ...(phone ? devices['iPhone 13'] : {}), viewport: { width: w, height: h }, colorScheme: scheme, reducedMotion: 'reduce' })
      const page = await ctx.newPage()
      watch(page)
      const at = state.frames.length
      // a slow hub: the terminal opens before the session learns its agent and puts its skin on
      if (phone) state.slow = 1200
      await page.goto(`${base}/next/m/local/p/11?view=terminal`)
      const [open] = await frames('term.open', 1, at)
      await page.locator('.xterm').waitFor()
      await page.locator('.session.skin-claude').waitFor()
      state.slow = 0
      await page.waitForTimeout(400)
      const label = `terminal ${w} ${scheme}`
      const ground = await page.evaluate(() => {
        const bg = (s: string) => getComputedStyle(document.querySelector(s) as Element).backgroundColor
        return { session: bg('.session'), grid: bg('.xterm-scrollable-element'), viewport: bg('.xterm-viewport') }
      })
      check(ground.grid === ground.session, `${label}: the grid takes the session's skin ${JSON.stringify(ground)}`)
      check(ground.viewport === 'rgba(0, 0, 0, 0)', `${label}: nothing paints under the last row ${JSON.stringify(ground)}`)
      check(!!open, `${label}: opened`)
      check(phone === (await page.getByRole('button', { name: 'Keyboard' }).isVisible()), `${label}: the keyboard key only on a phone`)
      await layout(page, label)
      await page.screenshot({ path: path.join(shots, `terminal-${w}-${scheme}.png`) })
      if (!phone && scheme === 'light') {
        const from = state.frames.length
        await page.setViewportSize({ width: 1000, height: 700 })
        await frames('term.resize', 1, from)
        await page.waitForTimeout(500)
        const r = (await frames('term.resize', 1, from)).at(-1)
        check(!!r && Number(r.cols) < Number(open?.cols) && Number(r.rows) < Number(open?.rows), `${label}: a smaller window resized to ${JSON.stringify(r)} from ${open?.cols}x${open?.rows}`)
      }
      await ctx.close()
    }
  }
  {
    // without WebGL the page draws the text itself, so what tmux sent can be read back
    const plain = await chromium.launch({ ...(process.env.CHROME ? { executablePath: process.env.CHROME } : {}), args: ['--disable-webgl', '--disable-3d-apis'] })
    const ctx = await plain.newContext({ viewport: { width: 1024, height: 700 }, colorScheme: 'light', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/m/local/p/11?view=terminal`)
    await page.locator('.xterm-rows').waitFor()
    await page.keyboard.type('pwd')
    await page.waitForFunction(() => document.querySelector('.xterm-rows')?.textContent?.includes('storefront on main $ pwd'), null, { timeout: 8000 }).catch(() => {})
    const text = (await page.locator('.xterm-rows').textContent()) ?? ''
    check((await page.locator('#term-screen').getAttribute('data-renderer')) === 'dom', 'terminal without WebGL: drawn by the page')
    check(text.includes('Last login') && text.includes('storefront on main $ pwd'), `terminal without WebGL: shows ${JSON.stringify(text.slice(0, 120))} (${trail()})`)
    await ctx.close()
    await plain.close()
  }

  // ---- answering, folding, a dropped socket ----
  resetChats()
  state.answers = []
  {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'light', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/`)
    await page.locator('.card').first().waitFor()
    await page.locator('.card-waiting').getByRole('button', { name: 'Yes' }).click()
    await page.locator('.card-waiting').getByText('Sent').waitFor()
    check(JSON.stringify(state.answers) === JSON.stringify([{ host: 'local', pane: '12', option: 1 }]), `answer: posted ${JSON.stringify(state.answers)}`)

    await page.getByRole('radio', { name: 'Working' }).click()
    check((await page.locator('.card').count()) === 2, 'filter: working and background sessions only')
    await page.getByRole('radio', { name: 'All' }).click()
    check((await page.locator('.card').count()) === 4, 'filter: all again')

    const group = page.getByRole('button', { name: /build-box/ })
    await group.click()
    check((await group.getAttribute('aria-expanded')) === 'false', 'fold: the group closes')
    check((await page.evaluate(() => localStorage.getItem('inyon.folded'))) === '["home:build-box"]', 'fold: kept in the web app’s key')
    await page.reload()
    await page.locator('.card').first().waitFor()
    check((await page.getByRole('button', { name: /build-box/ }).getAttribute('aria-expanded')) === 'false', 'fold: still closed after a reload')

    // the hub restarting: the socket drops and isn't taken back for a moment
    state.refuse = true
    for (const ws of sockets) ws.close()
    await page.getByText('Reconnecting').first().waitFor()
    state.refuse = false
    await page.getByText('Reconnecting').first().waitFor({ state: 'hidden', timeout: 15_000 })
    check((await page.locator('.card').count()) === 4, 'reconnect: the overview is back')

    // signed out elsewhere: the next drop finds the session gone and shows the form
    state.authed = false
    for (const ws of sockets) ws.close()
    await page.getByRole('heading', { name: 'Welcome back' }).waitFor({ timeout: 15_000 })
    await ctx.close()
  }

  // ---- the session header's ⋯: the web app's session menu ----
  {
    const until = async (ok: () => boolean, ms = 3000) => {
      const end = Date.now() + ms
      while (!ok() && Date.now() < end) await Bun.sleep(50)
      return ok()
    }
    state.authed = true
    resetChats()
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'light', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    watch(page)
    await page.goto(`${base}/next/m/local/p/11`)
    const more = page.locator('.session-head').getByRole('button', { name: 'More' })
    await more.click()
    const items = await page.getByRole('menuitem').allTextContents()
    for (const want of ['Turn on Remote Control', 'Compact context', 'Rewind…', 'Clear conversation', 'Exit to shell', 'End session…']) check(items.some((i) => i.includes(want)), `session menu: ${want} (${items.join(' | ')})`)
    await page.screenshot({ path: path.join(shots, 'session-menu-1440-light.png') })
    // a slash command, typed into the agent
    const sent = state.sends.length
    await page.getByRole('menuitem', { name: 'Compact context' }).click()
    check(await until(() => state.sends.length > sent && (state.sends.at(-1) as { text: string }).text === '/compact'), `session menu: compact sent ${JSON.stringify(state.sends.at(-1))}`)
    // Remote Control through the hub
    await more.click()
    await page.getByRole('menuitem', { name: 'Turn on Remote Control' }).click()
    await page.getByText('Remote Control on').waitFor()
    check(JSON.stringify(state.controls.at(-1)) === JSON.stringify({ pane: '11', what: 'rc', on: true }), `session menu: rc ${JSON.stringify(state.controls.at(-1))}`)
    // exiting asks first
    await more.click()
    await page.getByRole('menuitem', { name: 'Exit to shell' }).click()
    const ask = page.getByRole('dialog', { name: 'Exit Claude Code to the shell?' })
    await ask.waitFor()
    await ask.getByRole('button', { name: 'Exit' }).click()
    await page.getByText('Claude Code exited').waitFor()
    check(state.controls.at(-1)?.what === 'exit', 'session menu: exited at the hub')
    // Codex rewinds through the hub, after asking
    await page.goto(`${base}/next/m/local/p/12`)
    await more.click()
    await page.getByRole('menuitem', { name: 'Rewind…' }).click()
    await page.getByRole('dialog', { name: 'Rewind your last message?' }).getByRole('button', { name: 'Rewind' }).click()
    await page.getByText('Conversation rewound').waitFor()
    check(state.controls.at(-1)?.what === 'rewind' && state.controls.at(-1)?.pane === '12', `session menu: rewound ${JSON.stringify(state.controls.at(-1))}`)
    await ctx.close()
  }
} finally {
  await browser.close()
  server.stop(true)
}

// the sign-in check above reads a wrong password on purpose, and the hub refuses the socket once signed out
const unexpected = errors.filter((e) => !/401|Unauthorized|WebSocket/.test(e))
check(!unexpected.length, `errors in the page: ${unexpected.join(' | ')}`)
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`ok: sign-in, setup, the overview and a session at 390x844 and 1440x900 in light and dark, sending, older messages, answering from a card and from the chat, the terminal, the session menu, filtering, folding, reconnecting; screenshots in ${shots}`)
