import { useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import clsx from 'clsx'

export interface Keyed {
  key: string
  node: ReactNode
}

/** how long a leaving item stays, fading, before it is taken out */
export const LEAVE_MS = 120

/**
 * A grid whose items arrive, leave and move without jumping. A new item fades in and rises a
 * little (the first batch one row after another); one that goes fades out where it was, then the
 * others slide to their new places (measured before and after, moved with a transform only).
 * Nothing here changes layout on its own. `enter` is the class a new item arrives with.
 */
export function AnimatedList({ items, className, enter = 'animate-slide-up' }: { items: Keyed[]; className?: string; enter?: string }) {
  const box = useRef<HTMLDivElement>(null)
  /** keys shown last time, to tell a new item from one that was already there */
  const seen = useRef<Set<string>>(new Set())
  /** keys in the order they were shown last time, so one that leaves keeps its place while it fades */
  const order = useRef<string[]>([])
  /** where each item was last time, by key */
  const rects = useRef<Map<string, DOMRect>>(new Map())
  /** the list's own width last time: when it changes (the window settling at launch, a tablet turning) items reflow, they didn't move */
  const width = useRef<number | undefined>(undefined)
  /** the content each item had last time, so a leaving one fades out looking as it did */
  const last = useRef<Map<string, ReactNode>>(new Map())
  const [leaving, setLeaving] = useState<Keyed[]>([])

  const current = new Set(items.map((i) => i.key))
  const gone = [...seen.current].filter((k) => !current.has(k))
  for (const i of items) last.current.set(i.key, i.node)

  // which items are arriving is decided when the set of keys changes, not on every render: a
  // refresh that brings the same items must not take the class off a row still rising, which
  // would snap it into place. The first batch is also the one that comes in staggered
  const keys = items.map((i) => i.key).join('\u0000')
  const { entering, first } = useMemo(
    () => ({ entering: new Set(items.filter((i) => !seen.current.has(i.key)).map((i) => i.key)), first: seen.current.size === 0 && items.length > 0 }),
    [keys],
  )

  // a leaving item goes back in at the index it had, so it fades out in its own place and not in
  // the last cell of the grid
  const shown = items.map((i) => ({ ...i, entering: entering.has(i.key), leaving: false }))
  const out = leaving.filter((l) => !current.has(l.key)).map((l) => ({ ...l, entering: false, leaving: true, at: order.current.indexOf(l.key) }))
  for (const l of out.sort((a, b) => a.at - b.at)) shown.splice(l.at < 0 ? shown.length : Math.min(l.at, shown.length), 0, l)

  // an item that disappeared stays for a moment, marked, then goes for good. Its timer lives on
  // its own: the next render changes what counts as gone, and must not cancel it
  const timers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())
  useLayoutEffect(() => {
    if (!gone.length) return
    const add = gone.filter((k) => last.current.has(k)).map((k) => ({ key: k, node: last.current.get(k)! }))
    for (const k of gone) last.current.delete(k)
    setLeaving((l) => [...l.filter((x) => !gone.includes(x.key)), ...add])
    for (const k of gone) {
      clearTimeout(timers.current.get(k))
      timers.current.set(k, setTimeout(() => {
        timers.current.delete(k)
        setLeaving((l) => l.filter((x) => x.key !== k))
      }, LEAVE_MS))
    }
  }, [gone.join('\u0000')])
  useLayoutEffect(() => () => timers.current.forEach((t) => clearTimeout(t)), [])

  // items that moved: start where they were, then let the transition carry them
  useLayoutEffect(() => {
    const el = box.current
    if (!el) return
    const w = el.getBoundingClientRect().width
    const resized = width.current !== undefined && Math.abs(w - width.current) > 0.5
    width.current = w
    const next = new Map<string, DOMRect>()
    for (const child of Array.from(el.children) as HTMLElement[]) {
      const key = child.dataset.key!
      const r = child.getBoundingClientRect()
      next.set(key, r)
      const was = rects.current.get(key)
      if (!was || resized || child.dataset.leaving) continue
      const dx = was.left - r.left
      const dy = was.top - r.top
      if (!dx && !dy) continue
      child.style.transition = 'none'
      child.style.transform = `translate(${dx}px, ${dy}px)`
      requestAnimationFrame(() => {
        child.style.transition = ''
        child.style.transform = ''
      })
    }
    rects.current = next
    seen.current = current
    // not while something has just gone: this commit lacks it, the next one shows it leaving in place
    if (!gone.length) order.current = shown.map((s) => s.key)
  })

  return (
    <div ref={box} className={className}>
      {shown.map((i, n) => (
        <div
          key={i.key}
          data-key={i.key}
          data-leaving={i.leaving || undefined}
          style={first ? ({ '--i': n } as CSSProperties) : undefined}
          className={clsx('flip', i.entering && enter, i.entering && first && 'staggered', i.leaving && 'leave')}
        >
          {i.node}
        </div>
      ))}
    </div>
  )
}
