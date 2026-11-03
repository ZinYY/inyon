// Records a walkthrough of the real app and turns it into a framed, animated WebP.
//
// The page is captured with Chrome's screencast while a script clicks, taps, types and scrolls with
// real input events. The script also logs where the cursor went, each tap, and where the camera
// should look, on the same clock. Each output frame is then drawn by frame.html: the silver stage,
// the device, the screen as captured at that moment, a cursor or touch ring, and the camera's zoom.
import { createHash } from 'node:crypto'
import path from 'node:path'
import sharp from 'sharp'
import type { Browser, Page } from 'playwright'
import { launch, type Device } from './stage.ts'

type Pt = { x: number; y: number }
type Move = { t0: number; t1: number; from: Pt; to: Pt }
type Shot = { t: number; data: Buffer }
/** a camera position: the point of the page in the middle of the picture, and how far in */
type Cam = { t0: number; t1: number; from: View; to: View }
type View = { x: number; y: number; zoom: number }

const ease = (k: number) => (k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2)
const lerp = (a: number, b: number, k: number) => a + (b - a) * k
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export class Take {
  shots: Shot[] = []
  moves: Move[] = []
  cams: Cam[] = []
  presses: { t: number; up?: number; at: Pt }[] = []
  captions: { t: number; text: string }[] = []
  steps: { t: number; n: number }[] = []
  t0 = 0
  pos: Pt
  view: View
  private stop?: () => Promise<void>

  constructor(
    public page: Page,
    public device: Device,
  ) {
    this.pos = { x: device.width * 0.62, y: device.height * 0.72 }
    this.view = { x: device.width / 2, y: device.height / 2, zoom: 1 }
  }

  now() {
    return performance.now() - this.t0
  }

  async start() {
    const cdp = await this.page.context().newCDPSession(this.page)
    cdp.on('Page.screencastFrame', (f: any) => {
      this.shots.push({ t: this.now(), data: Buffer.from(f.data, 'base64') })
      cdp.send('Page.screencastFrameAck', { sessionId: f.sessionId }).catch(() => {})
    })
    const scale = await this.page.evaluate(() => devicePixelRatio)
    this.t0 = performance.now()
    await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 92, maxWidth: this.device.width * scale, maxHeight: this.device.height * scale, everyNthFrame: 1 })
    this.stop = async () => {
      await cdp.send('Page.stopScreencast').catch(() => {})
      await cdp.detach().catch(() => {})
    }
    if (!this.device.touch) await this.page.mouse.move(this.pos.x, this.pos.y)
  }

  async end(hold = 800) {
    await sleep(hold)
    this.endAt = this.now()
    await this.stop?.()
  }
  endAt = 0

  async wait(ms: number) {
    await sleep(ms)
  }

  /** glides the pointer to a point (the page sees every step, so hovers show) */
  async moveTo(to: Pt, ms = 700) {
    const from = { ...this.pos }
    const t0 = this.now()
    this.moves.push({ t0, t1: t0 + ms, from, to })
    const steps = Math.max(2, Math.round(ms / 25))
    for (let i = 1; i <= steps; i++) {
      const k = ease(i / steps)
      if (!this.device.touch) await this.page.mouse.move(lerp(from.x, to.x, k), lerp(from.y, to.y, k))
      await sleep(ms / steps)
    }
    this.pos = to
  }

  /** the centre of an element, in page coordinates */
  async center(sel: ReturnType<Page['locator']>): Promise<Pt> {
    await sel.scrollIntoViewIfNeeded()
    const b = (await sel.boundingBox())!
    return { x: b.x + b.width / 2, y: b.y + b.height / 2 }
  }

  /** moves to an element and clicks it (or taps it, on a touch device) */
  async click(sel: ReturnType<Page['locator']>, o: { move?: number; hold?: number } = {}) {
    const at = await this.center(sel)
    if (this.device.touch) {
      this.pos = at
      const press = { t: this.now(), at, up: 0 }
      this.presses.push(press)
      await sleep(o.hold ?? 140)
      press.up = this.now()
      await this.page.touchscreen.tap(at.x, at.y)
    } else {
      await this.moveTo(at, o.move ?? 700)
      await sleep(120)
      const press = { t: this.now(), at, up: 0 }
      this.presses.push(press)
      await this.page.mouse.down()
      await sleep(o.hold ?? 110)
      await this.page.mouse.up()
      press.up = this.now()
    }
  }

  /** types like a person, a character at a time */
  async type(text: string, perChar = 55) {
    for (const ch of text) {
      await this.page.keyboard.type(ch)
      await sleep(perChar + (ch === ' ' ? 40 : 0))
    }
  }

  /** scrolls the page under a point; on a touch device it shows as a swipe */
  async scroll(at: Pt, dy: number, ms = 900) {
    const steps = Math.round(ms / 30)
    if (this.device.touch) {
      const t0 = this.now()
      // a finger drags the other way to the scroll, and stays on the screen
      const reach = Math.min(260, Math.abs(dy) / 2)
      const clampY = (y: number) => Math.min(Math.max(y, 90), this.device.height - 120)
      const from = { x: at.x, y: clampY(at.y + (Math.sign(dy) * reach) / 2) }
      const to = { x: at.x, y: clampY(from.y - Math.sign(dy) * reach) }
      this.presses.push({ t: t0, at: from, up: t0 + ms })
      this.moves.push({ t0, t1: t0 + ms, from, to })
      this.pos = to
    } else await this.moveTo(at, 400)
    await this.page.mouse.move(at.x, at.y)
    for (let i = 0; i < steps; i++) {
      await this.page.mouse.wheel(0, dy / steps)
      await sleep(ms / steps)
    }
  }

  /** a real finger drag (touch events, not a wheel), for things that listen to touches like pull to refresh */
  async drag(from: Pt, to: Pt, ms = 900, hold = 150) {
    const cdp = (this.cdp ??= await this.page.context().newCDPSession(this.page))
    const t0 = this.now()
    this.presses.push({ t: t0, at: from, up: t0 + ms + hold })
    this.moves.push({ t0, t1: t0 + ms, from, to })
    this.pos = to
    const steps = Math.round(ms / 30)
    const touch = (type: 'touchStart' | 'touchMove' | 'touchEnd', p?: Pt) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: p ? [{ x: p.x, y: p.y }] : [] })
    await touch('touchStart', from)
    for (let i = 1; i <= steps; i++) {
      const k = i / steps
      const e = 1 - (1 - k) * (1 - k)
      await touch('touchMove', { x: from.x + (to.x - from.x) * e, y: from.y + (to.y - from.y) * e })
      await sleep(ms / steps)
    }
    await sleep(hold)
    await touch('touchEnd')
  }
  private cdp?: Awaited<ReturnType<ReturnType<Page['context']>['newCDPSession']>>

  /** points the camera at a part of the page (zoom 1 shows the whole device) */
  async look(to: View, ms = 900, wait = true) {
    const t0 = this.now()
    this.cams.push({ t0, t1: t0 + ms, from: { ...this.view }, to })
    this.view = to
    if (wait) await sleep(ms)
  }
  async lookAt(sel: ReturnType<Page['locator']>, zoom: number, ms = 900, wait = true) {
    const c = await this.center(sel)
    await this.look({ ...c, zoom }, ms, wait)
  }
  async wide(ms = 900, wait = true) {
    await this.look({ x: this.device.width / 2, y: this.device.height / 2, zoom: 1 }, ms, wait)
  }

  /** the line shown under the picture from now on (empty hides it) */
  caption(text: string) {
    this.captions.push({ t: this.now(), text })
  }
  /** which numbered step is under way */
  step(n: number) {
    this.steps.push({ t: this.now(), n })
  }

  // ---- reading the timeline back ----

  captionAt(t: number) {
    let c = ''
    for (const x of this.captions) if (x.t <= t) c = x.text
    return c
  }
  stepAt(t: number) {
    let n = -1
    for (const x of this.steps) if (x.t <= t) n = x.n
    return n
  }

  shotAt(t: number) {
    let best = this.shots[0]
    for (const s of this.shots) if (s.t <= t) best = s
    return best
  }
  pointerAt(t: number): Pt {
    let p = this.moves.length ? this.moves[0].from : this.pos
    for (const m of this.moves) {
      if (t >= m.t1) p = m.to
      else if (t >= m.t0) return { x: lerp(m.from.x, m.to.x, ease((t - m.t0) / (m.t1 - m.t0))), y: lerp(m.from.y, m.to.y, ease((t - m.t0) / (m.t1 - m.t0))) }
    }
    return p
  }
  viewAt(t: number): View {
    let v = this.cams.length ? this.cams[0].from : this.view
    for (const c of this.cams) {
      if (t >= c.t1) v = c.to
      else if (t >= c.t0) {
        const k = ease((t - c.t0) / (c.t1 - c.t0))
        return { x: lerp(c.from.x, c.to.x, k), y: lerp(c.from.y, c.to.y, k), zoom: lerp(c.from.zoom, c.to.zoom, k) }
      }
    }
    return v
  }
  /** 0 when no press is near; for a press: how far into it (down) and into its ring (after up) */
  pressAt(t: number) {
    for (const p of [...this.presses].reverse()) {
      if (t < p.t) continue
      const up = p.up || p.t + 120
      if (t <= up) return { down: true, ring: 0, at: this.device.touch ? this.pointerAt(t) : p.at }
      const k = (t - up) / 450
      if (k <= 1) return { down: false, ring: k, at: this.device.touch ? this.pointerAt(up) : p.at }
      return undefined
    }
    return undefined
  }
}

export interface FrameStyle {
  scheme: 'light' | 'dark'
  /** captions in a pill under the device */
  pill?: boolean
  /** a numbered list beside the device instead, with this label and title */
  steps?: string[]
  label?: string
  title?: string
  /** the picture's size in pixels */
  width: number
  height: number
  /** the address shown in a browser frame */
  url?: string
}

/**
 * Renders a Take into an animated WebP. `scale` is how many picture pixels one page pixel takes when
 * the camera is wide.
 */
export async function render(browser: Browser, take: Take, style: FrameStyle, out: string, o: { fps?: number; quality?: number; scale: number }) {
  const fps = o.fps ?? 20
  const frames = new Map<string, Buffer>()
  // Chrome on some ARM servers crashes or hangs after a hundred or so captures, at a different
  // frame each run, so either starts a fresh browser that carries on from the same frame
  let own: Browser | undefined
  let page: Page
  const open = async (b: Browser) => {
    const ctx = await b.newContext({ viewport: { width: style.width, height: style.height }, deviceScaleFactor: 1 })
    await ctx.route('http://frame.local/**', async (r) => {
      const u = new URL(r.request().url())
      if (u.pathname === '/') return r.fulfill({ path: path.join(import.meta.dir, 'frame.html'), contentType: 'text/html' })
      const f = frames.get(u.pathname.slice(1))
      return f ? r.fulfill({ body: f, contentType: 'image/jpeg' }) : r.fulfill({ status: 404 })
    })
    const p = await ctx.newPage()
    await p.goto('http://frame.local/')
    await p.evaluate((s: any) => (window as any).setup(s), { ...style, device: take.device, scale: o.scale })
    return p
  }
  page = await open(browser)

  const count = Math.ceil((take.endAt / 1000) * fps)
  const out_: { png: Buffer; hash: string; n: number }[] = []
  for (let i = 0; i < count; i++) {
    const t = (i * 1000) / fps
    const shot = take.shotAt(t)
    const key = `s${take.shots.indexOf(shot)}`
    frames.set(key, shot.data)
    const st = { src: `http://frame.local/${key}`, view: take.viewAt(t), pointer: take.pointerAt(t), press: take.pressAt(t), caption: take.captionAt(t), step: take.stepAt(t) }
    let png: Buffer | undefined
    for (let tries = 0; !png; tries++) {
      try {
        // a page can also hang instead of crashing
        const hung = new Promise<never>((_, no) => setTimeout(() => no(new Error('frame timed out')), 15000))
        png = await Promise.race([page.evaluate(async (st: any) => (window as any).draw(st), st).then(() => page.screenshot({ type: 'png', timeout: 15000 })), hung])
      } catch (e) {
        if (tries >= 3) throw e
        void own?.close().catch(() => {})
        own = await launch()
        page = await open(own)
      }
    }
    const hash = createHash('sha1').update(png).digest('hex')
    const last = out_[out_.length - 1]
    // a still moment is one frame shown for longer
    if (last && last.hash === hash) last.n++
    else out_.push({ png, hash, n: 1 })
  }
  await Promise.race([page.context().close().then(() => own?.close()), sleep(5000)]).catch(() => {})
  const webp = await sharp(out_.map((f) => f.png), { join: { animated: true } })
    .webp({ quality: o.quality ?? 82, effort: 5, loop: 0, delay: out_.map((f) => Math.round((f.n * 1000) / fps)), smartSubsample: true })
    .toBuffer()
  await Bun.write(out, webp)
  return { frames: out_.length, seconds: take.endAt / 1000, kb: Math.round(webp.length / 1024) }
}

export interface Placed {
  device: Device
  png: Buffer
  /** where the screen's top-left corner goes, and picture px per page px */
  x: number
  y: number
  scale: number
}

/** A still: devices placed on the stage, saved as a WebP. */
export async function still(browser: Browser, screens: Placed[], style: FrameStyle, out: string, scale = 2) {
  const ctx = await browser.newContext({ viewport: { width: style.width, height: style.height }, deviceScaleFactor: scale })
  const files = new Map(screens.map((s, i) => [`f${i}`, s.png]))
  await ctx.route('http://frame.local/**', async (r) => {
    const u = new URL(r.request().url())
    if (u.pathname === '/') return r.fulfill({ path: path.join(import.meta.dir, 'frame.html'), contentType: 'text/html' })
    const f = files.get(u.pathname.slice(1))
    return f ? r.fulfill({ body: f, contentType: 'image/png' }) : r.fulfill({ status: 404 })
  })
  const page = await ctx.newPage()
  await page.goto('http://frame.local/')
  await page.evaluate((s: any) => (window as any).setupStill(s), { ...style, screens: screens.map((s, i) => ({ device: s.device, x: s.x, y: s.y, scale: s.scale, src: `http://frame.local/f${i}` })) })
  const png = await page.screenshot({ type: 'png' })
  await ctx.close()
  const webp = await sharp(png).webp({ quality: 90, effort: 6, smartSubsample: true }).toBuffer()
  await Bun.write(out, webp)
  return { kb: Math.round(webp.length / 1024) }
}
