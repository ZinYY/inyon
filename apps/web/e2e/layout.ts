// bun e2e/layout.ts — opens the built app (dist/) in Chromium at phone widths, light and dark, fed
// made-up data, and fails when text drawn by the composer or a sent shell command leaves its box.
// Run `bun run build` first. CHROME=/path/to/chrome uses a browser other than Playwright's own.
import path from 'node:path'
import { chromium, devices } from 'playwright'

const dist = path.join(import.meta.dir, '../dist')
const server = Bun.serve({
  port: 0,
  async fetch(req) {
    const f = Bun.file(path.join(dist, new URL(req.url).pathname))
    // the app routes every other path itself
    return new Response((await f.exists()) && !new URL(req.url).pathname.endsWith('/') ? f : Bun.file(path.join(dist, 'index.html')))
  },
})
const base = `http://127.0.0.1:${server.port}`

const now = Date.now()
const host = {
  config: { id: 'box', name: 'studio', kind: 'local' }, status: 'online', hostname: 'studio', integration: 'installed', tmux: 'ok',
  agents: ['claude'], signedIn: ['claude'], os: 'Linux', latencyMs: 30, updatedAt: now, lastSeen: now,
  sessions: [{ id: '$1', name: 'app', attached: 0, windows: [{ id: '@1', index: 1, name: '', active: true, activity: now / 1000, panes: [
    { id: '%1', index: 0, active: true, command: 'claude', cwd: '/home/sam/app', title: '✳ Fix the login test', width: 80, height: 40, dead: false, agent: { kind: 'claude', pid: 1, state: 'idle', mode: 'default' } },
  ] }] }],
}
const ts = new Date(now).toISOString()
const longCommand = 'git log --oneline --graph --decorate --all --since="two weeks ago" -- apps/web/src/components/chat/Composer.tsx apps/web/src/components/chat/Messages.tsx'
const items = [
  { kind: 'user', seq: 1, uuid: 'u1', ts, text: 'ls -la', images: 0, shell: true },
  { kind: 'command', seq: 2, uuid: 'u2', ts, name: '!', args: 'ls -la', output: 'total 48\n-rw-r--r-- 1 sam sam 812 package.json' },
  { kind: 'user', seq: 3, uuid: 'u3', ts, text: longCommand, images: 0, shell: true },
  { kind: 'user', seq: 4, uuid: 'u4', ts, text: 'for f in *.ts; do\n  echo "$f"\ndone', images: 0, shell: true },
]
const info = { hostId: 'box', paneId: '%1', running: true, agent: 'claude', queue: [], tasks: [], subagents: [], artifacts: [], stats: { turns: 3, compactions: 0 } }
const typed = { short: '!ls -la', long: `!${longCommand}`, multi: '!for f in *.ts; do\n  echo "$f"\ndone' }

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const problems: string[] = []
let checks = 0
try {
  for (const width of [320, 375, 390, 430])
    for (const scheme of ['light', 'dark'] as const) {
      const ctx = await browser.newContext({ ...devices['iPhone 13'], viewport: { width, height: 760 }, colorScheme: scheme, reducedMotion: 'reduce' })
      await ctx.route(/\/api\//, (r) => r.fulfill({ json: {} }))
      await ctx.route('**/api/session', (r) => r.fulfill({ json: { authed: true } }))
      await ctx.routeWebSocket(/\/ws(\?|$)/, (ws) =>
        ws.onMessage((m) => {
          const msg = JSON.parse(String(m))
          if (msg.t === 'sub.overview') ws.send(JSON.stringify({ t: 'overview', hosts: [host] }))
          if (msg.t === 'sub.chat') ws.send(JSON.stringify({ t: 'chat.reset', key: `${msg.host}:${msg.pane}`, info, items, hasMore: false }))
        }),
      )
      const page = await ctx.newPage()
      await page.goto(`${base}/m/box/p/1`)
      const box = page.getByRole('textbox', { name: /^Message/ })
      await box.waitFor()
      await page.locator('.bg-bubble.font-mono').nth(2).waitFor()
      for (const [name, text] of Object.entries(typed)) {
        await box.fill(text)
        await page.waitForTimeout(100)
        // every line of text inside the composer clears its edges and its rounded corners
        const out: string[] = await page.evaluate(() => {
          const ta = document.querySelector('textarea[aria-label^="Message"]')!
          const frame = ta.closest('.rounded-3xl')!
          const b = frame.getBoundingClientRect()
          const r = parseFloat(getComputedStyle(frame).borderTopLeftRadius)
          const bad: string[] = []
          const walker = document.createTreeWalker(frame, NodeFilter.SHOW_TEXT)
          for (let n = walker.nextNode(); n; n = walker.nextNode()) {
            if (!n.textContent?.trim()) continue
            const range = document.createRange()
            range.selectNodeContents(n)
            for (const q of range.getClientRects()) {
              if (!q.width) continue
              const corner = (x: number, cx: number) => q.top < b.top + r && Math.hypot(x - cx, q.top - (b.top + r)) > r
              if (q.top < b.top || q.left < b.left || q.right > b.right || (q.left < b.left + r && corner(q.left, b.left + r)) || (q.right > b.right - r && corner(q.right, b.right - r)))
                bad.push(n.textContent.trim().slice(0, 40))
            }
          }
          return bad
        })
        checks++
        for (const t of out) problems.push(`${width}px ${scheme}, ${name} command: "${t}" leaves the composer`)
      }
      // sent shell commands wrap inside their bubble and keep their lines
      const bubbles = await page.locator('.bg-bubble.font-mono').evaluateAll((els) =>
        els.map((el) => ({ text: el.textContent!.slice(0, 30), over: el.scrollWidth - el.clientWidth, keepsLines: getComputedStyle(el).whiteSpace.startsWith('pre') })),
      )
      checks++
      for (const b of bubbles) {
        if (b.over > 0) problems.push(`${width}px ${scheme}: shell command "${b.text}" runs ${b.over}px out of its bubble`)
        if (!b.keepsLines) problems.push(`${width}px ${scheme}: shell command "${b.text}" loses its line breaks`)
      }
      await ctx.close()
    }
} finally {
  await browser.close()
  server.stop(true)
}

if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`layout: ${checks} checks at 320–430px, light and dark, all inside their boxes`)
