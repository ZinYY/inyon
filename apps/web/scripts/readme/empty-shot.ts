// bun scripts/readme/empty-shot.ts: the overview with nothing on it (the hosted build, no computers), light and dark
import path from 'node:path'
import { chromium } from 'playwright'
import { PHONE, open, serve, settle } from './stage.ts'
const out = path.join(process.env.HOME!, 'development/inyon-recordings/0.13')
const hosted = serve(path.join(import.meta.dir, '../../dist-hosted'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
try {
  for (const scheme of ['light', 'dark'] as const) {
    const { ctx, page } = await open(browser, hosted.base, PHONE, scheme, 1)
    await page.addInitScript(() => sessionStorage.setItem('inyon.intro.seen', '1'))
    await page.goto(hosted.base + '/')
    await settle(page, 1500)
    await page.screenshot({ path: path.join(out, `empty-${scheme}.png`) })
    await ctx.close()
  }
} finally {
  await browser.close()
  hosted.stop()
}
