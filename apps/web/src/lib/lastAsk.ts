import type { HostState, ScreenPrompt } from '@inyon/protocol'
import { useStore } from './store.ts'

/**
 * The question each session was last seen asking, with its answers, kept through a reconnect.
 * Opening the app from an alert shows the conversation before the machine is back; until then the
 * overview says only that the machine is connecting, and the question would be gone. Kept from the
 * overview (the copy saved on the device counts), forgotten once the machine is back and the
 * session isn't asking any more.
 */
const asks = new Map<string, ScreenPrompt>()
const keyOf = (host: string, pane: string) => `${host}\u0000${pane}`

export function noteAsks(hosts: HostState[]) {
  for (const h of hosts) {
    for (const s of h.sessions) {
      for (const w of s.windows) {
        for (const p of w.panes) {
          const a = p.agent
          const k = keyOf(h.config.id, p.id)
          if (a?.state === 'waiting' && a.options?.length) {
            asks.set(k, { body: [a.waitingFor || 'Waiting for your answer'], options: a.options.map((o) => ({ ...o, selected: false })), multi: false })
          } else if (h.status === 'online') asks.delete(k)
        }
      }
    }
  }
}

/** The question this session was last seen asking, if it may still be waiting. */
export const lastAsk = (host: string, pane: string) => asks.get(keyOf(host, pane))

noteAsks(useStore.getState().hosts)
useStore.subscribe((s, prev) => {
  if (s.hosts !== prev.hosts) noteAsks(s.hosts)
})
