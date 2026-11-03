// bun scripts/launch/concepts.ts [out-dir]: the three launch-animation concepts as short recordings
// in a phone frame, light and dark, each handing off into the real overview (the made-up world).
import path from 'node:path'
import { mkdirSync, readFileSync } from 'node:fs'
import { chromium } from 'playwright'
import sharp from 'sharp'
import { PHONE, open, serve, settle } from '../readme/stage.ts'

const out = path.resolve(process.argv[2] ?? path.join(process.env.HOME!, 'development/inyon-recordings/launch'))
mkdirSync(out, { recursive: true })
const { base, stop } = serve(path.join(import.meta.dir, '../../dist'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const html = readFileSync(path.join(import.meta.dir, 'concept.html'), 'utf8')
const FPS = 30
const LENGTH_MS = 2000

try {
  for (const scheme of ['light', 'dark'] as const) {
    // the overview as the app shows it, for the hand-off
    const { ctx, page } = await open(browser, base, PHONE, scheme, 1)
    await page.goto(base + '/')
    await settle(page, 1200)
    const shot = `data:image/png;base64,${(await page.screenshot({ type: 'png' })).toString('base64')}`
    await ctx.close()

    for (const concept of ['a', 'b', 'c'] as const) {
      const c = await browser.newContext({ viewport: { width: PHONE.width, height: PHONE.height }, deviceScaleFactor: 2, colorScheme: scheme })
      const p = await c.newPage()
      await p.route('http://concept.local/**', (r) => r.fulfill({ contentType: 'text/html', body: html }))
      await p.goto(`http://concept.local/?c=${concept}&scheme=${scheme}`)
      const cdp = await c.newCDPSession(p)
      const frames: { at: number; png: Buffer }[] = []
      cdp.on('Page.screencastFrame', async (f) => {
        frames.push({ at: f.metadata.timestamp! * 1000, png: Buffer.from(f.data, 'base64') })
        await cdp.send('Page.screencastFrameAck', { sessionId: f.sessionId }).catch(() => {})
      })
      await cdp.send('Page.startScreencast', { format: 'png', everyNthFrame: 1 })
      await p.evaluate((s) => (window as any).start(s), shot)
      await p.waitForTimeout(LENGTH_MS)
      await cdp.send('Page.stopScreencast')
      await c.close()
      // even steps from the captured frames (the screencast only sends changed ones)
      const t0 = frames[0]!.at
      const steps = Math.round((LENGTH_MS / 1000) * FPS)
      const pick: Buffer[] = []
      for (let i = 0; i < steps; i++) {
        const t = t0 + (i * 1000) / FPS
        let f = frames[0]!
        for (const x of frames) if (x.at <= t) f = x
        pick.push(f.png)
      }
      // a run of identical frames becomes one frame shown longer
      const runs: { png: Buffer; n: number }[] = []
      for (const png of pick) {
        const last = runs[runs.length - 1]
        if (last && last.png.equals(png)) last.n++
        else runs.push({ png, n: 1 })
      }
      const file = path.join(out, `${concept}-${scheme}.webp`)
      const first = await sharp(pick[0]).metadata()
      await sharp(runs.map((r) => r.png), { join: { animated: true } })
        .webp({ quality: 80, effort: 5, loop: 0, delay: runs.map((r) => Math.round((r.n * 1000) / FPS)) })
        .toFile(file)
      console.log(`${concept} ${scheme}: ${frames.length} frames captured, ${first.width}x${first.height}, -> ${file}`)
    }
  }
} finally {
  await browser.close()
  stop()
}
