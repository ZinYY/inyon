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
