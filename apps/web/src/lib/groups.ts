import { agentDef, type HostState, type SessionInfo } from '@inyon/protocol'
import { allPanes, paneTitle, terminalName, type PaneRef } from './overview.ts'

/**
 * Panes grouped the way tmux holds them: machine → tmux session → window. With dozens of sessions
 * across several machines a flat list stops working; groups fold away, and a search looks through
 * all of them at once.
 */
export interface SessionGroup {
  key: string
  session: SessionInfo
  panes: PaneRef[]
  waiting: number
}

export interface HostGroup {
  key: string
  host: HostState
  sessions: SessionGroup[]
  panes: number
  waiting: number
}

/** Every word of the query appears somewhere in the text (case-insensitive). */
export function matchesQuery(text: string, query: string) {
  const t = text.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => t.includes(w))
}

/** Everything a search can find a pane by. */
export function searchText(r: PaneRef) {
  return [
    r.pane.agent ? paneTitle(r) : (terminalName(r) ?? ''),
    r.pane.agent ? agentDef(r.pane.agent.kind)?.name : 'terminal',
    r.pane.command,
    r.host.config.name,
    r.host.hostname,
    r.session.name,
    `${r.session.name}:${r.window.index}`,
    r.window.name,
    r.pane.cwd,
  ]
    .filter(Boolean)
    .join(' ')
}

/** tmux numbers sessions as it makes them ("$4"), so the number is the order they were started in. */
export const sessionOrder = (s: { id: string; name: string }) => {
  const n = Number(s.id.replace(/^\$/, ''))
  return Number.isFinite(n) ? n : Number.MAX_SAFE_INTEGER
}

/** tmux's own address for a pane: session:window, plus .pane when the window is split. */
export const tmuxTarget = (r: PaneRef) => `${r.session.name}:${r.window.index}${r.window.panes.length > 1 ? `.${r.pane.index}` : ''}`

/**
 * Groups panes by machine and tmux session, keeping only those that pass `keep` and match `query`.
 * Everything keeps its place: machines in the order they were added, sessions in the order tmux
 * started them, panes in tmux's window and pane order. Nothing moves when an agent starts working
 * or asks something, so a row is still where you aimed when the click lands.
 */
export function groupPanes(hosts: HostState[], o: { query?: string; keep?: (r: PaneRef) => boolean } = {}): HostGroup[] {
  const refs = allPanes(hosts).filter((r) => (!o.keep || o.keep(r)) && (!o.query || matchesQuery(searchText(r), o.query)))
  const byHost = new Map<string, HostGroup>()
  const bySession = new Map<string, SessionGroup>()
  for (const r of refs) {
    const hk = r.host.config.id
    let hg = byHost.get(hk)
    if (!hg) byHost.set(hk, (hg = { key: hk, host: r.host, sessions: [], panes: 0, waiting: 0 }))
    const sk = `${hk}\u0000${r.session.id}`
    let sg = bySession.get(sk)
    if (!sg) {
      bySession.set(sk, (sg = { key: sk, session: r.session, panes: [], waiting: 0 }))
      hg.sessions.push(sg)
    }
    const waiting = r.pane.agent?.state === 'waiting' ? 1 : 0
    sg.panes.push(r)
    sg.waiting += waiting
    hg.panes++
    hg.waiting += waiting
  }
  const order = new Map(hosts.map((h, i) => [h.config.id, i]))
  const groups = [...byHost.values()]
  for (const g of groups) g.sessions.sort((a, b) => sessionOrder(a.session) - sessionOrder(b.session))
  return groups.sort((a, b) => order.get(a.key)! - order.get(b.key)!)
}

/** Folded groups, kept per browser. A search shows every match, folded or not. */
const KEY = 'inyon.folded'

export function loadFolded(): Set<string> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '[]')
    return new Set(Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

export function saveFolded(folded: Set<string>) {
  try {
    localStorage.setItem(KEY, JSON.stringify([...folded].slice(-500)))
  } catch {}
}
