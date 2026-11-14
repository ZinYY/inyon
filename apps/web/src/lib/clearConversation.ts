import { chatKey, type AgentKind, type ScreenState, type ServerMsg } from '@inyon/protocol'
import { api } from './api.ts'
import { socket } from './socket.ts'
import { patchChat, useStore, type ChatState } from './store.ts'
import { inheritReadingPosition } from './readingPosition.ts'

export interface RefreshOptions {
  agent?: AgentKind
  session?: string
  differentFrom?: string
  stopped?: boolean
  closed?: boolean
  allowWorking?: boolean
  allowPrompt?: boolean
  readOnly?: boolean
}
export interface RefreshedConversation {
  reset: Extract<ServerMsg, { t: 'chat.reset' }>
  screen: ScreenState
  draft?: { text: string; source: 'app' | 'terminal'; at: number } | null
}
export interface ClearingConversation {
  id?: number
  phase: 'clearing' | 'refreshing' | 'failed'
  label: string
  options: RefreshOptions
  target?: string
  onReady?: (pane: string) => void
  rewind?: boolean
  error?: string
  reconnect?: boolean
  retained?: Pick<ChatState, 'info' | 'items' | 'hasMore' | 'loaded' | 'screen' | 'live' | 'pending' | 'moreAt' | 'catchingUp'>
}

let operationId = 0
const retained = (chat: ChatState | undefined): ClearingConversation['retained'] => chat && ({
  info: chat.info, items: chat.items, hasMore: chat.hasMore, loaded: chat.loaded,
  screen: chat.screen, live: chat.live, pending: chat.pending, moreAt: chat.moreAt,
})
const current = (key: string, operation: ClearingConversation) => {
  const active = useStore.getState().chats[key]?.clearing
  return !!active && active.id === operation.id
}
const reachable = (host: string) => useStore.getState().connected &&
  !useStore.getState().hosts.some((h) => h.config.id === host && h.status !== 'online')

/** The lock is synchronous and shared by every control, including delayed draft/retry work. */
export const isClearing = (host: string, pane: string) => {
  const chat = useStore.getState().chats[chatKey(host, pane)]
  return !!chat?.clearing || !!chat?.readingMore || chat?.catchingUp !== undefined
}

export function isClearCommand(agent: AgentKind | undefined, text: string) {
  return agent === 'codex' ? /^\/(clear|new)$/.test(text.trim()) : agent === 'claude' && text.trim() === '/clear'
}

export function transitionCommand(agent: AgentKind | undefined, text: string): string | undefined {
  if (isClearCommand(agent, text)) return 'Clearing conversation'
  if (agent === 'codex' && /^\/(btw|side)(?:\s|$)/.test(text.trim())) return 'Opening side conversation'
  if (!agent || !['codex', 'claude', 'opencode', 'antigravity'].includes(agent) || /[\r\n]/.test(text.trim())) return
  const command = text.trim()
  if (/^\/compact(?:\s|$)/.test(command)) return 'Compacting conversation'
  if (/^\/(rewind|undo|redo)$/.test(command)) return 'Updating conversation'
  if (/^\/resume(?:\s|$)/.test(command)) return 'Switching conversation'
  if (/^\/fork(?:\s|$)/.test(command)) return 'Forking conversation'
  if (/^\/(exit|quit)$/.test(command)) return 'Exiting agent'
}

function applySnapshot(host: string, pane: string, result: RefreshedConversation, reconnect = false) {
  // Use the HTTP response to this refresh, not an unrelated cached socket reset.
  // Replace the prefix too: rewind/undo can change history without changing its ID.
  const { reset, screen, draft } = result
  if (reset?.t !== 'chat.reset') throw new Error('The refreshed conversation has not arrived yet.')
  patchChat(chatKey(host, pane), (chat) => {
    const previous = chat.clearing?.retained ?? chat
    const head = reset.items[0]
    const keep = reconnect && reset.info.sessionId === previous.info?.sessionId && reset.hasMore && head &&
      previous.items.some((i) => i.seq === head.seq && i.uuid === head.uuid)
    const same = previous.info?.sessionId === reset.info.sessionId
    return {
      info: reset.info, items: keep ? [...previous.items.filter((i) => i.seq < head.seq), ...reset.items] : reset.items,
      hasMore: keep ? previous.hasMore : reset.hasMore, screen,
      loaded: true, stale: undefined, catchingUp: undefined, readingMore: undefined, live: [], pending: [],
      draft: draft ?? { text: '', source: 'terminal', at: Date.now() },
      ...(reconnect && { pending: same ? chat.pending : chat.pending.map((p) => ({ ...p, status: 'failed' as const, retryable: false, error: 'The conversation changed while disconnected. Check before retrying.' })), draft: chat.draft ?? draft ?? undefined }),
    }
  })
}

async function finish(host: string, pane: string, operation: ClearingConversation) {
  const key = chatKey(host, pane)
  const target = operation.target ?? pane
  patchChat(key, () => ({ clearing: { ...operation, phase: 'refreshing' } }))
  const unsubscribe = socket.subscribeChat(host, target)
  try {
    if (operation.options.closed) {
      await api.refresh()
      patchChat(key, () => ({ clearing: undefined }))
      operation.onReady?.(target)
      return true
    }
    const result = await api.refreshConversation(host, target, operation.options)
    if (!current(key, operation) || operation.reconnect && !reachable(host)) return false
    applySnapshot(host, target, result, operation.reconnect)
    patchChat(key, () => ({ clearing: undefined, rewindFlow: operation.rewind && !!result.screen.prompt }))
    if (target !== pane) inheritReadingPosition(key, chatKey(host, target))
    operation.onReady?.(target)
    return true
  } finally { unsubscribe() }
}

/** One mutation, then one authoritative read. Never retry a potentially destructive command. */
export async function changeConversation(
  host: string, pane: string, label: string, action: () => Promise<unknown>,
  options: RefreshOptions = {}, extra: Pick<ClearingConversation, 'rewind' | 'onReady'> = {},
) {
  if (isClearing(host, pane)) return false
  const key = chatKey(host, pane)
  const operation: ClearingConversation = { id: ++operationId, phase: 'clearing', label, options, retained: retained(useStore.getState().chats[key]), ...extra }
  patchChat(key, () => ({ clearing: operation }))
  const unsubscribe = socket.subscribeChat(host, pane)
  try {
    const result = await action()
    if (result && typeof result === 'object' && 'pane' in result && typeof result.pane === 'string') operation.target = result.pane
    return await finish(host, pane, operation)
  } catch (e) {
    patchChat(key, () => ({ clearing: { ...operation, phase: 'failed', error: (e as Error).message } }))
    return false
  } finally { unsubscribe() }
}

export function clearConversation(host: string, pane: string, command: string) {
  const state = useStore.getState()
  const info = state.chats[chatKey(host, pane)]?.info
  const agent = state.hosts.find((h) => h.config.id === host)?.sessions.flatMap((s) => s.windows)
    .flatMap((w) => w.panes).find((p) => p.id === pane)?.agent
  return changeConversation(host, pane, 'Clearing conversation', () => api.send(host, pane, command, `clear-${crypto.randomUUID()}`, true),
    { agent: info?.agent ?? agent?.kind, differentFrom: info?.sessionId ?? agent?.sessionId })
}

export function sendTransition(host: string, pane: string, agent: AgentKind | undefined, text: string) {
  if (isClearCommand(agent, text)) return clearConversation(host, pane, text)
  const rewind = text.trim() === '/rewind'
  const stopped = /^\/(exit|quit)$/.test(text.trim())
  const side = agent === 'codex' && /^\/(btw|side)(?:\s|$)/.test(text.trim())
  const info = useStore.getState().chats[chatKey(host, pane)]?.info
  const differentFrom = side && !info?.transcriptPath?.startsWith('codex-side:') ? info?.sessionId : undefined
  return changeConversation(host, pane, transitionCommand(agent, text) ?? 'Updating conversation',
    () => api.send(host, pane, text, `change-${crypto.randomUUID()}`, true),
    { agent, stopped, allowPrompt: !stopped, allowWorking: side, ...(differentFrom && { differentFrom }) }, { rewind })
}

/** Recovery only reads status; it never repeats clear, rewind, fork or another mutation. */
export async function refreshClearedConversation(host: string, pane: string) {
  const key = chatKey(host, pane)
  const operation = useStore.getState().chats[key]?.clearing
  if (!operation || operation.phase !== 'failed') return
  if (operation.reconnect) return recoverConnection(host, pane)
  try { await finish(host, pane, operation) }
  catch (e) { if (current(key, operation)) patchChat(key, () => ({ clearing: { ...operation, phase: 'failed', error: (e as Error).message } })) }
}

/** Hold the last readable view immediately; reconnecting never repeats a send. */
export function waitForConnection(host: string, pane: string) {
  const key = chatKey(host, pane)
  const chat = useStore.getState().chats[key]
  if (!chat || chat.clearing && !chat.clearing.reconnect) return
  patchChat(key, () => ({ clearing: {
    id: ++operationId, phase: 'refreshing', label: 'Reconnecting', reconnect: true,
    retained: chat.clearing?.retained ?? retained(chat),
    options: { allowWorking: true, allowPrompt: true, readOnly: true },
  } }))
}

const recovering = new Map<string, Promise<void>>()
export function recoverConnection(host: string, pane: string): Promise<void> | undefined {
  const key = chatKey(host, pane)
  const operation = useStore.getState().chats[key]?.clearing
  if (!reachable(host) || !operation?.reconnect) return
  if (recovering.has(key)) return recovering.get(key)
  const work = (async () => {
    try { await finish(host, pane, operation) }
    catch (e) {
      if (current(key, operation)) patchChat(key, () => ({ clearing: { ...operation, phase: 'failed', error: (e as Error).message } }))
    } finally {
      recovering.delete(key)
      const next = useStore.getState().chats[key]?.clearing
      if (next?.reconnect && next.id !== operation.id) void recoverConnection(host, pane)
    }
  })()
  recovering.set(key, work)
  return work
}
