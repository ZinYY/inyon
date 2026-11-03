import { useEffect, useState } from 'react'
import { MonitorX } from 'lucide-react'
import { api, type SendFailure } from '../../lib/api.ts'
import { timeAgo } from '../../lib/format.ts'
import { Sheet, Waiting } from '../ui/primitives.tsx'

/** The screen a send to this pane failed on, as the hub kept it: the newest one. */
export function FailedScreen({ host, pane, open, onClose }: { host: string; pane: string; open: boolean; onClose: () => void }) {
  const [found, setFound] = useState<SendFailure | null | undefined>(undefined)
  useEffect(() => {
    if (!open) return
    setFound(undefined)
    let live = true
    api
      .sendFailures()
      .then((all) => live && setFound(all.find((f) => f.host === host && f.pane === pane) ?? null))
      // a hub from before 0.22 (or the phone's own) keeps none
      .catch(() => live && setFound(null))
    return () => {
      live = false
    }
  }, [open, host, pane])
  return (
    <Sheet open={open} onClose={onClose} title="What was on screen" icon={<MonitorX className="size-5" />} wide>
      {found === undefined ? (
        <Waiting shape="lines" count={4} label="Loading the screen" />
      ) : found === null ? (
        <p className="text-[15px] text-fg-2">Nothing was kept for this send. Open the terminal to see the agent now.</p>
      ) : (
        <div className="space-y-3">
          <p className="text-[15px] text-fg-2">
            {found.error} <span className="font-mono text-[12px] text-fg-3">{timeAgo(found.at)}</span>
          </p>
          <pre className="max-h-[60vh] overflow-auto rounded-2xl border border-line bg-raised p-3 font-mono text-[12px] leading-[1.5] text-fg">{found.screen.join('\n') || ' '}</pre>
        </div>
      )}
    </Sheet>
  )
}
