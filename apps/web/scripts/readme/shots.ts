// bun scripts/readme/shots.ts [name…]: makes the README's pictures in docs/ from the real app (dist/,
// so run `bun run build` first) fed the made-up world in demo.ts. With names, only those are made.
import path from 'node:path'
import { mkdirSync } from 'node:fs'
import type { Page } from 'playwright'
import { DESKTOP, PHONE, TABLET, launch, open, paneUrl, serve, settle, type Device } from './stage.ts'
import { PANES } from './demo.ts'
import { Take, render, still, type Placed } from './record.ts'

const root = path.join(import.meta.dir, '../../../..')
const docs = path.join(root, 'docs')
mkdirSync(docs, { recursive: true })
const { base, stop } = serve(path.join(import.meta.dir, '../../dist'))
const browser = await launch()

const main = (page: Page) => page.getByRole('main')

/** the screen of one page, as a PNG at 2× */
async function screen(device: Device, scheme: 'light' | 'dark', url: string, prep?: (page: Page) => Promise<void>) {
  const { ctx, page } = await open(browser, base, device, scheme)
  await page.goto(url)
  await settle(page, 1500)
  if (prep) await prep(page)
  const png = await page.screenshot({ type: 'png' })
  await ctx.close()
  return png
}

const jobs: Record<string, () => Promise<void>> = {
  /** desktop: answer Codex from the overview, then watch the result in the session */
  async answer() {
    const { ctx, page } = await open(browser, base, DESKTOP, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, DESKTOP)
    await take.start()
    take.caption('Every session on every machine, in one place')
    await take.wait(1600)
    const card = main(page).locator('div.rounded-3xl', { hasText: 'Run bun test src/auth?' }).first()
    take.caption('Codex is asking before it runs the tests')
    await take.lookAt(card, 1.75, 1000)
    await take.wait(700)
    await take.click(page.getByRole('button', { name: 'Yes, run it' }), { move: 900 })
    take.caption('Answer it from the overview')
    await take.wait(1300)
    await take.wide(900)
    take.caption('Then open it as a chat')
    await take.click(main(page).getByRole('link', { name: /Migrate auth to passkeys/ }).first(), { move: 900 })
    await take.wait(900)
    const last = main(page).getByText('All 31 auth tests pass', { exact: false })
    await last.waitFor({ timeout: 8000 })
    take.caption('The tests ran, and Codex carries on')
    await take.lookAt(last, 1.6, 1000)
    await take.wait(2000)
    await take.wide(900)
    await take.end(600)
    await ctx.close()
    console.log('answer', await render(browser, take, { scheme: 'light', width: 1600, height: 1060, pill: true }, path.join(docs, 'answer.webp'), { scale: 1, fps: 15, quality: 70 }))
  },

  /** phone: open a session, look at the change, watch the run finish, reply */
  async phone() {
    const { ctx, page, world } = await open(browser, base, PHONE, 'dark')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, PHONE)
    await take.start()
    take.step(0)
    await take.wait(1200)
    await take.scroll({ x: 195, y: 560 }, 380, 900)
    await take.wait(700)
    take.step(1)
    await take.click(main(page).getByRole('link', { name: /Fix flaky checkout test/ }).first())
    await take.wait(1400)
    take.step(2)
    const edit = main(page).getByRole('button', { name: /Edited/ }).first()
    await take.scroll({ x: 195, y: 420 }, -520, 900)
    await take.wait(400)
    await take.click(edit)
    await take.wait(500)
    await take.lookAt(main(page).locator('table').filter({ hasText: 'this.discount = rate(code)' }).first(), 1.9, 1000)
    await take.wait(1800)
    await take.wide(800)
    take.step(3)
    world.finishCheckout()
    await take.wait(600)
    await take.scroll({ x: 195, y: 420 }, 900, 900)
    await take.wait(1200)
    take.step(4)
    const box = page.getByRole('textbox', { name: /^Message/ })
    await take.click(box)
    await take.wait(300)
    await take.type('Add that regression test and commit it')
    await take.wait(300)
    await take.click(page.getByRole('button', { name: 'Send' }))
    await take.wait(2600)
    await take.end(1200)
    await ctx.close()
    console.log('phone', await render(browser, take, {
      scheme: 'dark', width: 1500, height: 1080, label: 'On your phone', title: 'Pick up any session',
      steps: ['See every session', 'Open one as a chat', 'Look at the change', 'Watch the run finish', 'Reply from anywhere'],
    }, path.join(docs, 'phone.webp'), { scale: 1.12, fps: 15, quality: 70 }))
  },

  /** tablet: find a session in the tree, then end it */
  async tablet() {
    const { ctx, page } = await open(browser, base, TABLET, 'light')
    await page.goto(base + '/')
    await settle(page)
    const take = new Take(page, TABLET)
    await take.start()
    take.caption('Sessions grouped by machine and tmux session')
    await take.wait(1200)
    await take.lookAt(page.getByRole('navigation').first(), 1.6, 1000)
    await take.wait(1200)
    await take.wide(800)
    take.caption('End one you are done with')
    const card = main(page).getByRole('button', { name: /Actions for Tidy the billing API types/ })
    await take.click(card)
    await take.wait(500)
    await take.click(page.getByRole('menuitem', { name: /End session/ }))
    await take.wait(400)
    const sheet = page.getByRole('dialog')
    take.caption('It says exactly what closes')
    await take.lookAt(sheet, 1.55, 900)
    await take.wait(1700)
    await take.wide(700)
    await take.click(sheet.getByRole('button', { name: 'End session' }))
    take.caption('The agent exits, then only its pane closes')
    await take.wait(2400)
    await take.end(800)
    await ctx.close()
    console.log('tablet', await render(browser, take, { scheme: 'light', width: 1500, height: 1080, pill: true }, path.join(docs, 'tablet.webp'), { scale: 1.08, fps: 15, quality: 70 }))
  },

  /** the picture at the top: desktop, tablet and phone, light and dark */
  async hero() {
    for (const scheme of ['light', 'dark'] as const) {
      const desk = await screen(DESKTOP, scheme, base + '/')
      const tab = await screen(TABLET, scheme, paneUrl(base, 'local', PANES.checkout))
      const ph = await screen(PHONE, scheme, paneUrl(base, 'local', PANES.passkeys))
      const placed: Placed[] = [
        { device: DESKTOP, png: desk, x: 40, y: 76, scale: 0.6 },
        { device: TABLET, png: tab, x: 610, y: 352, scale: 0.37 },
        { device: PHONE, png: ph, x: 968, y: 186, scale: 0.5 },
      ]
      console.log('hero', scheme, await still(browser, placed, { scheme, width: 1200, height: 690 }, path.join(docs, `hero-${scheme}.webp`)))
    }
  },

  /** smaller pictures further down */
  async gallery() {
    for (const scheme of ['light', 'dark'] as const) {
      const phones = [
        await screen(PHONE, scheme, base + '/'),
        await screen(PHONE, scheme, paneUrl(base, 'local', PANES.checkout)),
        await screen(PHONE, scheme, base + '/machines'),
      ]
      console.log('phones', scheme, await still(browser, phones.map((png, i) => ({ device: PHONE, png, x: 120 + i * 340, y: 74, scale: 0.72 })), { scheme, width: 1200, height: 720 }, path.join(docs, `phones-${scheme}.webp`)))
      const term = await screen(DESKTOP, scheme, paneUrl(base, 'local', PANES.checkout, 'terminal'))
      console.log('terminal', scheme, await still(browser, [{ device: DESKTOP, png: term, x: 60, y: 82, scale: 0.75 }], { scheme, width: 1200, height: 800 }, path.join(docs, `terminal-${scheme}.webp`)))
    }
  },
}

const names = process.argv.slice(2)
let failed = false
try {
  for (const [name, job] of Object.entries(jobs)) if (!names.length || names.includes(name)) await job()
} catch (e) {
  console.error(e)
  failed = true
} finally {
  // a browser whose GPU process crashed can hang on close
  await Promise.race([browser.close(), new Promise((r) => setTimeout(r, 5000))])
  stop()
  process.exit(failed ? 1 : 0)
}
