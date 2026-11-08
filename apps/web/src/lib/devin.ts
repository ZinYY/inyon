export interface DevinSession {
  id: string
  title: string | null
  cwd: string
  model: string
  updated: number
}

export interface DevinMessage {
  id: string
  role: 'user' | 'assistant' | 'tool'
  text: string
  thinking?: string | null
  tools: { name: string; input: unknown }[]
  time?: string | null
}

export interface DevinUpdate {
  version: number
  reset: boolean
  loading: boolean
  session: DevinSession | null
  state: { status: 'idle' | 'busy'; reason?: string | null; source: 'app' | 'cli'; tmux?: string | null; pane?: string | null; live?: string | null } | null
  error?: string | null
  order: string[]
  messages: DevinMessage[]
}

/** Validate before scheduling a React state update: a failed JSON read must not
 * advance the cursor or throw later from inside React's state updater. */
export function parseDevinUpdate(value: unknown): DevinUpdate {
  const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v)
  const nullableString = (v: unknown) => v == null || typeof v === 'string'
  const session = object(value) ? value.session : undefined
  const state = object(value) ? value.state : undefined
  if (!object(value) || !Number.isSafeInteger(value.version) || (value.version as number) < 0
    || typeof value.reset !== 'boolean' || typeof value.loading !== 'boolean'
    || !Array.isArray(value.order) || !value.order.every((id) => typeof id === 'string')
    || !Array.isArray(value.messages) || !value.messages.every((m) => object(m)
      && typeof m.id === 'string' && ['user', 'assistant', 'tool'].includes(m.role as string)
      && typeof m.text === 'string' && nullableString(m.thinking) && nullableString(m.time)
      && Array.isArray(m.tools) && m.tools.every((t) => object(t) && typeof t.name === 'string'))
    || !(session === null || object(session) && typeof session.id === 'string' && nullableString(session.title)
      && typeof session.cwd === 'string' && typeof session.model === 'string' && typeof session.updated === 'number')
    || !(state === null || object(state) && ['idle', 'busy'].includes(state.status as string)
      && ['app', 'cli'].includes(state.source as string) && ['reason', 'tmux', 'pane', 'live'].every((k) => nullableString(state[k])))
    || !nullableString(value.error)) {
    throw new Error('Could not read Devin history. Retrying…')
  }
  return value as unknown as DevinUpdate
}

export function mergeDevin(previous: Map<string, DevinMessage>, update: DevinUpdate): Map<string, DevinMessage> {
  const next = update.reset ? new Map<string, DevinMessage>() : new Map(previous)
  const keep = new Set(update.order)
  for (const id of next.keys()) if (!keep.has(id)) next.delete(id)
  for (const message of update.messages) next.set(message.id, message)
  return next
}

export const DEVIN_BUSY = 'Devin can only receive a new message after the previous response has finished. Finish any pending question or approval in Devin first.'
import type { Part, Row } from './chat.ts'


export function devinRow(message: DevinMessage): Extract<Row, { type: 'user' | 'assistant' }> {
  const ts = message.time ?? ''
  const base = { seq: 0, uuid: message.id, ts }
  if (message.role === 'user') return { type: 'user', key: message.id, item: { ...base, kind: 'user', text: message.text, images: 0 } }
  const parts: Part[] = []
  if (message.thinking) parts.push({ type: 'thinking', key: `${message.id}:thinking`, text: message.thinking })
  if (message.role === 'tool') parts.push({ type: 'tools', key: message.id, calls: [{ id: message.id, name: 'Tool output', input: {}, result: { ...base, kind: 'tool_result', toolUseId: message.id, text: message.text, isError: false } }] })
  else if (message.text) parts.push({ type: 'text', key: message.id, text: message.text })
  if (message.tools.length) parts.push({ type: 'tools', key: `${message.id}:tools`, calls: message.tools.map((tool, i) => ({ id: `${message.id}:${i}`, name: tool.name, input: tool.input })) })
  return { type: 'assistant', key: message.id, ts, parts, open: false }
}

/** Native storage splits one reply into many assistant/tool records. Render
 * them as the same shared assistant turn, with a single footer at the end. */
export function devinTurns(messages: DevinMessage[]): DevinMessage[][] {
  const turns: DevinMessage[][] = []
  for (const message of messages) {
    const last = turns.at(-1)
    if (message.role !== 'user' && last && last[0]!.role !== 'user') last.push(message)
    else turns.push([message])
  }
  return turns
}

export function devinTurn(messages: DevinMessage[], open = false): Extract<Row, { type: 'user' | 'assistant' }> {
  const first = devinRow(messages[0]!)
  if (first.type === 'user') return first
  return { ...first, open, ts: messages.at(-1)!.time ?? '', parts: messages.flatMap((m) => {
    const row = devinRow(m)
    return row.type === 'assistant' ? row.parts : []
  }) }
}
