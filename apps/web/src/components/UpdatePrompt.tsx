import { useEffect, useState } from 'react'
import clsx from 'clsx'
import { RotateCw, X } from 'lucide-react'
import { markReloadSeen, reloadDue, useHubBuild, watchHub, type Build } from '../lib/updates.ts'
import { Button } from './ui/primitives.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'

/**
 * "Inyon was updated": the hub runs newer code than this page. Offered once per hub build, and
 * it stays out of the way: nothing is blocked, and closing it is the end of it.
 */
export function UpdatePrompt() {
  const hub = useHubBuild()
  const [shown, setShown] = useState<Build | undefined>()
  const [closing, setClosing] = useState(false)
  useEffect(() => watchHub(), [])
  useEffect(() => {
    if (!reloadDue(hub)) return
    markReloadSeen(hub)
    setClosing(false)
    setShown(hub)
  }, [hub])
  // it fades for a moment before it goes, as a toast does
  const close = () => {
    setClosing(true)
    setTimeout(() => {
      setShown(undefined)
      setClosing(false)
    }, LEAVE_MS)
  }
  if (!shown) return null
  return (
    <div className="pt-safe pointer-events-none fixed inset-x-0 top-0 z-[55] flex justify-center px-3 pt-3 sm:inset-x-auto sm:top-auto sm:right-5 sm:bottom-5 sm:px-0 sm:pt-0">
      <div role="status" className={clsx('pointer-events-auto flex w-full max-w-sm items-center gap-3 rounded-2xl border border-line bg-elev py-2.5 pr-2.5 pl-3.5 shadow-float', closing ? 'animate-fade-out' : 'animate-toast-in')}>
        <RotateCw className="size-4 shrink-0 text-fg-3" />
        <div className="min-w-0 flex-1">
          <p className="text-[13.5px] font-medium">Inyon was updated</p>
          <p className="text-[12.5px] text-fg-3">Reload to get the new version.</p>
        </div>
        <Button size="sm" variant="primary" onClick={() => location.reload()}>
          Reload
        </Button>
        <button type="button" aria-label="Not now" title="Not now" onClick={close} className="press rounded-md p-1 text-fg-3 hover:text-fg">
          <X className="size-4" />
        </button>
      </div>
    </div>
  )
}
