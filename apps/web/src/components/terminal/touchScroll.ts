/**
 * Phones: a finger dragged up or down the terminal scrolls it the way a mouse wheel does. The
 * tmux view takes wheel reports; PSmux also keeps history in xterm's normal buffer. Scroll that
 * buffer directly when no program has requested the mouse. Synthetic wheel events do not reliably
 * scroll the viewport on touch devices. Full-screen programs and tmux still get wheel events.
 * Taps and sideways drags are left alone; a flick keeps scrolling for a moment.
 */
import type { IBuffer, IModes, Terminal } from '@xterm/xterm'

type ScrollTerminal = Pick<Terminal, 'rows' | 'scrollLines'> & {
  readonly modes: Pick<IModes, 'mouseTrackingMode'>
  readonly buffer: { readonly active: Pick<IBuffer, 'type'> }
}

/** Finger travel, in rows, for one wheel step (tmux scrolls a few lines per step). */
const ROWS_PER_STEP = 2
/** Travel before a touch counts as a drag rather than a tap. */
const SLOP = 8

export function touchScroll(host: HTMLElement, term: ScrollTerminal) {
  let startX = 0
  let startY = 0
  let lastY = 0
  let lastT = 0
  let carried = 0
  let dragging = false
  let velocity = 0
  let fling = 0

  const target = () => host.querySelector<HTMLElement>('.xterm-screen') ?? host
  const localHistory = () => term.modes.mouseTrackingMode === 'none' && term.buffer.active.type === 'normal'

  /** One wheel step: down (newer) for a positive direction, up into the history for a negative one. */
  const wheel = (dir: 1 | -1, x: number, y: number) => {
    target().dispatchEvent(new WheelEvent('wheel', { deltaY: dir, deltaMode: 1, clientX: x, clientY: y, bubbles: true, cancelable: true }))
  }

  /** Turns pixels of travel into whole steps, keeping the remainder for the next move. */
  const travel = (px: number, x: number, y: number) => {
    carried += px
    // Read the mode on every move: an agent can enter or leave its full-screen UI without
    // reopening the terminal. Local history follows the finger one row at a time.
    const local = localHistory()
    const step = Math.max(8, (host.clientHeight / Math.max(1, term.rows)) * (local ? 1 : ROWS_PER_STEP))
    while (Math.abs(carried) >= step) {
      const dir = carried > 0 ? 1 : -1
      if (local) term.scrollLines(dir)
      else wheel(dir, x, y)
      carried -= dir * step
    }
  }

  const stop = () => cancelAnimationFrame(fling)

  const onStart = (e: TouchEvent) => {
    stop()
    if (e.touches.length !== 1) return (dragging = false), undefined
    const t = e.touches[0]!
    startX = t.clientX
    startY = lastY = t.clientY
    lastT = e.timeStamp
    carried = 0
    velocity = 0
    dragging = false
  }

  const onMove = (e: TouchEvent) => {
    if (e.touches.length !== 1) return
    const t = e.touches[0]!
    if (!dragging) {
      const dx = Math.abs(t.clientX - startX)
      const dy = Math.abs(t.clientY - startY)
      if (dy < SLOP || dy < dx) return
      dragging = true
    }
    // the drag is ours: the page doesn't scroll and the terminal's own touch handling stays out
    e.preventDefault()
    e.stopPropagation()
    // finger up shows newer lines, as on any phone list
    const px = lastY - t.clientY
    const dt = Math.max(1, e.timeStamp - lastT)
    velocity = 0.8 * (px / dt) + 0.2 * velocity
    lastY = t.clientY
    lastT = e.timeStamp
    travel(px, t.clientX, t.clientY)
  }

  const onEnd = (e: TouchEvent) => {
    if (!dragging) return
    dragging = false
    e.stopPropagation()
    const x = startX
    const y = lastY
    let v = velocity
    let prev = performance.now()
    // a flick keeps going and slows down, the way lists do
    const glide = (now: number) => {
      const dt = now - prev
      prev = now
      v *= Math.pow(0.95, dt / 16)
      if (Math.abs(v) < 0.05) return
      travel(v * dt, x, y)
      fling = requestAnimationFrame(glide)
    }
    if (Math.abs(v) > 0.3) fling = requestAnimationFrame(glide)
  }

  const opts = { capture: true, passive: false } as const
  host.addEventListener('touchstart', onStart, opts)
  host.addEventListener('touchmove', onMove, opts)
  host.addEventListener('touchend', onEnd, opts)
  host.addEventListener('touchcancel', onEnd, opts)
  return () => {
    stop()
    host.removeEventListener('touchstart', onStart, opts)
    host.removeEventListener('touchmove', onMove, opts)
    host.removeEventListener('touchend', onEnd, opts)
    host.removeEventListener('touchcancel', onEnd, opts)
  }
}
