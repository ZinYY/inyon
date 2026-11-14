import { LoaderCircle } from 'lucide-react'
import type { ClearingConversation as Clearing } from '../../lib/clearConversation.ts'
import { refreshClearedConversation } from '../../lib/clearConversation.ts'
import { Button } from '../ui/primitives.tsx'
import type { ReactNode } from 'react'

export function ConversationWait({ label, detail, failed, children }: { label: string; detail?: ReactNode; failed?: boolean; children?: ReactNode }) {
  return <div className="absolute inset-0 z-20 flex items-center justify-center rounded-3xl border border-line bg-elev/95 px-4 py-3">
    <div className="w-full text-center" role="status" aria-live="polite">
      <div className="flex items-center justify-center gap-2 text-[14px] font-medium">
        {!failed && <LoaderCircle aria-hidden className="size-4 shrink-0 animate-spin text-accent-text" />}
        <span>{label}</span>
      </div>
      {detail && <p className="mt-1 text-[12px] text-fg-2">{detail}</p>}
      {children}
    </div>
  </div>
}

export function ClearingConversation({ host, pane, state, onTerminal }: { host: string; pane: string; state: Clearing; onTerminal: () => void }) {
  const failed = state.phase === 'failed'
  return <ConversationWait failed={failed} label={failed ? 'Waiting for the conversation' : state.phase === 'refreshing' ? 'Refreshing conversation…' : `${state.label}…`}
    detail={failed ? state.error : 'You can keep reading. Input will return when the refresh is complete.'}>
      {failed && <div className="mt-2 flex justify-center gap-2">
        <Button onClick={() => void refreshClearedConversation(host, pane)}>Refresh status</Button>
        <Button variant="ghost" onClick={onTerminal}>Open terminal</Button>
      </div>}
  </ConversationWait>
}
