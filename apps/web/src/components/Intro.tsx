import { useEffect, useRef, useState } from 'react'
import clsx from 'clsx'

/**
 * The cold-start intro: a computer drops in, a light trails behind it and draws the line, and the
 * kite is where it lands. Under 1.2 s, cold start only, a tap skips it. It ends by flying the
 * mark onto the mark in the header (measured, so it lands exactly) and fading the page in behind.
 * If the page hasn't drawn its header yet when the drawing is done (a first open, waiting on the
 * session), the finished mark holds for up to a second more; where no header mark turns up it
 * shrinks and fades in place. Under reduced motion nothing moves: the finished mark shows for a
 * moment and fades.
 */
const SEEN = 'inyon.intro.seen'
/** when the drawing is done and the hand-off starts */
const HANDOFF_AT = 1000
/** how long the finished mark waits for a header mark to land on */
const HOLD_MS = 1200
const HANDOFF_MS = 320
/** the overlay goes on transitionend; this backs that up on a device that drops the event */
const SETTLE_MS = HANDOFF_MS + 400

const SQUIRCLE = 'M25 0H39C60 0 64 4 64 25V39C64 60 60 64 39 64H25C4 64 0 60 0 39V25C0 4 4 0 25 0Z'
const KITE = 'M33.5 9H48.5A3.5 3.5 0 0 1 52 12.5V33.5A3.5 3.5 0 0 1 48.5 37H33.5A3.5 3.5 0 0 1 30 33.5V12.5A3.5 3.5 0 0 1 33.5 9ZM41 18.4A4.6 4.6 0 1 0 41 27.6A4.6 4.6 0 1 0 41 18.4Z'
const LINE = 'M38.5 36.5C35 48 25 42 17.5 49.5'

/** Whether this page load should show the intro: the first in this app session. */
export function introDue() {
  try {
    if (sessionStorage.getItem(SEEN)) return false
    sessionStorage.setItem(SEEN, '1')
    return true
  } catch {
    return false
  }
}

const reduced = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches

/** The header's mark, if one is on screen, to land on. */
function target(): DOMRect | undefined {
  for (const el of document.querySelectorAll<HTMLElement>('[data-intro-target]')) {
    const r = el.getBoundingClientRect()
    if (r.width > 0 && r.height > 0) return r
  }
  return undefined
}

type Phase = 'draw' | 'handoff' | 'gone'

export function Intro({ onDone }: { onDone?: () => void }) {
  const [phase, setPhase] = useState<Phase>('draw')
  const at = useRef<Phase>('draw')
  const mark = useRef<HTMLDivElement>(null)
  const [flight, setFlight] = useState<React.CSSProperties>()
  const timers = useRef<ReturnType<typeof setTimeout>[]>([])
  const frame = useRef(0)
  /** where the flight ends: the header mark's rectangle */
  const dest = useRef<DOMRect | undefined>(undefined)
  const later = (fn: () => void, ms: number) => timers.current.push(setTimeout(fn, ms))
  const stop = () => {
    timers.current.forEach(clearTimeout)
    cancelAnimationFrame(frame.current)
  }

  const finish = () => {
    if (at.current === 'gone') return
    at.current = 'gone'
    setPhase('gone')
    document.documentElement.classList.remove('intro-running')
    onDone?.()
  }
  /** Flies the mark onto the header's; `until` is how long to wait for a header mark first. */
  const handoff = (until: number) => {
    if (at.current !== 'draw') return
    if (!target() && Date.now() < until) return later(() => handoff(until), 50)
    // measured and started inside a frame: a transition begun from a timer after the page has
    // sat still takes the last painted frame as its start and can be over before it is seen
    frame.current = requestAnimationFrame(() => {
      if (at.current !== 'draw') return
      const from = mark.current?.getBoundingClientRect()
      const to = target()
      if (from && to) {
        // the same drawing in both, so one translate and one scale put it exactly there
        const scale = to.width / from.width
        const dx = to.left + to.width / 2 - (from.left + from.width / 2)
        const dy = to.top + to.height / 2 - (from.top + from.height / 2)
        dest.current = to
        setFlight({ transform: `translate(${dx}px, ${dy}px) scale(${scale})` })
      } else setFlight({ transform: 'scale(0.6)', opacity: 0 })
      at.current = 'handoff'
      setPhase('handoff')
      later(finish, SETTLE_MS)
    })
  }
  /** Whether the mark sits on the header's mark now, within a pixel. */
  const arrived = () => {
    const to = dest.current
    const r = mark.current?.getBoundingClientRect()
    if (!to || !r) return true
    return Math.abs(r.left - to.left) < 1 && Math.abs(r.top - to.top) < 1 && Math.abs(r.width - to.width) < 1
  }

  useEffect(() => {
    document.documentElement.classList.add('intro-running')
    const drawn = reduced() ? 400 : HANDOFF_AT
    later(() => handoff(Date.now() + HOLD_MS), drawn)
    return () => {
      stop()
      document.documentElement.classList.remove('intro-running')
    }
  }, [])
  const landed = (e: React.TransitionEvent) => {
    // the flight's end, once the mark is really there; anything else waits for the settle timer
    if (at.current === 'handoff' && e.target === mark.current && (e.propertyName === 'transform' || e.propertyName === 'opacity') && arrived()) finish()
  }

  if (phase === 'gone') return null
  const skip = () => {
    stop()
    // a tap wants it gone: fly if the header is there, otherwise fade at once
    handoff(0)
  }
  return (
    <div
      className={clsx('intro fixed inset-0 z-[90] grid place-items-center bg-bg', phase === 'handoff' && 'intro-off', reduced() && 'intro-still')}
      onPointerDown={skip}
      onKeyDown={skip}
      role="presentation"
      aria-hidden
      data-testid="intro"
    >
      <div ref={mark} className="intro-mark" style={phase === 'handoff' ? flight : undefined} onTransitionEnd={landed}>
        <svg viewBox="0 0 64 64" className="size-full overflow-visible">
          <path className="intro-tile" d={SQUIRCLE} fill="var(--logo-tile)" />
          <g className="intro-glyph">
            <path className="intro-kite" fillRule="evenodd" d={KITE} />
            <path className="intro-line" d={LINE} fill="none" strokeWidth="3.8" strokeLinecap="round" />
            <circle className="intro-node" cx="15.5" cy="51" r="4" />
            <circle className="intro-light" r="2.6" />
          </g>
        </svg>
      </div>
    </div>
  )
}
