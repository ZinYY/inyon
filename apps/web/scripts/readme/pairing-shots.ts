// bun scripts/readme/pairing-shots.ts [out-dir]: the two sides of hub pairing as stills, light and
// dark: "Open on your phone" on the hub, and "Connect to a hub" in the app.
import path from 'node:path'
import { mkdirSync } from 'node:fs'
import { chromium } from 'playwright'
import { DESKTOP, PHONE, open, serve, settle } from './stage.ts'

const out = path.resolve(process.argv[2] ?? path.join(process.env.HOME!, 'development/inyon-recordings/0.12'))
mkdirSync(out, { recursive: true })
const { base, stop } = serve(path.join(import.meta.dir, '../../dist'))
// the app side is the hosted build: the phone's core runs in the page, so Add offers a hub
const hosted = serve(path.join(import.meta.dir, '../../dist-hosted'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const code = { id: 'K7Q4MX2T', code: '493812', link: `inyon://hub?${new URLSearchParams({ u: 'http://studio:4777', c: 'K7Q4MX2T:493812', n: 'studio' })}`, expiresAt: Date.now() + 600_000 }
try {
  for (const scheme of ['light', 'dark'] as const) {
    // the hub's web app
    const { ctx, page } = await open(browser, base, DESKTOP, scheme, 1)
    await page.route('**/api/phones/pair', (r) => r.fulfill({ json: code }))
    await page.route('**/api/phones', (r) => r.fulfill({ json: [] }))
    await page.goto(base + '/machines')
    await settle(page, 800)
    await page.getByRole('button', { name: 'Phone' }).click()
    await page.getByText('K7Q4-MX2T').waitFor()
    await page.waitForTimeout(600)
    await page.screenshot({ path: path.join(out, `hub-${scheme}.png`) })
    await ctx.close()

    // the app
    const p = await open(browser, hosted.base, PHONE, scheme, 1)
    await p.page.goto(hosted.base + '/machines')
    await settle(p.page, 800)
    await p.page.getByRole('button', { name: 'Add' }).click()
    await p.page.getByRole('button', { name: /Connect to a hub/ }).waitFor()
    await p.page.waitForTimeout(500)
    await p.page.screenshot({ path: path.join(out, `app-add-${scheme}.png`) })
    await p.page.getByRole('button', { name: /Connect to a hub/ }).click()
    await p.page.getByLabel('Hub address').fill('studio:4777')
    await p.page.getByLabel('Hub code').fill('K7Q4-MX2T 493812')
    await p.page.waitForTimeout(500)
    await p.page.screenshot({ path: path.join(out, `app-hub-${scheme}.png`) })
    await p.ctx.close()
    console.log(scheme, 'done')
  }
} finally {
  await browser.close()
  stop()
  hosted.stop()
}
