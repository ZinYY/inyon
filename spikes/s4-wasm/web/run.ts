// Builds nothing: serves web/ (with pkg/ from build.sh), opens it in headless Chromium and
// prints what the page measured. Exits 1 if a check failed.
import { createRequire } from 'node:module'
import { startServer } from './server.ts'

const require = createRequire(import.meta.dir + '/../../../apps/web/package.json')
const { chromium } = require('playwright')
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-linux-arm64/chrome-headless-shell'

const server = startServer()
const browser = await chromium.launch({ executablePath: process.env.S4_CHROME ?? exe, args: ['--disable-gpu'] })
const results = []
try {
  // twice: the second load shows startup with the module already compiled and cached
  for (const run of ['cold', 'warm']) {
    const page = await browser.newPage()
    page.on('pageerror', (e: Error) => console.error('page error:', e.message))
    page.on('console', (m: any) => m.type() === 'error' && console.error('console:', m.text()))
    const t0 = performance.now()
    await page.goto(`http://127.0.0.1:${server.port}/`)
    await page.waitForFunction(() => (window as any).s4, null, { timeout: 60000 })
    const r = await page.evaluate(() => (window as any).s4)
    r.run = run
    r.pageToDoneMs = +(performance.now() - t0).toFixed(1)
    results.push(r)
    await page.close()
  }
} finally {
  await browser.close()
  server.stop(true)
}
console.log(JSON.stringify(results, null, 2))
process.exit(results.every((r) => r.ok) ? 0 : 1)
