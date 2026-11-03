// bun scripts/perf/updates.ts [dist]: how smooth the overview stays while the hub keeps sending
// updates. One pane's activity line changes 40 times, 100 ms apart, on a throttled phone profile;
// the page records the gap between frames the whole time.
import path from 'node:path'
import { chromium } from 'playwright'
import { PHONE, open, serve, settle } from '../readme/stage.ts'
import { PANES } from '../readme/demo.ts'

const dist = path.resolve(process.argv[2] ?? 'dist')
const { base, stop } = serve(dist)
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
try {
  for (const throttle of [1, 4]) {
    const { ctx, page, world } = await open(browser, base, PHONE, 'light', 1)
    const cdp = await ctx.newCDPSession(page)
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: throttle })
    await page.goto(base + '/')
    await settle(page, 1200)
    // main-thread time the burst costs, which doesn't depend on how busy this machine is
    await cdp.send('Performance.enable')
    const metrics = async () => Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map((m) => [m.name, m.value]))
    const m0 = await metrics()
    if (process.env.PROFILE) {
      await cdp.send('Profiler.enable')
      await cdp.send('Profiler.start')
    }
    await page.evaluate(() => {
      const gaps: number[] = []
      let last = performance.now()
      const tick = (t: number) => {
        gaps.push(t - last)
        last = t
        ;(window as any).__gaps = gaps
        requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    })
    const pane = world.pane(PANES.checkout)!.p
    for (let i = 0; i < 40; i++) {
      pane.agent.activity = `Running the checkout suite… (${2 + i}m ${(i * 7) % 60}s · ↓ ${3.1 + i / 10}k tokens)`
      world.pushOverview()
      await page.waitForTimeout(100)
    }
    const m1 = await metrics()
    if (process.env.PROFILE) {
      // where the time goes: functions by their own time, with the file they live in
      const { profile } = await cdp.send('Profiler.stop')
      const self = new Map<string, number>()
      const dt = profile.timeDeltas ?? []
      const byId = new Map(profile.nodes.map((n) => [n.id, n]))
      profile.samples?.forEach((id, i) => {
        const n = byId.get(id)!
        const f = n.callFrame
        const key = `${f.functionName || '(anonymous)'} ${f.url.split('/').pop()}:${f.lineNumber}`
        self.set(key, (self.get(key) ?? 0) + (dt[i] ?? 0) / 1000)
      })
      console.log(`  samples ${profile.samples?.length ?? 0}, nodes ${profile.nodes.length}, deltas ${dt.length}`)
      const top = [...self].sort((a, b) => b[1] - a[1]).slice(0, 18)
      for (const [k, v] of top) console.log(`  ${v.toFixed(1).padStart(7)} ms  ${k}`)
    }
    const ms = (k: string) => (((m1[k] ?? 0) - (m0[k] ?? 0)) * 1000).toFixed(0)
    console.log(`cpu x${throttle}: per 40 updates, script ${ms('ScriptDuration')} ms, style ${ms('RecalcStyleDuration')} ms, layout ${ms('LayoutDuration')} ms, tasks ${ms('TaskDuration')} ms`)
    const gaps: number[] = await page.evaluate(() => (window as any).__gaps.slice(5))
    const sorted = [...gaps].sort((a, b) => a - b)
    const q = (p: number) => sorted[Math.floor(p * (sorted.length - 1))]!.toFixed(1)
    const long = gaps.filter((g) => g > 16.7).length
    console.log(`cpu x${throttle}: frames ${gaps.length}, p50 ${q(0.5)} ms, p95 ${q(0.95)} ms, max ${q(1)} ms, over 16.7 ms: ${long}`)
    await ctx.close()
  }
} finally {
  await browser.close()
  stop()
}
