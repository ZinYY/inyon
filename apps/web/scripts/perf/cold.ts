// cold load of the built app on a throttled "phone": first paint, first content, main chunk parse, overview painted
import path from 'node:path'
import { chromium } from 'playwright'
import { PHONE, open, serve, settle } from '../readme/stage.ts'
const { base, stop } = serve(path.resolve('dist'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
for (const throttle of [1, 4]) {
  const times: number[] = []
  const again: number[] = []
  const paints: Record<string, number> = {}
  for (let i = 0; i < 3; i++) {
    const { ctx, page } = await open(browser, base, PHONE, 'light', 1)
    const cdp = await ctx.newCDPSession(page)
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: throttle })
    await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 40, downloadThroughput: 1.5e6 / 8 * 10, uploadThroughput: 750e3 / 8 })
    const t0 = Date.now()
    // what the first paint waits for: every file fetched before the overview shows
    const fetched: string[] = []
    page.on('response', (r) => void r.body().then((b) => fetched.push(`${r.url().split('/').pop()} ${(b.length / 1024).toFixed(0)}k`), () => {}))
    await page.goto(base + '/')
    await page.getByRole('main').getByText('Fix flaky checkout test').first().waitFor()
    times.push(Date.now() - t0)
    if (i === 0 && throttle === 1) console.log('first open fetched:', fetched.join(', '))
    // the second open: the worker has the app and the device has the last overview
    await page.waitForTimeout(1500)
    const t1 = Date.now()
    await page.reload()
    await page.getByRole('main').getByText('Fix flaky checkout test').first().waitFor()
    again.push(Date.now() - t1)
    const p = await page.evaluate(() => Object.fromEntries(performance.getEntriesByType('paint').map((e) => [e.name, Math.round(e.startTime)])))
    Object.assign(paints, p)
    await ctx.close()
  }
  console.log(`cpu x${throttle}: first open ${times.join('/')} ms, second open ${again.join('/')} ms`)
}
await browser.close(); stop()
