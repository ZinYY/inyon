import { LoaderCircle } from 'lucide-react'
import type { ClearingConversation as Clearing } from '../../lib/clearConversation.ts'
import { refreshClearedConversation } from '../../lib/clearConversation.ts'
import { Button } from '../ui/primitives.tsx'

export function ClearingConversation({ host, pane, state, onTerminal }: { host: string; pane: string; state: Clearing; onTerminal: () => void }) {
  const failed = state.phase === 'failed'
  return <div className="absolute inset-0 z-40 flex items-center justify-center bg-bg/70 px-6">
    <div className="max-w-sm rounded-3xl border border-line bg-elev p-6 text-center shadow-card" role="status" aria-live="polite">
      {!failed && <LoaderCircle aria-hidden className="mx-auto mb-3 size-6 animate-spin text-accent-text" />}
      <p className="text-[16px] font-medium">{failed ? 'Waiting for the conversation' : state.phase === 'refreshing' ? 'Refreshing conversation…' : `${state.label}…`}</p>
      <p className="mt-2 text-[13px] text-fg-2">{failed ? state.error : 'Chat will be ready when the updated conversation has loaded.'}</p>
      {failed && <div className="mt-4 flex justify-center gap-2">
        <Button onClick={() => void refreshClearedConversation(host, pane)}>Refresh status</Button>
        <Button variant="ghost" onClick={onTerminal}>Open terminal</Button>
      </div>}
    </div>
  </div>
}
