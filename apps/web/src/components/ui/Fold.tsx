import { useCallback, useState, type ReactNode } from 'react'
import clsx from 'clsx'

/**
 * A body that opens and closes under a row. It is mounted at the first open and kept after, so
 * closing folds it shut instead of cutting it off, and opening again doesn't rebuild it.
 */
export function useFold(): [open: boolean, mounted: boolean, toggle: () => void] {
  const [open, setOpen] = useState(false)
  const [mounted, setMounted] = useState(false)
  const toggle = useCallback(() => {
    setOpen((o) => !o)
    setMounted(true)
  }, [])
  return [open, mounted, toggle]
}

/** The body itself: grid rows from nothing to its height (`.expand`), its content fading with it. */
export function Fold({ open, mounted, className, children }: { open: boolean; mounted: boolean; className?: string; children: ReactNode }) {
  if (!mounted) return null
  return (
    <div className={clsx('expand', className)} data-open={open}>
      <div inert={!open}>{children}</div>
    </div>
  )
}
