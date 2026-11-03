import type { HostState, HostStatus } from '@inyon/protocol'
import { causeOf, connLog, dropped, setPowerSource, watchPage } from './connlog.ts'
import { nativePower } from './native.ts'
import { useStore } from './store.ts'

/**
 * Keeps the connection log (connlog.ts) for every computer and machine, from the machine list:
 * one that goes from online to anything else dropped, with its own error as the reason, and one
 * that comes back is up again. Hubs are logged where their sockets are (socket.ts).
 */
export function followHosts(hosts: HostState[], seen: Map<string, { status: HostStatus; error?: string }>) {
  const ids = new Set<string>()
  for (const h of hosts) {
    const id = h.config.id
    ids.add(id)
    const about = { kind: h.config.kind === 'link' ? ('computer' as const) : ('machine' as const), name: h.config.name }
    const was = seen.get(id)
    seen.set(id, { status: h.status, error: h.error })
    if (h.status === 'online') {
      if (was?.status !== 'online') connLog.up(id, about)
      continue
    }
    const cause = causeOf(h.error)
    // its hub out of reach is the hub's drop (socket.ts), not one for each machine it carries
    if (cause === 'hub-down') continue
    if (was?.status === 'online') dropped(id, about, cause, { detail: h.error })
    else if (h.error && h.error !== was?.error) {
      // still down, and it says something new: another failed try
      if (connLog.get(id)?.up === false) connLog.down(id, about, cause, { detail: h.error })
      // down with a reason of its own (not paired, say), first seen so or after its hub came back
      else dropped(id, about, cause, { detail: h.error })
    }
  }
  for (const id of seen.keys()) if (!ids.has(id)) seen.delete(id)
}

let installed = false
export function installConnWatch() {
  if (installed) return
  installed = true
  watchPage()
  setPowerSource(nativePower)
  const seen = new Map<string, { status: HostStatus; error?: string }>()
  useStore.subscribe((s, prev) => {
    // the copy this device kept paints first; only what the hub or the phone's core says counts
    // (the hub's first answer can match the kept copy and leave the same list, but it still counts)
    if (s.hostsStale || (s.hosts === prev.hosts && !prev.hostsStale)) return
    followHosts(s.hosts, seen)
  })
}
