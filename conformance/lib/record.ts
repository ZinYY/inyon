/**
 * How a step's pushed messages are written down. The hub's timers (a 4 s poll, a 100 ms overview
 * coalescer, screen reads 180-1600 ms apart) decide how many messages a change produces and how
 * they interleave, so the transcript keeps what the app depends on and nothing timing-dependent:
 *
 * - Messages are split into streams: the overview, notifications, one stream per chat and one per
 *   terminal. Order is kept within a stream; streams are written in a fixed order.
 * - Snapshot messages, which the app replaces wholesale (overview, chat.info, chat.screen,
 *   chat.live, chat.draft, chat.progress), keep only the step's last one.
 * - chat.append messages in a row are one append (how the hub batches lines depends on how they
 *   arrive), and term.data chunks of one terminal are one string.
 *
 * Everything else (chat.reset, chat.more, notify, term.exit, pong, anything unknown) is kept in order.
 */

export type Msg = { t: string; [k: string]: unknown }

const SNAPSHOT: Record<string, string> = {
  'chat.info': 'info',
  'chat.screen': 'screen',
  'chat.live': 'live',
  'chat.draft': 'draft',
  'chat.progress': 'progress',
}

export interface Folded {
  overview?: Msg
  notify?: Msg[]
  pong?: number
  chats?: Record<string, { events?: Msg[]; info?: unknown; screen?: unknown; live?: unknown; draft?: unknown; progress?: unknown }>
  terms?: Record<string, { data?: string; exit?: Msg[] }>
  other?: Msg[]
}

export function fold(msgs: Msg[]): Folded {
  const out: Folded = {}
  for (const m of msgs) {
    if (m.t === 'overview') out.overview = m
    else if (m.t === 'notify') (out.notify ??= []).push(m)
    else if (m.t === 'pong') out.pong = (out.pong ?? 0) + 1
    else if (m.t.startsWith('chat.') && typeof m.key === 'string') {
      const chat = ((out.chats ??= {})[m.key] ??= {})
      const { t: _t, key: _k, ...rest } = m
      const snap = SNAPSHOT[m.t]
      if (snap) (chat as Record<string, unknown>)[snap] = Object.values(rest)[0]
      else {
        const events = (chat.events ??= [])
        const last = events.at(-1)
        if (m.t === 'chat.append' && last?.t === 'chat.append') last.items = [...(last.items as unknown[]), ...(m.items as unknown[])]
        else events.push(m.t === 'chat.append' ? { ...m, items: [...(m.items as unknown[])] } : m)
      }
    } else if (m.t.startsWith('term.') && typeof m.id === 'string') {
      const term = ((out.terms ??= {})[m.id] ??= {})
      if (m.t === 'term.data') term.data = (term.data ?? '') + String(m.d ?? '')
      else (term.exit ??= []).push(m)
    } else (out.other ??= []).push(m)
  }
  // fixed order, whatever order the messages came in: the kinds as listed, chats and terminals by
  // key, a chat's parts as listed
  for (const k of ['chats', 'terms'] as const) {
    const o = out[k]
    if (o) out[k] = Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) as any
  }
  for (const [key, chat] of Object.entries(out.chats ?? {})) out.chats![key] = ordered(chat, CHAT_PARTS)
  for (const [id, term] of Object.entries(out.terms ?? {})) out.terms![id] = ordered(term, ['data', 'exit'])
  return ordered(out, KINDS)
}

const KINDS = ['overview', 'notify', 'pong', 'chats', 'terms', 'other'] as const
const CHAT_PARTS = ['events', 'info', 'screen', 'live', 'draft', 'progress'] as const

function ordered<T extends object>(o: T, keys: readonly string[]): T {
  const out: Record<string, unknown> = {}
  for (const k of keys) if (k in o) out[k] = (o as Record<string, unknown>)[k]
  return out as T
}

/** Actions on a machine, grouped by pane or terminal, in order within each. */
export function foldActions(actions: { stream: string; did: Record<string, unknown> }[]) {
  const out: Record<string, Record<string, unknown>[]> = {}
  for (const a of actions) (out[a.stream] ??= []).push(a.did)
  return Object.fromEntries(Object.entries(out).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
}
