// bun scripts/readme/motion.ts [out-dir]: short recordings of the 0.11.1 motion on the phone
// profile, for review. Built app (dist/) plus the made-up world, as the README pictures.
import path from 'node:path'
import { mkdirSync } from 'node:fs'
import { chromium } from 'playwright'
import { PHONE, open, serve, settle } from './stage.ts'
import { Take, render } from './record.ts'

const out = path.resolve(process.argv[2] ?? path.join(process.env.HOME!, 'development/inyon-recordings/0.11.1'))
mkdirSync(out, { recursive: true })
const { base, stop } = serve(path.join(import.meta.dir, '../../dist'))
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
const style = { scheme: 'light' as const, width: 900, height: 1080, pill: true }
const opts = { scale: 1, fps: 20, quality: 70 }

const clips: Record<string, () => Promise<void>> = {
  /** an answer: Sending, Sent, then the card fades and the list closes up */
  async answer() {
    const { ctx, page } = await open(browser, base, PHONE, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, PHONE)
    await take.start()
    take.caption('Codex is asking')
    await take.wait(900)
    await take.click(page.getByRole('button', { name: 'Yes, run it' }))
    take.caption('Sending, then sent; two short pulses once Codex took it')
    await take.wait(2200)
    take.caption('The card fades, and the others slide up')
    await take.wait(1800)
    await take.end(600)
    await ctx.close()
    console.log('answer', await render(browser, take, style, path.join(out, 'answer.webp'), opts))
  },

  /** pull to refresh: the mark follows the finger, tilts, and turns once */
  async pull() {
    const { ctx, page } = await open(browser, base, PHONE, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, PHONE)
    await take.start()
    take.caption('Pull down: the mark comes with the finger')
    await take.wait(700)
    await take.drag({ x: 195, y: 230 }, { x: 195, y: 420 }, 1100, 250)
    take.caption('Let go: it turns once while the page reloads')
    await take.wait(1600)
    await take.end(500)
    await ctx.close()
    console.log('pull', await render(browser, take, style, path.join(out, 'pull.webp'), opts))
  },

  /** a session finishing: its card changes state and moves, without a jump */
  async reorder() {
    const { ctx, page, world } = await open(browser, base, PHONE, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, PHONE)
    await take.start()
    take.caption('A working session finishes')
    await take.scroll({ x: 195, y: 560 }, 420, 900)
    await take.wait(600)
    world.finishCheckout()
    take.caption('Its card settles into place; nothing jumps')
    await take.wait(2200)
    await take.end(500)
    await ctx.close()
    console.log('reorder', await render(browser, take, style, path.join(out, 'reorder.webp'), opts))
  },

  /** the terminal rising in over the chat, and the chat skeleton that says what it waits for */
  async terminal() {
    const { ctx, page } = await open(browser, base, PHONE, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, PHONE)
    await take.start()
    take.caption('Open a session')
    await take.scroll({ x: 195, y: 560 }, 380, 800)
    await take.click(page.getByRole('main').getByRole('link', { name: /Fix flaky checkout test/ }).first())
    await take.wait(1400)
    take.caption('Terminal: it rises in over the chat')
    await take.click(page.getByRole('button', { name: /Terminal/ }).first())
    await take.wait(1800)
    await take.end(500)
    await ctx.close()
    console.log('terminal', await render(browser, take, style, path.join(out, 'terminal.webp'), opts))
  },
}

const names = process.argv.slice(3)
try {
  for (const [name, clip] of Object.entries(clips)) if (!names.length || names.includes(name)) await clip()
} finally {
  await browser.close()
  stop()
}
