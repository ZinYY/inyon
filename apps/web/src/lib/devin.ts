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
