import { chatKey, type AgentKind, type ChatInfo } from '@inyon/protocol'
import { api } from './api.ts'
import { socket } from './socket.ts'
import { patchChat, useStore } from './store.ts'

export interface ClearingConversation {
  phase: 'clearing' | 'refreshing' | 'failed'
  before?: string
  error?: string
}

export const isClearing = (host: string, pane: string) => !!useStore.getState().chats[chatKey(host, pane)]?.clearing

export function isClearCommand(agent: AgentKind | undefined, text: string) {
  return agent === 'codex' ? /^\/(clear|new)$/.test(text.trim()) : agent === 'claude' && text.trim() === '/clear'
}

function newConversation(info: ChatInfo, before?: string) {
  // Codex may not allocate the new thread until its first message. The backend confirms
  // the native empty screen before the send returns; a missing ID is then a valid reset.
  return info.running && (!before || info.sessionId !== before)
}

function refreshSnapshot(host: string, pane: string, before?: string) {
  const key = chatKey(host, pane)
  return new Promise<void>((resolve, reject) => {
    const stop = socket.on((m) => {
      if (m.t !== 'chat.reset' || m.key !== key || !newConversation(m.info, before)) return
      clearTimeout(timer)
      stop()
      resolve()
    })
    const timer = setTimeout(() => {
      stop()
      reject(new Error('The refreshed conversation has not arrived yet. Check the connection and refresh its status.'))
    }, 30_000)
    socket.refreshChat(host, pane)
  })
}

/** One operation shared by the composer and both Clear menus, surviving navigation away. */
export async function clearConversation(host: string, pane: string, command: string) {
  if (isClearing(host, pane)) return
  const key = chatKey(host, pane)
  const state = useStore.getState()
  const before = state.chats[key]?.info?.sessionId ?? state.hosts.find((h) => h.config.id === host)?.sessions
    .flatMap((s) => s.windows).flatMap((w) => w.panes).find((p) => p.id === pane)?.agent?.sessionId
  patchChat(key, () => ({ clearing: { phase: 'clearing', before } }))
  const unsubscribe = socket.subscribeChat(host, pane)
  try {
    // No automatic retry of a destructive command after an ambiguous network failure.
    await api.send(host, pane, command, `clear-${crypto.randomUUID()}`, true)
    patchChat(key, () => ({ clearing: { phase: 'refreshing', before } }))
    await refreshSnapshot(host, pane, before)
    patchChat(key, () => ({ clearing: undefined, pending: [], live: [], draft: { text: '', source: 'app', at: Date.now() } }))
  } catch (e) {
    patchChat(key, () => ({ clearing: { phase: 'failed', before, error: (e as Error).message } }))
  } finally {
    unsubscribe()
  }
}

/** Recover a delayed response by reading status again, never by sending /clear a second time. */
export async function refreshClearedConversation(host: string, pane: string) {
  const key = chatKey(host, pane)
  const operation = useStore.getState().chats[key]?.clearing
  if (!operation || operation.phase !== 'failed') return
  patchChat(key, () => ({ clearing: { ...operation, phase: 'refreshing', error: undefined } }))
  const unsubscribe = socket.subscribeChat(host, pane)
  try {
    await api.refresh()
    await refreshSnapshot(host, pane, operation.before)
    patchChat(key, () => ({ clearing: undefined, pending: [], live: [], draft: { text: '', source: 'app', at: Date.now() } }))
  } catch (e) {
    patchChat(key, () => ({ clearing: { ...operation, phase: 'failed', error: (e as Error).message } }))
  } finally {
    unsubscribe()
  }
}
