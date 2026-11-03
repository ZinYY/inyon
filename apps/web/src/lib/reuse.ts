/**
 * `next` with every part that equals the same part of `prev` replaced by the object from `prev`.
 * The hub sends the whole overview on each change, so without this every machine, session and
 * pane is a new object and every card on screen renders again. With it, a card whose pane didn't
 * change keeps the same object and a memoised component skips it.
 */
export function reuse<T>(prev: T, next: T): T {
  if (prev === next) return prev
  if (Array.isArray(prev) && Array.isArray(next)) {
    let same = prev.length === next.length
    const out = next.map((item, i) => {
      const kept = i < prev.length ? reuse(prev[i], item) : item
      if (kept !== prev[i]) same = false
      return kept
    })
    return (same ? prev : out) as T
  }
  if (isPlain(prev) && isPlain(next)) {
    const keys = Object.keys(next)
    let same = keys.length === Object.keys(prev).length
    const out: Record<string, unknown> = {}
    for (const k of keys) {
      const kept = k in prev ? reuse((prev as any)[k], (next as any)[k]) : (next as any)[k]
      if (kept !== (prev as any)[k]) same = false
      out[k] = kept
    }
    return (same ? prev : out) as T
  }
  return next
}

const isPlain = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v) && Object.getPrototypeOf(v) === Object.prototype
