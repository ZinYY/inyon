import { useEffect, useRef } from 'react'

/**
 * Runs `reset` when a sheet opens, or when it switches to a different item while open. Not when the
 * item's object merely changes: the store rebuilds those on every refresh, and resetting then
 * would throw away what you're typing.
 */
export function useOpened(open: boolean, key: string | undefined, reset: () => void) {
  const last = useRef<string | null>(null)
  const fn = useRef(reset)
  fn.current = reset
  useEffect(() => {
    if (!open) {
      last.current = null
      return
    }
    const k = key ?? ''
    if (last.current === k) return
    last.current = k
    fn.current()
  }, [open, key])
}
