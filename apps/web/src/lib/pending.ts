import type { ChatItem, SendState } from '@inyon/protocol'

export interface PendingMessage {
  id: string
  text: string
  status: 'sending' | 'sent' | 'failed'
  at: number
  error?: string
  /** failed because the connection dropped, not because it was refused: sent again once it's back */
  retryable?: boolean
  /** in the agent's queue: shown as a queued bubble instead */
  queued?: boolean
  /** sent from another device; this one can only watch it */
  remote?: boolean
}

export const norm = (s: string) => s.replace(/\s+/g, ' ').trim()
const clean = (s: string) => {
  // Codex expands attachment paths into image envelopes and substitutes numbered labels.
  // Restore their paths before comparing; the full text and each attachment must still match.
  const images = new Map<string, string>()
  s = s.replace(/<image\s+name=(\[Image #\d+\])\s+path="([^"]+)">[\s\S]*?<\/image>/g, (_, name, path) => {
    images.set(name, path)
    return ''
  })
  s = s.replace(/(Attached files:\s*)([\s\S]*)$/, (_, heading, files) => heading + files.replace(/\[Image #\d+\]/g, (name: string) => images.get(name) ?? name))
  return norm(s.replace(/<\/?pasted_content\b[^>]*>/g, '')).replace(/^!\s*/, '!')
}
export const msgKey = (s: string) => clean(s).slice(0, 400)
export const PENDING_MS = 10 * 60_000
// A receipt and its message share their words. Spacing, quotes and markup can differ: agents
// rewrap pastes, and some swap straight quotes for curly ones.
const loose = (s: string) => clean(s).toLowerCase().replace(/[^\p{L}\p{N}!/]+/gu, '')
// The transcript is stamped by the agent's machine and the message by this device; phones and
// laptops can be several seconds apart.
const SKEW_MS = 30_000

/** Only a new transcript entry can confirm a send; consume each receipt at most once. */
export function reconcile(pending: PendingMessage[], items: ChatItem[]) {
  const receipts = items.flatMap((item) => item.kind === 'user'
    ? [{ text: loose(item.shell ? `!${item.text}` : item.text), at: Date.parse(item.ts) }]
    : item.kind === 'command' && item.name ? [{ text: loose(`/${item.name.replace(/^\/+/, '')} ${item.args}`), at: Date.parse(item.ts) }] : [])
  return pending.filter((message) => {
    const text = loose(message.text)
    // very long messages are cut short in some transcripts
    const receipt = receipts.findIndex((entry) => entry.at >= message.at - SKEW_MS && (entry.text === text || entry.text.length >= 20_000 && text.startsWith(entry.text)))
    if (receipt < 0) return true
    receipts.splice(receipt, 1)
    return false
  })
}

/**
 * This device's sends that still need a bubble. The hub's ledger decides (it matched each send by
 * id against the agent's own record); comparing text with the conversation is only the fallback
 * for a send the hub isn't tracking, such as one from before a hub restart.
 */
export function settle(pending: PendingMessage[], items: ChatItem[], sends: readonly SendState[] = []): PendingMessage[] {
  const byId = new Map(sends.map((s) => [s.id, s]))
  const untracked = reconcile(pending.filter((p) => !byId.has(p.id)), items)
  return pending.flatMap((p): PendingMessage[] => {
    const s = byId.get(p.id)
    if (!s) return untracked.includes(p) ? [p] : []
    if (s.state === 'delivered') return []
    if (s.state === 'queued') return [{ ...p, queued: true, status: 'sent' }]
    // the reply to the request can be lost (a phone asleep); the ledger still knows
    if (s.state === 'failed' && p.status === 'sent') return [{ ...p, status: 'failed', error: s.error }]
    return [p.queued ? { ...p, queued: false } : p]
  })
}

/** Sends from other devices that haven't reached the agent yet, so every screen shows them. */
export function othersSends(sends: readonly SendState[] = [], mine: readonly PendingMessage[]): PendingMessage[] {
  const ids = new Set(mine.map((p) => p.id))
  return sends
    .filter((s) => !ids.has(s.id) && !s.setting && (s.state === 'sending' || s.state === 'sent') && Date.now() - s.at < 5 * 60_000)
    .map((s) => ({ id: s.id, text: s.text, at: s.at, status: s.state === 'sending' ? 'sending' : 'sent', remote: true }))
}
