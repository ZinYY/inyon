// bun scripts/readme/intro-check.ts: the intro on a cold start, phone and desktop, light and dark.
// Records each as a short clip for review and checks that the mark lands on the header's mark to
// the pixel before the real one takes over.
import path from 'node:path'
import { mkdirSync } from 'node:fs'
import { chromium } from 'playwright'
import sharp from 'sharp'
import { DESKTOP, PHONE, open, serve } from './stage.ts'

const out = path.resolve(process.argv[2] ?? path.join(process.env.HOME!, 'development/inyon-recordings/0.13'))
mkdirSync(out, { recursive: true })
const { base, stop } = serve(path.join(import.meta.dir, '../../dist'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
let failed = false
try {
  for (const device of [PHONE, DESKTOP]) {
    for (const scheme of ['light', 'dark'] as const) {
      const { ctx, page } = await open(browser, base, device, scheme, 1)
      // a cold start: nothing seen yet in this session
      await page.addInitScript(() => sessionStorage.removeItem('inyon.intro.seen'))
      const cdp = await ctx.newCDPSession(page)
      const frames: { at: number; png: Buffer }[] = []
      cdp.on('Page.screencastFrame', async (f) => {
        frames.push({ at: f.metadata.timestamp! * 1000, png: Buffer.from(f.data, 'base64') })
        await cdp.send('Page.screencastFrameAck', { sessionId: f.sessionId }).catch(() => {})
      })
      await cdp.send('Page.startScreencast', { format: 'png', everyNthFrame: 1, maxWidth: 800, maxHeight: 800 })
      await page.goto(base + '/')
      // where the intro's mark is when it has flown, and where the header's mark is
      const landing = await page.evaluate(
        () =>
          new Promise<{ mark: DOMRect; target: DOMRect } | null>((resolve) => {
            // the mark's place on every frame while it flies; the last one before the overlay goes is where it landed
            const start = Date.now()
            let last: { mark: DOMRect; target: DOMRect } | null = null
            const tick = () => {
              const intro = document.querySelector('.intro')
              if (!intro) return resolve(last)
              const mark = document.querySelector('.intro-off .intro-mark')
              if (mark) {
                const t = [...document.querySelectorAll('[data-intro-target]')].map((e) => e.getBoundingClientRect()).find((r) => r.width > 0)
                if (t) last = { mark: mark.getBoundingClientRect().toJSON(), target: t.toJSON() }
              }
              if (Date.now() - start > 6000) return resolve(last)
              requestAnimationFrame(tick)
            }
            tick()
          }),
      )
      await page.waitForTimeout(800)
      await cdp.send('Page.stopScreencast')
      const gone = await page.evaluate(() => !document.querySelector('.intro') && !document.documentElement.classList.contains('intro-running'))
      await ctx.close()
      const name = `intro-${device === PHONE ? 'phone' : 'desktop'}-${scheme}`
      if (!landing) {
        console.log(`${name}: FAIL, no landing measured`)
        failed = true
      } else {
        const d = Math.hypot(landing.mark.left - landing.target.left, landing.mark.top - landing.target.top)
        const ds = Math.abs(landing.mark.width - landing.target.width)
        const ok = d <= 1.5 && ds <= 1.5 && gone
        if (!ok) failed = true
        console.log(`${name}: ${ok ? 'ok' : 'FAIL'} landed ${d.toFixed(2)} px off, size off by ${ds.toFixed(2)} px, overlay gone: ${gone}`)
      }
      // the clip, 30 fps, deduped
      const t0 = frames[0]!.at
      const steps = Math.round(((frames.at(-1)!.at - t0) / 1000) * 30)
      const runs: { png: Buffer; n: number }[] = []
      for (let i = 0; i < steps; i++) {
        const t = t0 + (i * 1000) / 30
        let f = frames[0]!
        for (const x of frames) if (x.at <= t) f = x
        const last = runs[runs.length - 1]
        if (last && last.png.equals(f.png)) last.n++
        else runs.push({ png: f.png, n: 1 })
      }
      await sharp(runs.map((r) => r.png), { join: { animated: true } })
        .webp({ quality: 80, effort: 5, loop: 0, delay: runs.map((r) => Math.round((r.n * 1000) / 30)) })
        .toFile(path.join(out, `${name}.webp`))
    }
  }
} finally {
  await browser.close()
  stop()
}
if (failed) process.exit(1)
