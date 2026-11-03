import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import clsx from 'clsx'
import { haptic } from '../../lib/haptics.ts'
import { Logo } from '../AppShell.tsx'

/** how far to pull (px, after damping) before letting go refreshes */
export const PULL_TRIGGER = 64
const MAX = 96

/**
 * A scrolling page that refreshes when you pull down from the top on a touch screen. The mark
 * follows the finger and its line pays out as it comes, ticks once when the line is all out, and
 * reels in and out while `onRefresh` settles.
 */
export function PullRefresh({ onRefresh, className, children }: { onRefresh: () => Promise<unknown>; className?: string; children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null)
  const [pull, setPull] = useState(0)
  const [busy, setBusy] = useState(false)
  const live = useRef({ start: -1, pull: 0, armed: false, busy: false })

  useEffect(() => {
    const el = box.current
    if (!el) return
    const s = live.current
    const down = (e: TouchEvent) => {
      s.start = el.scrollTop <= 0 && !s.busy && e.touches.length === 1 ? e.touches[0].clientY : -1
      s.armed = false
    }
    const move = (e: TouchEvent) => {
      if (s.start < 0) return
      const dy = e.touches[0].clientY - s.start
      if (dy <= 0 || el.scrollTop > 0) {
        if (s.pull) setPull((s.pull = 0))
        return
      }
      // the page stays put while the mark comes down
      if (e.cancelable) e.preventDefault()
      s.pull = Math.min(MAX, dy * 0.5)
      setPull(s.pull)
      const armed = s.pull >= PULL_TRIGGER
      if (armed !== s.armed && armed) haptic('select')
      s.armed = armed
    }
    const up = () => {
      if (s.start < 0) return
      s.start = -1
      if (!s.armed) return setPull((s.pull = 0))
      s.busy = true
      setBusy(true)
      setPull((s.pull = PULL_TRIGGER))
      // long enough to see it happen, however fast the hub answers
      Promise.allSettled([onRefresh(), new Promise((r) => setTimeout(r, 600))]).then(() => {
        s.busy = false
        setBusy(false)
        setPull((s.pull = 0))
      })
    }
    el.addEventListener('touchstart', down, { passive: true })
    el.addEventListener('touchmove', move, { passive: false })
    el.addEventListener('touchend', up)
    el.addEventListener('touchcancel', up)
    return () => {
      el.removeEventListener('touchstart', down)
      el.removeEventListener('touchmove', move)
      el.removeEventListener('touchend', up)
      el.removeEventListener('touchcancel', up)
    }
  }, [onRefresh])

  const dragging = live.current.start >= 0 && !busy
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        aria-hidden={!busy}
        role={busy ? 'status' : undefined}
        aria-label={busy ? 'Refreshing' : undefined}
        className={clsx(
          'pointer-events-none absolute left-1/2 z-30 flex size-9 items-center justify-center rounded-full border border-line bg-elev text-fg-2 shadow-float',
          !dragging && 'transition-[transform,opacity] duration-(--dur-move) ease-spring motion-reduce:transition-none',
        )}
        style={{
          top: 'calc(env(safe-area-inset-top) + 52px)',
          opacity: pull ? Math.min(1, pull / (PULL_TRIGGER * 0.6)) : 0,
          transform: `translate(-50%, ${pull - 40}px)`,
        }}
      >
        <span className={clsx('flex', busy ? 'mark-reeling' : 'mark-pull')} style={busy ? undefined : ({ '--pull': Math.min(1, pull / PULL_TRIGGER) } as CSSProperties)}>
          <Logo className="size-5" />
        </span>
      </div>
      <div ref={box} className={clsx('overscroll-y-contain', className)}>
        {children}
      </div>
    </div>
  )
}
