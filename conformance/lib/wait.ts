import type { Msg } from './record.ts'

/** Predicates for AppSocket.until. */

type Host = { config: { id: string }; status: string; sessions: { windows: { panes: { id: string; agent?: { state: string } }[] }[] }[] }

const hostIn = (m: Msg, id: string) => (m.t === 'overview' ? (m.hosts as Host[]).find((h) => h.config.id === id) : undefined)
const panes = (h: Host) => h.sessions.flatMap((s) => s.windows.flatMap((w) => w.panes))

/** An overview where machine `id` is online with `n` panes. */
export const onlineWith = (id: string, n: number) => (m: Msg) => {
  const h = hostIn(m, id)
  return !!h && h.status === 'online' && panes(h).length === n
}

/** An overview where pane `pane` on `id` has an agent in state `state`. */
export const agentState = (id: string, pane: string, state: string) => (m: Msg) => {
  const h = hostIn(m, id)
  return !!h && panes(h).some((p) => p.id === pane && p.agent?.state === state)
}

/** An overview where machine `id` has status `status`. */
export const hostStatus = (id: string, status: string) => (m: Msg) => hostIn(m, id)?.status === status
