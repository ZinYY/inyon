import { AGENT_CATALOG } from '@inyon/protocol'
/**
 * Where the hub is. In a browser the app is served by the hub, so every path is relative. The
 * phone app has no hub (see phone.ts).
 */
export function wsUrl() {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws?catalog=${AGENT_CATALOG}`
}

/** The hub's own address as the machines see it (for pairing commands). */
export const hubOrigin = () => location.origin
