import { agentDef, type HostState, type PaneInfo, type SessionInfo, type WindowInfo } from '@inyon/protocol'
import { paneSlug } from './api.ts'

export interface PaneRef {
  host: HostState
  session: SessionInfo
  window: WindowInfo
  pane: PaneInfo
}

export function allPanes(hosts: HostState[]): PaneRef[] {
  const out: PaneRef[] = []
  for (const host of hosts)
    for (const session of host.sessions)
      for (const window of session.windows) for (const pane of window.panes) out.push({ host, session, window, pane })
  return out
}

const rank = { waiting: 0, working: 1, idle: 2, unknown: 3 }

export function agentPanes(hosts: HostState[]) {
  return allPanes(hosts)
    .filter((r) => r.pane.agent)
    .sort((a, b) => rank[a.pane.agent!.state] - rank[b.pane.agent!.state] || b.window.activity - a.window.activity)
}

export const paneHref = (host: string, pane: string, view?: 'terminal' | 'chat') =>
  `/m/${encodeURIComponent(host)}/p/${paneSlug(pane)}${view ? `?view=${view}` : ''}`

/** Title the agent gives its terminal ("✳ Fix login bug"), else the window name. */
export function paneTitle(r: PaneRef) {
  if (r.pane.companion) return 'Companion'
  if (r.pane.label) return r.pane.label
  const t = r.pane.title.replace(/^[^\w\s]\s+/u, '').trim()
  if (r.pane.agent && t && !/^(Claude Code|codex|opencode|gemini|agy|Antigravity)$/i.test(t) && t !== r.host.hostname) return t
  // a window tmux named after its shell (psmux on Windows names them "powershell") says nothing
  const auto = !r.window.name || r.window.name === r.pane.command || /^(powershell|pwsh|cmd|bash|zsh|sh|fish|node)$/i.test(r.window.name)
  if (r.pane.agent && auto) return r.pane.agent.custom ?? agentDef(r.pane.agent.kind)?.name ?? r.pane.command
  return r.window.name || r.pane.command
}

/** A terminal's tmux window name when someone named it (tmux's automatic name is the command). */
export function terminalName(r: PaneRef) {
  return r.window.name && r.window.name !== r.pane.command ? r.window.name : undefined
}
