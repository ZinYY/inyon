// bun e2e/connect.ts — opens the built app (dist/, or DIST=<dir>) in Chromium on a phone and a
// desktop, light and dark, lets the hub go away, and opens "Can't connect?" from the Reconnecting
// chip. Fails when the checks don't run in order, a fix is missing, raw details show before Details
// is opened, text is under 12px, or the page scrolls sideways. SHOTS=<dir> saves screenshots.
// CHROME=/path/to/chrome uses a browser other than Playwright's own.
import path from 'node:path'
import { mkdirSync } from 'node:fs'
import { chromium, type Page } from 'playwright'

const dist = process.env.DIST ? path.resolve(process.env.DIST) : path.join(import.meta.dir, '../dist')
const shots = process.env.SHOTS
if (shots) mkdirSync(shots, { recursive: true })
const server = Bun.serve({
  port: 0,
  async fetch(req) {
    const f = Bun.file(path.join(dist, new URL(req.url).pathname))
    return new Response((await f.exists()) && !new URL(req.url).pathname.endsWith('/') ? f : Bun.file(path.join(dist, 'index.html')))
  },
})
// *.localhost reaches this machine, and reads like a hub's name in the sheet
const base = `http://studio.localhost:${server.port}`

const now = Date.now()
const host = {
  config: { id: 'box', name: 'studio', kind: 'local' }, status: 'online', hostname: 'studio', integration: 'installed', tmux: 'ok',
  agents: ['claude'], signedIn: ['claude'], os: 'Linux', latencyMs: 30, updatedAt: now, lastSeen: now,
  sessions: [{ id: '$1', name: 'app', attached: 0, windows: [{ id: '@1', index: 1, name: '', active: true, activity: now / 1000, panes: [
    { id: '%1', index: 0, active: true, command: 'claude', cwd: '/home/sam/app', title: '✳ Fix the login test', width: 80, height: 40, dead: false, agent: { kind: 'claude', pid: 1, state: 'idle', mode: 'default' } },
  ] }] }],
}

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const problems: string[] = []
const check = (ok: boolean, what: string) => ok || problems.push(what)

async function noSidewaysScroll(page: Page, where: string) {
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)
  check(wide <= 0, `${where}: the page scrolls sideways by ${wide}px`)
  const small = await page.$$eval('[role="dialog"] *', (els) =>
    els.filter((e) => e.childNodes.length && [...e.childNodes].some((n) => n.nodeType === 3 && n.textContent!.trim())).map((e) => [e.textContent!.trim().slice(0, 30), parseFloat(getComputedStyle(e).fontSize)] as const).filter(([, px]) => px < 12),
  )
  check(!small.length, `${where}: text under 12px: ${JSON.stringify(small)}`)
}

try {
  for (const [label, viewport] of [['phone', { width: 390, height: 844 }], ['desktop', { width: 1440, height: 900 }]] as const)
    for (const scheme of ['light', 'dark'] as const) {
      const where = `${label} ${scheme}`
      const ctx = await browser.newContext({ viewport, colorScheme: scheme, reducedMotion: 'reduce', ...(label === 'phone' && { isMobile: true, hasTouch: true, deviceScaleFactor: 3 }) })
      let down = false
      const sockets: { close(o?: { code?: number; reason?: string }): Promise<void> }[] = []
      await ctx.route(/\/api\//, (r) => r.fulfill({ json: {} }))
      await ctx.route('**/api/session', async (r) => {
        if (!down) return r.fulfill({ json: { authed: true } })
        // the hub stopped: nothing answers, after a moment so the running step shows
        await new Promise((ok) => setTimeout(ok, 1500))
        await r.abort('connectionrefused').catch(() => {})
      })
      await ctx.routeWebSocket(/\/ws(\?|$)/, (ws) => {
        if (down) return void ws.close({ code: 1011, reason: '' }).catch(() => {})
        sockets.push(ws)
        ws.onMessage((m) => {
          const msg = JSON.parse(String(m))
          if (msg.t === 'sub.overview') ws.send(JSON.stringify({ t: 'overview', hosts: [host] }))
        })
      })
      const page = await ctx.newPage()
      await page.addInitScript(() => sessionStorage.setItem('inyon.intro.seen', '1'))
      await page.goto(base)
      await page.locator('text=Fix the login test >> visible=true').first().waitFor()
      // the hub goes away
      down = true
      for (const ws of sockets) await ws.close({ code: 1006 }).catch(() => {})
      const chip = page.getByRole('button', { name: 'Reconnecting' }).last()
      await chip.waitFor()
      await chip.click()
      const sheet = page.getByRole('dialog', { name: "Can't connect?" })
      await sheet.waitFor()
      // mid-run: the step that's out says what it's doing, nothing else on screen
      const running = sheet.getByRole('status')
      await running.waitFor()
      check((await running.textContent()) === 'Checking the hub answers…', `${where}: the running step said ${await running.textContent()}`)
      if (shots) await page.screenshot({ path: path.join(shots, `${label}-${scheme}-running.png`) })
      const fail = sheet.locator('[data-state="fail"]')
      await fail.waitFor({ timeout: 15_000 })
      const failText = (await fail.textContent()) ?? ''
      check(failText.includes("Couldn't connect to"), `${where}: the failing step says ${failText}`)
      check(failText.includes('Check the hub is running'), `${where}: the failing step has no fix: ${failText}`)
      check((await sheet.locator('[data-state="skip"]', { hasText: 'Skipped until the step above passes.' }).count()) === 3, `${where}: the steps after the failure weren't skipped`)
      check(!(await sheet.locator('pre').isVisible().catch(() => false)), `${where}: raw details show before Details is opened`)
      await page.waitForTimeout(400)
      await noSidewaysScroll(page, where)
      if (shots) await page.screenshot({ path: path.join(shots, `${label}-${scheme}.png`) })
      await sheet.getByRole('button', { name: 'Details' }).click()
      await page.waitForTimeout(400)
      check(((await sheet.locator('pre').textContent()) ?? '').includes('Inyon connection report'), `${where}: Details doesn't show the report`)
      await noSidewaysScroll(page, `${where} with details`)
      if (shots) {
        await sheet.locator('pre').scrollIntoViewIfNeeded()
        await page.screenshot({ path: path.join(shots, `${label}-${scheme}-details.png`) })
      }
      await ctx.close()
      console.log(`ok   ${where}`)
    }
} finally {
  await browser.close()
  server.stop(true)
}
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log("Can't connect? checked at 390x844 and 1440x900, light and dark")
