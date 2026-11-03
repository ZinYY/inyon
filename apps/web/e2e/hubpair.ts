// bun e2e/hubpair.ts: the phone app pairs with a real hub by typed code and drives a session on it,
// end to end. Starts a hub on a free port with a throwaway home, data dir and tmux server, runs a
// stand-in for Claude Code in a pane (fake-claude.py), serves the hosted build (the phone core in a
// page) from another origin, gets a code the way the hub's web app does and types it into Add →
// Connect to a hub. Then, from the phone: a message, a goal, a model, an effort, an attachment and
// a draft, each checked where it lands, in the agent's pane. Run `bun run build:hosted` first.
import path from 'node:path'
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import { chromium, type Page } from 'playwright'

const root = path.resolve(import.meta.dir, '..')
const bun = process.execPath
const scratch = mkdtempSync(path.join(os.tmpdir(), 'inyon-hubpair-'))
const home = path.join(scratch, 'home')
const data = path.join(scratch, 'data')
const project = path.join(home, 'project')
// a socket path must fit in 108 bytes, so the tmux folder stays short
const tmuxDir = mkdtempSync('/tmp/ihp-')
for (const d of [home, data, project, path.join(home, '.local/bin')]) mkdirSync(d, { recursive: true })
// the probe finds agents on ~/.local/bin and by the process name
const claude = path.join(home, '.local/bin/claude')
copyFileSync(path.join(import.meta.dir, 'fake-claude.py'), claude)
chmodSync(claude, 0o755)
// a tmux server of its own: with $TMUX set, tmux would ignore TMUX_TMPDIR and use the one it's in
const env: Record<string, string | undefined> = { ...process.env, HOME: home, TMUX_TMPDIR: tmuxDir }
delete env.TMUX
delete env.TMUX_PANE
const tmux = (...args: string[]) => new TextDecoder().decode(Bun.spawnSync(['tmux', '-f', '/dev/null', ...args], { env, cwd: project }).stdout).trim()
tmux('new-session', '-d', '-s', 'work', '-x', '110', '-y', '32', '-c', project, claude)
const pane = tmux('list-panes', '-a', '-F', '#{pane_id}')
/** What the agent was sent: one entry per Enter. */
const received = () => (existsSync(path.join(home, 'fake-claude.log')) ? readFileSync(path.join(home, 'fake-claude.log'), 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l) as string) : [])

const hubPort = 47000 + Math.floor(Math.random() * 900)
// NODE_ENV=test: the hub doesn't build the inyon program for four systems while it runs
const hub = Bun.spawn([bun, 'src/index.ts'], {
  cwd: path.resolve(root, '../hub'),
  env: { ...env, INYON_DATA_DIR: data, PORT: String(hubPort), BIND: '127.0.0.1', NODE_ENV: 'test' },
  stdout: 'ignore',
  stderr: 'inherit',
})
const hubUrl = `http://127.0.0.1:${hubPort}`
// the first-run code, which the hub keeps in its data folder (`inyon hub code`)
const setupCodeFile = path.join(data, 'setup-code')
const setupCode = () => (existsSync(setupCodeFile) ? readFileSync(setupCodeFile, 'utf8').trim() : '')

// the hosted build from its own origin, as GitHub Pages or a phone would have it
const dist = path.join(root, 'dist-hosted')
const site = Bun.serve({
  port: 0,
  hostname: '127.0.0.1',
  fetch(req) {
    const url = new URL(req.url)
    const file = Bun.file(path.join(dist, url.pathname === '/' || !url.pathname.includes('.') ? 'index.html' : url.pathname))
    return file.size ? new Response(file) : new Response('Not found', { status: 404 })
  },
})
const siteUrl = `http://127.0.0.1:${site.port}`

const fail = (msg: string): never => {
  throw new Error(msg)
}
/** Waits until `check` holds, or fails saying what didn't happen. */
async function until(what: string, check: () => boolean | Promise<boolean>, ms = 20000) {
  for (const end = Date.now() + ms; Date.now() < end; await Bun.sleep(200)) if (await check()) return
  fail(what)
}
/** The agent was sent `text` (and nothing came back from the phone as a failure). */
const reaches = (page: Page, what: string, text: string) =>
  until(`${what} never reached the agent; it got ${JSON.stringify(received())}`, async () => {
    const err = await page.getByText(/Could not|Couldn't reach|Failed to fetch/).first().textContent({ timeout: 50 }).catch(() => null)
    if (err) fail(`${what} failed on the phone: ${err}`)
    return received().includes(text)
  })

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
let code = 0
let shot: Page | undefined
try {
  // wait for the hub, then give it a password as the first visit would
  await until('the hub wrote no setup code', () => /^[0-9a-f]{8}$/i.test(setupCode()), 10000)
  const setup = await fetch(`${hubUrl}/api/setup`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ password: 'correct horse battery', code: setupCode() }) })
  if (!setup.ok) fail(`setup answered ${setup.status}`)
  const cookie = setup.headers.get('set-cookie')!.split(';')[0]!
  // the hub's web app asks for a code; a fresh sign-in is allowed to
  const pair = await fetch(`${hubUrl}/api/phones/pair`, { method: 'POST', headers: { 'content-type': 'application/json', cookie, origin: hubUrl }, body: JSON.stringify({ origin: hubUrl }) })
  if (!pair.ok) fail(`pair answered ${pair.status}: ${await pair.text()}`)
  const pairCode = (await pair.json()) as { id: string; code: string; link: string }

  // the phone: type the address and code
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true })
  const page = await ctx.newPage()
  shot = page
  const failed: string[] = []
  page.on('requestfailed', (r) => r.url().startsWith(hubUrl) && failed.push(`${r.method()} ${r.url().slice(hubUrl.length)}: ${r.failure()?.errorText}`))
  await page.goto(`${siteUrl}/machines`)
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  await page.getByRole('button', { name: /Connect to a hub/ }).click()
  await page.getByLabel('Hub address').fill(`127.0.0.1:${hubPort}`)
  await page.getByLabel('Hub code').fill(`${pairCode.id} ${pairCode.code}`)
  await page.getByRole('button', { name: 'Pair' }).click()
  await page.getByText(/Paired with/).waitFor({ timeout: 15000 })
  const hubs = await page.evaluate(() => JSON.parse(localStorage.getItem('inyon.hubs.v1') ?? '[]').map((h: any) => h.id) as string[])
  if (hubs.length !== 1) fail(`expected one paired hub, got ${JSON.stringify(hubs)}`)
  // the hub lists the phone
  const phones = (await (await fetch(`${hubUrl}/api/phones`, { headers: { cookie } })).json()) as { name: string }[]
  if (phones.length !== 1) fail(`the hub lists ${phones.length} phones`)
  // the code was spent
  const again = await fetch(`${hubUrl}/api/phones/claim`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code: `${pairCode.id}:${pairCode.code}`, name: 'x' }) })
  if (again.ok) fail('the code worked twice')
  console.log(`ok: paired as "${phones[0]!.name}", code single-use`)

  // the session on the hub's own machine, under the hub's id
  await page.goto(`${siteUrl}/m/${encodeURIComponent(`hub:${hubs[0]}/local`)}/p/${pane.slice(1)}`)
  const box = page.getByRole('textbox', { name: /^Message/ })
  await box.waitFor({ timeout: 30000 })
  await page.getByText("Hi. I'm a stand-in for Claude.").waitFor({ timeout: 20000 })

  // a message: reaches the agent, and its bubble stops sending once the agent writes it down
  await box.fill('ping from the phone')
  await page.getByRole('button', { name: 'Send' }).click()
  await reaches(page, 'the message', 'ping from the phone')
  await page.getByText('Got it: ping from the phone').waitFor({ timeout: 20000 })
  await until('the message is still marked as sending', async () => (await page.getByText(/Sending/i).count()) === 0)
  console.log('ok: a message reaches the agent and settles')

  // a goal, from the goal sheet
  await page.getByRole('button', { name: 'Set a goal' }).click()
  await page.getByRole('dialog').getByRole('textbox').fill('all tests pass')
  await page.getByRole('button', { name: 'Set goal' }).click()
  await reaches(page, 'the goal', '/goal all tests pass')
  console.log('ok: a goal reaches the agent')

  // a model, from the model menu
  await page.getByRole('button', { name: 'Model' }).click()
  const model = page.getByRole('menu').getByRole('menuitem').filter({ hasText: /Opus|Sonnet|Haiku/ }).first()
  const picked = (await model.textContent())!
  await model.click()
  await until(`picking ${picked} sent no /model`, () => received().some((t) => t.startsWith('/model ')))
  console.log(`ok: a model (${received().find((t) => t.startsWith('/model '))}) reaches the agent`)

  // an effort level, from the effort menu
  const effort = page.getByRole('button', { name: /^Reasoning effort/ })
  if (await effort.count()) {
    await effort.click()
    await page.getByRole('menu').getByRole('menuitem').filter({ hasText: 'High' }).first().click()
    await until('the effort never reached the agent', () => received().some((t) => /^\/(effort|model) .*high/i.test(t)))
    console.log('ok: an effort level reaches the agent')
  }

  // an attachment: uploaded to the hub, then named in the message
  const file = path.join(scratch, 'notes.txt')
  writeFileSync(file, 'attached from the phone\n')
  await page.locator('input[type=file]').first().setInputFiles(file)
  await until('the attachment never finished uploading', async () => (await page.getByRole('button', { name: 'Send' }).isEnabled().catch(() => false)))
  await box.fill('see the notes')
  await page.getByRole('button', { name: 'Send' }).click()
  await until(`the attachment never reached the agent; it got ${JSON.stringify(received().at(-1))}`, () => received().some((t) => t.startsWith('see the notes') && /Attached files:.*notes\.txt/s.test(t)))
  const saved = received().find((t) => t.startsWith('see the notes'))!.split('\n').at(-1)!
  if (!existsSync(saved) || readFileSync(saved, 'utf8') !== 'attached from the phone\n') fail(`the attachment wasn't saved on the hub's machine (${saved})`)
  console.log('ok: an attachment uploads to the hub and reaches the agent')

  // a draft: typed on the phone, shown in the agent's input box
  await box.fill('half a thought')
  await until("the draft never showed in the agent's input box", () => tmux('capture-pane', '-p', '-t', pane).includes('❯ half a thought'))
  console.log("ok: a draft shows in the agent's input box")

  if (failed.length) fail(`requests to the hub failed:\n  ${failed.join('\n  ')}`)
  await ctx.close()
} catch (err) {
  console.error('FAIL', (err as Error).message)
  // SHOT=/path/to/file.png keeps a picture of the phone at the failure
  if (process.env.SHOT) await shot?.screenshot({ path: process.env.SHOT }).catch(() => {})
  code = 1
} finally {
  await browser.close()
  site.stop(true)
  hub.kill()
  Bun.spawnSync(['tmux', 'kill-server'], { env })
  rmSync(scratch, { recursive: true, force: true })
  rmSync(tmuxDir, { recursive: true, force: true })
}
process.exit(code)
