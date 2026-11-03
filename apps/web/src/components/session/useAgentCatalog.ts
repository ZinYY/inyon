import { useEffect, useState } from 'react'
import type { AgentCatalog, AgentKind } from '@inyon/protocol'
import { api } from '../../lib/api.ts'

/** agents whose model list the hub reads from the CLI (agy models, Codex's models cache) */
const LIVE = new Set<AgentKind>(['antigravity', 'codex', 'opencode', 'copilot'])

export function useAgentCatalog(host: string, pane: string, agent?: AgentKind) {
  const [state, setState] = useState<{ key: string; catalog: AgentCatalog }>()
  const key = `${host}:${pane}:${agent}`
  useEffect(() => {
    if (!agent || !LIVE.has(agent)) return
    let active = true
    const refresh = () => api.models(host, pane).then((catalog) => { if (active) setState({ key, catalog }) }).catch(() => {})
    void refresh()
    const timer = setInterval(refresh, 60_000)
    // a provider connected or disconnected (ProvidersSheet): the list changes at once
    window.addEventListener('inyon:models-changed', refresh)
    return () => {
      active = false
      clearInterval(timer)
      window.removeEventListener('inyon:models-changed', refresh)
    }
  }, [host, pane, agent, key])
  return state?.key === key ? state.catalog : undefined
}
