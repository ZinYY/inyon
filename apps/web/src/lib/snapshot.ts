import type { ChatInfo, ChatItem, HostState, Organize } from '@inyon/protocol'

/**
 * The last overview and the last few chats, kept on the device so the app paints real content
 * in its first frame and the live data then replaces it in place. Nothing here is trusted
 * beyond drawing: every action still goes to the hub, and the next message from it wins.
 */
const OVERVIEW_KEY = 'inyon.overview.v1'
const ORGANIZE_KEY = 'inyon.organize.v1'
const CHAT_PREFIX = 'inyon.chat.v1.'
const CHAT_INDEX = 'inyon.chats.v1'
/** older than this and the picture is more misleading than helpful */
const MAX_AGE_MS = 7 * 24 * 3600_000
/** a chat page holds at most this many messages and bytes */
const CHAT_ITEMS = 40
const MAX_BYTES = 300_000
/** chats kept, most recently shown first */
const CHATS_KEPT = 3

export interface ChatSnapshot {
  info?: ChatInfo
  items: ChatItem[]
  hasMore: boolean
}

function read<T>(key: string): T | undefined {
  try {
    const raw = localStorage.getItem(key)
    if (!raw) return
    const v = JSON.parse(raw) as { at: number; data: T }
    if (!v || typeof v.at !== 'number' || Date.now() - v.at > MAX_AGE_MS) return
    return v.data
  } catch {
    return
  }
}

/** Keeps a picture. With `orDrop`, one that can't be kept takes the old one away with it, so a
 * later launch doesn't show something out of date as if it were current. */
function write(key: string, data: unknown, orDrop = false) {
  try {
    const raw = JSON.stringify({ at: Date.now(), data })
    if (raw.length > MAX_BYTES) throw new Error('too big')
    localStorage.setItem(key, raw)
  } catch {
    if (orDrop) {
      try {
        localStorage.removeItem(key)
      } catch {}
    }
  }
}

const waiting = new Map<string, { timer: ReturnType<typeof setTimeout>; fn: () => void }>()
/** Writes a little after the last change, not on every message. */
function later(key: string, ms: number, fn: () => void) {
  clearTimeout(waiting.get(key)?.timer)
  const timer = setTimeout(() => {
    waiting.delete(key)
    fn()
  }, ms)
  waiting.set(key, { timer, fn })
}

export const loadOverview = () => read<HostState[]>(OVERVIEW_KEY)
export const saveOverview = (hosts: HostState[]) => later(OVERVIEW_KEY, 1000, () => write(OVERVIEW_KEY, hosts))

/** each hub's pins and parked sessions, so the first frame already has them in their places */
export const loadOrganize = () => read<Record<string, Organize>>(ORGANIZE_KEY)
export const saveOrganize = (by: Record<string, Organize>) => later(ORGANIZE_KEY, 1000, () => write(ORGANIZE_KEY, by, true))

export const loadChat = (key: string) => read<ChatSnapshot>(CHAT_PREFIX + key)

export function saveChat(key: string, chat: ChatSnapshot) {
  later(CHAT_PREFIX + key, 1500, () => {
    const items = chat.items.slice(-CHAT_ITEMS)
    write(CHAT_PREFIX + key, { info: chat.info, items, hasMore: chat.hasMore || items.length < chat.items.length })
    // only the last few chats stay; the rest reload from the hub as before
    const index = [key, ...chatIndex().filter((k) => k !== key)]
    for (const old of index.slice(CHATS_KEPT)) localStorage.removeItem(CHAT_PREFIX + old)
    try {
      localStorage.setItem(CHAT_INDEX, JSON.stringify(index.slice(0, CHATS_KEPT)))
    } catch {}
  })
}

function chatIndex(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(CHAT_INDEX) ?? '[]')
    return Array.isArray(v) ? v.filter((k) => typeof k === 'string') : []
  } catch {
    return []
  }
}

/** Signing out takes the pictures with it. */
export function clearSnapshots() {
  for (const w of waiting.values()) clearTimeout(w.timer)
  waiting.clear()
  try {
    for (const k of chatIndex()) localStorage.removeItem(CHAT_PREFIX + k)
    localStorage.removeItem(CHAT_INDEX)
    localStorage.removeItem(OVERVIEW_KEY)
    localStorage.removeItem(ORGANIZE_KEY)
  } catch {}
}

/** Writes everything that is waiting now (tests, and the page going away). */
export function flushSnapshots() {
  for (const [key, w] of waiting) {
    clearTimeout(w.timer)
    waiting.delete(key)
    w.fn()
  }
}
