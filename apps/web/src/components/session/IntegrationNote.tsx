import { useState } from 'react'
import { Plug } from 'lucide-react'
import type { AgentKind, HostState } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { Button } from '../ui/primitives.tsx'

export const INTEGRATION_TEXT: Record<string, string> = {
  missing: 'Turn on the Claude integration for live limits, context and cost.',
  removed: '~/.claude/settings.json was replaced and no longer runs the Claude integration, so live limits and context stopped updating.',
  outdated: 'The Claude integration on this machine is an older version. Update it for accurate limits after account switches and compaction.',
}

/** Why live numbers are missing for a Claude session, with the one-click fix. */
export function IntegrationNote({ host, agent = 'claude' }: { host: HostState; agent?: AgentKind }) {
  const [busy, setBusy] = useState(false)
  const isAgy = agent === 'antigravity'
  const state = isAgy ? host.agyIntegration ?? 'unknown' : host.integration
  const enable = async () => {
    setBusy(true)
    try {
      await api.integration(host.config.id, true)
      toast({ title: `${isAgy ? 'Antigravity' : 'Claude'} integration on`, body: isAgy ? 'Restart this CLI session to enable live numbers; new sessions pick it up automatically.' : 'Numbers refresh on the next reply.', tone: 'success' })
    } catch (e) {
      toast({ title: 'Could not turn it on', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex items-start gap-3 rounded-3xl border border-line bg-elev shadow-card p-3.5">
      <Plug className="mt-0.5 size-4 shrink-0 text-warn" />
      <p className="min-w-0 flex-1 text-[12.5px] leading-relaxed text-fg-2">{isAgy ? 'Turn on the Antigravity integration for live context and plan limits. Restart this CLI session afterward to enable it.' : INTEGRATION_TEXT[state] ?? INTEGRATION_TEXT.missing}</p>
      <Button size="sm" variant="primary" onClick={enable} loading={busy}>
        {state === 'outdated' ? 'Update' : 'Turn on'}
      </Button>
    </div>
  )
}
