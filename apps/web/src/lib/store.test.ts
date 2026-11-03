import { describe, expect, jest, setSystemTime, test, afterEach } from 'bun:test'
import type { ChatItem } from '@inyon/protocol'
import { ANSWERS_WAIT_MS, hydrateChat, notifyTag, patchChat, PENDING_MS, rememberSources, useStore, type PendingMessage } from './store.ts'
import { clearNativeNotification } from './native.ts'
import { closeChat, emit } from '../test/fakes.ts'
import { agentPane, chatInfo, host, item } from '../test/fixtures.ts'

const chat = (key: string) => useStore.getState().chats[key]
const seqs = (items: ChatItem[]) => items.map((i) => i.seq)
const reset = (key: string, items: ChatItem[], hasMore = false) => emit({ t: 'chat.reset', key, info: chatInfo(), items, hasMore })

afterEach(() => setSystemTime())

describe('hub messages', () => {
  test('overview replaces the machines', () => {
    expect(useStore.getState().hostsLoaded).toBe(false)
    emit({ t: 'overview', hosts: [host('a'), host('b')] })
    expect(useStore.getState().hosts.map((h) => h.config.id)).toEqual(['a', 'b'])
    expect(useStore.getState().hostsLoaded).toBe(true)
  })

  test("a paired hub's remembered machines stay while the phone's own core answers first", () => {
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'studio', url: 'http://box:4777', token: 't', pairedAt: 1 }]))
    rememberSources([host('own'), host('hub:h1/srv')])
    emit({ t: 'overview', hosts: [host('own'), host('new')] })
    expect(useStore.getState().hosts.map((h) => h.config.id).sort()).toEqual(['hub:h1/srv', 'new', 'own'])
    // the hub answers: its part is replaced in turn
    emit({ t: 'overview', hosts: [], source: 'hub:h1' } as any)
    expect(useStore.getState().hosts.map((h) => h.config.id).sort()).toEqual(['new', 'own'])
    rememberSources([])
    localStorage.removeItem('inyon.hubs.v1')
  })

  test('chat.reset loads the transcript', () => {
    const items = [item.user('hi'), item.text('hello')]
    emit({ t: 'chat.reset', key: 'k', info: chatInfo({ title: 'T' }), items, hasMore: true })
    expect(chat('k')).toMatchObject({ loaded: true, hasMore: true, items, info: { title: 'T' } })
  })

  test('live text is kept until the record with the same id arrives, then goes with it', () => {
    reset('live', [item.user('say hi')])
    emit({ t: 'chat.live', key: 'live', items: [{ id: 'msg_1', kind: 'text', text: 'Hi ag' }] })
    emit({ t: 'chat.live', key: 'live', items: [{ id: 'msg_1', kind: 'text', text: 'Hi again!' }, { id: 'rs_1', kind: 'thinking', text: 'brief' }] })
    expect(chat('live').live).toEqual([{ id: 'msg_1', kind: 'text', text: 'Hi again!' }, { id: 'rs_1', kind: 'thinking', text: 'brief' }])
    // the message lands in the record: its live copy is dropped in the same update, the other stays
    const recorded: ChatItem = { ...(item.text('Hi again!') as Extract<ChatItem, { kind: 'assistant' }>), uuid: 'msg_1', msgId: 'msg_1' }
    emit({ t: 'chat.append', key: 'live', items: [recorded] })
    expect(chat('live').live).toEqual([{ id: 'rs_1', kind: 'thinking', text: 'brief' }])
    // a live list that names something already recorded never shows it
    emit({ t: 'chat.live', key: 'live', items: [{ id: 'msg_1', kind: 'text', text: 'Hi again!' }] })
    expect(chat('live').live).toEqual([])
    // the hub says nothing is being written
    emit({ t: 'chat.live', key: 'live', items: [] })
    expect(chat('live').live).toEqual([])
  })

  test('text read off the screen gives way to a new reply on record, and comes back only if the hub sends it again', () => {
    reset('scr', [item.user('tea?'), item.text('An older reply.')])
    emit({ t: 'chat.live', key: 'scr', items: [{ id: 'screen:1', kind: 'text', text: 'Tea likely began in China' }] })
    // records already there (an older reply) don't hide it
    expect(chat('scr').live).toEqual([{ id: 'screen:1', kind: 'text', text: 'Tea likely began in China' }])
    // a tool call on record isn't the reply: it stays
    emit({ t: 'chat.append', key: 'scr', items: [{ ...(item.text('x') as Extract<ChatItem, { kind: 'assistant' }>), seq: 100_050, block: { type: 'tool_use', id: 't1', name: 'Bash', input: {} } }] })
    expect(chat('scr').live).toHaveLength(1)
    // the reply lands: the screen copy goes in the same update
    emit({ t: 'chat.append', key: 'scr', items: [{ ...item.text('Tea likely began in China, where...'), seq: 100_051 }] })
    expect(chat('scr').live).toEqual([])
    // what's still being written after it comes back from the hub
    emit({ t: 'chat.live', key: 'scr', items: [{ id: 'screen:1', kind: 'text', text: 'During the Tang dynasty' }] })
    expect(chat('scr').live).toEqual([{ id: 'screen:1', kind: 'text', text: 'During the Tang dynasty' }])
    // Antigravity and Gemini send the whole conversation again: a new reply in it does the same
    const all = chat('scr').items
    emit({ t: 'chat.reset', key: 'scr', info: chatInfo({ title: 'T' }), items: all, hasMore: false })
    expect(chat('scr').live).toHaveLength(1)
    emit({ t: 'chat.reset', key: 'scr', info: chatInfo({ title: 'T' }), items: [...all, { ...item.text('During the Tang dynasty, tea'), seq: 100_060 }], hasMore: false })
    expect(chat('scr').live).toEqual([])
  })

  test('a resent latest page keeps the earlier messages already loaded', () => {
    const old = [item.user('1'), item.text('2'), item.user('3'), item.text('4')]
    reset('k', old, true)
    emit({ t: 'chat.more', key: 'k', items: [], hasMore: false })
    // the hub resends from item 3 on, with one new item
    const next = [old[2], old[3], item.text('5')]
    reset('k', next, true)
    expect(seqs(chat('k').items)).toEqual([...seqs(old), next[2].seq])
    // the reader already reached the start; that stays known
    expect(chat('k').hasMore).toBe(false)
  })

  test('a page that starts somewhere unknown replaces everything', () => {
    reset('k', [item.user('1'), item.text('2')], true)
    const other = [item.user('x'), item.text('y')]
    reset('k', other, true)
    expect(chat('k').items).toEqual(other)
  })

  test('chat.append adds only items it has not seen', () => {
    const a = item.user('a')
    const b = item.text('b')
    reset('k', [a, b])
    const c = item.text('c')
    emit({ t: 'chat.append', key: 'k', items: [b, c] })
    expect(seqs(chat('k').items)).toEqual([a.seq, b.seq, c.seq])
  })

  test('chat.more prepends older items without duplicates', () => {
    const [a, b, c] = [item.user('a'), item.text('b'), item.user('c')]
    reset('k', [b, c], true)
    emit({ t: 'chat.more', key: 'k', items: [a, b], hasMore: false })
    expect(seqs(chat('k').items)).toEqual([a.seq, b.seq, c.seq])
    expect(chat('k').hasMore).toBe(false)
  })

  test('chat.info without a transcript counts as loaded; with one, it waits for the items', () => {
    emit({ t: 'chat.info', key: 'none', info: chatInfo() })
    expect(chat('none').loaded).toBe(true)
    emit({ t: 'chat.info', key: 'some', info: chatInfo({ transcriptPath: '/t.jsonl' }) })
    expect(chat('some').loaded).toBe(false)
    reset('some', [])
    emit({ t: 'chat.info', key: 'some', info: chatInfo({ transcriptPath: '/t.jsonl' }) })
    expect(chat('some').loaded).toBe(true)
  })

  test('chat.screen and chat.draft land on the chat, keeping the rest', () => {
    const items = [item.user('a')]
    reset('k', items)
    emit({ t: 'chat.screen', key: 'k', screen: { working: true, width: 80 } })
    emit({ t: 'chat.draft', key: 'k', text: 'half typed', source: 'terminal', at: 5 })
    expect(chat('k')).toMatchObject({ items, screen: { working: true }, draft: { text: 'half typed', source: 'terminal', at: 5 } })
  })
})

describe('pending messages', () => {
  const T = Date.UTC(2026, 9, 11, 12)
  const p = (text: string, more: Partial<PendingMessage> = {}): PendingMessage => ({ id: text, text, status: 'sent', at: T, ...more })
  const ts = new Date(T + 1000).toISOString()

  test('the transcript confirms them on reset and on append', () => {
    patchChat('k', () => ({ pending: [p('one'), p('two'), p('three')] }))
    reset('k', [item.user('one', { ts })])
    expect(chat('k').pending.map((m) => m.text)).toEqual(['two', 'three'])
    emit({ t: 'chat.append', key: 'k', items: [item.user('three', { ts })] })
    expect(chat('k').pending.map((m) => m.text)).toEqual(['two'])
  })

  test('a reappearing old item does not confirm a new send', () => {
    const old = item.user('yes', { ts })
    reset('k', [old])
    patchChat('k', () => ({ pending: [p('yes')] }))
    emit({ t: 'chat.append', key: 'k', items: [old] })
    expect(chat('k').pending).toHaveLength(1)
  })

  test('they survive a reload; one that was mid-send says delivery was interrupted', () => {
    setSystemTime(T + 60_000)
    patchChat('k', () => ({ pending: [p('sending', { status: 'sending' }), p('failed', { status: 'failed', error: 'boom' }), p('sent')] }))
    const saved = sessionStorage.getItem('inyon.pending.k')
    expect(JSON.parse(saved!)).toHaveLength(3)
    // a fresh page: same tab storage, empty store
    useStore.setState({ chats: {} })
    patchChat('k', () => ({}))
    expect(chat('k').pending.map((m) => [m.text, m.status, m.error])).toEqual([
      ['sending', 'failed', 'Delivery was interrupted. Check the terminal before retrying.'],
      ['failed', 'failed', 'boom'],
      ['sent', 'sent', undefined],
    ])
  })

  test('old or malformed saved messages are dropped', () => {
    setSystemTime(T + PENDING_MS + 1)
    sessionStorage.setItem('inyon.pending.k', JSON.stringify([p('stale'), { id: 1, text: 'bad' }, null, p('fresh', { at: T + PENDING_MS })]))
    patchChat('k', () => ({}))
    expect(chat('k').pending.map((m) => m.text)).toEqual(['fresh'])
    sessionStorage.setItem('inyon.pending.j', '{not json')
    patchChat('j', () => ({}))
    expect(chat('j').pending).toEqual([])
  })

  test('the saved copy goes away once nothing is pending', () => {
    patchChat('k', () => ({ pending: [p('a')] }))
    patchChat('k', () => ({ pending: [] }))
    expect(sessionStorage.getItem('inyon.pending.k')).toBeNull()
  })
})

describe('the companion agent', () => {
  test('its reply shows once as a toast that opens its chat, cut short when long', () => {
    useStore.setState({ toasts: [] })
    const reply = { t: 'companion.reply' as const, id: 'r-1', host: 'local', pane: '%9', text: 'Codex on the laptop needs you.', at: 1 }
    emit(reply)
    emit(reply)
    expect(useStore.getState().toasts.map((t) => [t.title, t.body, t.href, t.tone])).toEqual([['Companion', 'Codex on the laptop needs you.', '/m/local/p/9', 'info']])
    // with its agent's mark, once the overview shows the pane
    useStore.setState({ hosts: [host('local', { panes: [agentPane('%9', 'codex', { companion: true })] })] })
    emit({ ...reply, id: 'r-1b' })
    expect(useStore.getState().toasts.at(-1)!.agent).toBe('codex')
    emit({ ...reply, id: 'r-2', text: 'word '.repeat(80), error: true })
    const last = useStore.getState().toasts.at(-1)!
    expect(last.tone).toBe('error')
    expect(last.body!.length).toBeLessThanOrEqual(241)
    expect(last.body!.endsWith('…')).toBe(true)
  })

  test('its open chat holds the toast back, its terminal does not', () => {
    const reply = { t: 'companion.reply' as const, host: 'local', pane: '%9', text: 'Done.', at: 1 }
    try {
      history.replaceState(null, '', '/m/local/p/9')
      useStore.setState({ toasts: [] })
      emit({ ...reply, id: 'v-1' })
      expect(useStore.getState().toasts).toEqual([])
      history.replaceState(null, '', '/m/local/p/9?view=terminal')
      emit({ ...reply, id: 'v-2' })
      expect(useStore.getState().toasts.map((t) => t.body)).toEqual(['Done.'])
    } finally {
      history.replaceState(null, '', '/')
    }
  })
})

describe('toasts and settings', () => {
  test('at most three toasts at a time, newest last; one per conversation', () => {
    const { toast } = useStore.getState()
    for (const n of [1, 2, 3, 4]) toast({ title: `t${n}`, tone: 'info' })
    expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['t2', 't3', 't4'])
    useStore.getState().dismiss(useStore.getState().toasts[0].id)
    expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['t3', 't4'])
    toast({ title: 'working', tone: 'info', href: '/m/box/p/1' })
    toast({ title: 'finished', tone: 'success', href: '/m/box/p/1' })
    expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['t3', 't4', 'finished'])
  })

  test('settings merge and persist', () => {
    useStore.getState().setSettings({ density: 'compact' })
    expect(useStore.getState().settings).toMatchObject({ density: 'compact', theme: 'system' })
    expect(JSON.parse(localStorage.getItem('inyon.settings.v2')!)).toMatchObject({ density: 'compact', haptics: true })
  })
})

test("a chat left a while ago doesn't show its old goal when opened again", () => {
  jest.useFakeTimers()
  try {
    emit({ t: 'chat.reset', key: 'g', info: chatInfo({ title: 'T', goal: { condition: 'ship it', met: false } as any }), items: [item.user('hi')], hasMore: false })
    closeChat('g')
    jest.advanceTimersByTime(61_000)
    // the title stays for the header while it reloads; the goal waits for the hub to say it's still on
    expect(chat('g')).toMatchObject({ items: [], loaded: false, info: { title: 'T' } })
    expect(chat('g').info?.goal).toBeUndefined()
  } finally {
    jest.useRealTimers()
  }
})

describe('page notices', () => {
  const moved = (host = 'laptop') => emit({ t: 'page.notify', host, slug: 'robot', title: 'Robot', body: '3 of 12 done · Payment sheet' })
  afterEach(() => history.pushState({}, '', '/'))

  test("a page's board moving on toasts with a link to the page, unless that page is open", () => {
    moved('hub:h1/box')
    expect(useStore.getState().toasts[0]).toMatchObject({ title: 'Robot', body: '3 of 12 done · Payment sheet', href: '/pages/hub%3Ah1%2Fbox/robot' })
    expect(useStore.getState().activity).toEqual([])
    useStore.setState({ toasts: [] })
    history.pushState({}, '', '/pages/laptop/robot')
    moved()
    expect(useStore.getState().toasts).toEqual([])
  })

  test('page notices can be turned off', () => {
    useStore.getState().setSettings({ notifyPages: false })
    moved()
    expect(useStore.getState().toasts).toEqual([])
    useStore.getState().setSettings({ notifyPages: true })
  })

  test("in the phone app's background: one native notification per page that opens it", async () => {
    const posted: any[] = []
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'android',
      Plugins: { InyonDevice: { postNotification: async (o: any) => void posted.push(o) } },
    }
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    try {
      moved()
      await Promise.resolve()
      expect(posted).toEqual([{ title: 'Robot', body: '3 of 12 done · Payment sheet', href: '/pages/laptop/robot', tag: 'page:laptop/robot', kind: 'page' }])
      expect(useStore.getState().toasts).toEqual([])
    } finally {
      delete (window as any).Capacitor
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })
})

describe('notify', () => {
  const note = (kind: 'waiting' | 'done' | 'offline' | 'online', title = 'Claude needs you') => emit({ t: 'notify', host: 'box', pane: '%3', kind, title, body: 'b', agent: 'claude' })
  afterEach(() => history.pushState({}, '', '/'))

  test('agent events go in the activity list and toast with a link to the session', () => {
    note('waiting')
    expect(useStore.getState().activity[0]).toMatchObject({ host: 'box', pane: '%3', kind: 'waiting' })
    expect(useStore.getState().toasts[0]).toMatchObject({ title: 'Claude needs you', tone: 'warn', href: '/m/box/p/3' })
  })

  test('machine events toast but are not activity', () => {
    note('offline', 'box is offline')
    expect(useStore.getState().activity).toEqual([])
    expect(useStore.getState().toasts[0]).toMatchObject({ tone: 'warn', href: '/machines/box' })
  })

  test('no toast for the conversation on screen', () => {
    history.pushState({}, '', '/m/box/p/3')
    note('done')
    expect(useStore.getState().toasts).toEqual([])
    expect(useStore.getState().activity).toHaveLength(1)
  })

  test('finished notices can be turned off; they still go in the activity list', () => {
    useStore.getState().setSettings({ notifyDone: false })
    note('done')
    expect(useStore.getState().toasts).toEqual([])
    expect(useStore.getState().activity).toHaveLength(1)
    note('waiting')
    expect(useStore.getState().toasts).toHaveLength(1)
  })

  test('the activity list keeps the latest 30', () => {
    for (let i = 0; i < 35; i++) note('done', `n${i}`)
    const titles = useStore.getState().activity.map((a) => a.title)
    expect(titles).toHaveLength(30)
    expect(titles[0]).toBe('n34')
  })

  test("in the phone app's background: one native notification per session, with the logo, cleared on opening", async () => {
    const posted: any[] = []
    const cleared: string[] = []
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'android',
      Plugins: { InyonDevice: { postNotification: async (o: any) => void posted.push(o), clearNotification: async (o: any) => void cleared.push(o.tag) } },
    }
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    try {
      note('waiting')
      emit({ t: 'notify', host: 'box', pane: '', kind: 'offline', title: 'box is offline', body: 'b' })
      // a question's notification waits a moment for its answers
      await new Promise((r) => setTimeout(r, ANSWERS_WAIT_MS + 50))
      expect(posted).toEqual([
        { title: 'box is offline', body: 'b', href: '/machines/box', tag: 'machine:box', kind: 'offline' },
        { title: 'Claude needs you', body: 'b', href: '/m/box/p/3', tag: 'box:%3', kind: 'waiting' },
      ])
      clearNativeNotification(notifyTag('box', '%3'))
      expect(cleared).toEqual(['box:%3'])
    } finally {
      delete (window as any).Capacitor
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })

  test("a question's notification offers its answers as buttons: one to three, and none once it's answered", async () => {
    const posted: any[] = []
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'android',
      Plugins: { InyonDevice: { postNotification: async (o: any) => void posted.push(o), clearNotification: async () => {} } },
    }
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    const asks = (labels: string[]) =>
      host('box', { panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state: 'waiting', waitingFor: 'Run git push?', options: labels.map((label, i) => ({ n: i + 1, label })) } })] })
    const settle = () => new Promise((r) => setTimeout(r, ANSWERS_WAIT_MS + 50))
    try {
      // the overview lands after the notice, as the hub sends them
      note('waiting')
      emit({ t: 'overview', hosts: [asks(['Yes', "Yes, and don't ask again", 'No'])] })
      await settle()
      expect(posted.at(-1)).toMatchObject({ tag: 'box:%3', kind: 'waiting', host: 'box', pane: '%3', options: [{ n: 1, label: 'Yes' }, { n: 2, label: "Yes, and don't ask again" }, { n: 3, label: 'No' }] })
      // four answers don't fit: just Open
      note('waiting')
      emit({ t: 'overview', hosts: [asks(['A', 'B', 'C', 'D'])] })
      await settle()
      expect(posted.at(-1).options).toBeUndefined()
      // answered on the computer before the notification went up: none at all
      const before = posted.length
      note('waiting')
      emit({ t: 'overview', hosts: [host('box', { panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state: 'working' } })] })] })
      await settle()
      expect(posted.length).toBe(before)
    } finally {
      delete (window as any).Capacitor
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })

  test('a question answered somewhere else takes its notification with it', async () => {
    const cleared: string[] = []
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'android',
      Plugins: { InyonDevice: { postNotification: async () => ({ ok: true }), clearNotification: async (o: any) => void cleared.push(o.tag) } },
    }
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    const box = (state: 'waiting' | 'working' | 'idle', status: 'online' | 'error' = 'online') => host('box', { status, panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state } })] })
    try {
      note('waiting')
      emit({ t: 'overview', hosts: [box('waiting')] })
      // still asking, or the machine went quiet: it stays
      emit({ t: 'overview', hosts: [box('working', 'error')] })
      expect(cleared).toEqual([])
      // answered on the computer
      emit({ t: 'overview', hosts: [box('working')] })
      expect(cleared).toEqual(['box:%3'])
      emit({ t: 'overview', hosts: [box('idle')] })
      expect(cleared).toEqual(['box:%3'])
      // a finished notice replaced it: nothing to take back
      note('waiting')
      note('done')
      emit({ t: 'overview', hosts: [box('idle')] })
      expect(cleared).toEqual(['box:%3'])
    } finally {
      delete (window as any).Capacitor
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })
  test("in a browser tab: an answered question's notification is closed too, and a newer one replaces the old", () => {
    const made: { title: string; tag?: string; closed: boolean }[] = []
    class FakeNotification {
      static permission = 'granted'
      onclick: (() => void) | null = null
      onclose: (() => void) | null = null
      entry: (typeof made)[number]
      constructor(title: string, o?: NotificationOptions) {
        this.entry = { title, tag: o?.tag, closed: false }
        made.push(this.entry)
      }
      close() {
        this.entry.closed = true
      }
    }
    const was = (window as any).Notification
    ;(window as any).Notification = FakeNotification
    ;(globalThis as any).Notification = FakeNotification
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    const box = (state: 'waiting' | 'working') => host('box', { panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state } })] })
    try {
      note('waiting', 'first question')
      note('waiting', 'second question')
      expect(made.map((n) => [n.title, n.tag, n.closed])).toEqual([['first question', 'box:%3', true], ['second question', 'box:%3', false]])
      emit({ t: 'overview', hosts: [box('waiting')] })
      expect(made[1].closed).toBe(false)
      // answered on the computer
      emit({ t: 'overview', hosts: [box('working')] })
      expect(made[1].closed).toBe(true)
    } finally {
      ;(window as any).Notification = was
      ;(globalThis as any).Notification = was
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })
})

describe('fewer renders and a first paint from the device', () => {
  test('an overview keeps the objects of what did not change', () => {
    emit({ t: 'overview', hosts: [host('a', { panes: [agentPane('%1', 'claude')] }), host('b')] })
    const before = useStore.getState().hosts
    emit({ t: 'overview', hosts: [host('a', { panes: [agentPane('%1', 'claude', { title: 'new' })] }), host('b')] })
    const after = useStore.getState().hosts
    expect(after).not.toBe(before)
    expect(after[1]).toBe(before[1])
    expect(after[0]!.config).toBe(before[0]!.config)
    expect(after[0]!.sessions[0]!.windows[0]!.panes[0]!.title).toBe('new')
    expect(useStore.getState().hostsStale).toBe(false)
  })

  test('a chat page kept on the device shows until the hub sends the real one', async () => {
    const { flushSnapshots } = await import('./snapshot.ts')
    reset('kept', [item.user('a'), item.text('b')])
    flushSnapshots()
    useStore.setState({ chats: {} })
    hydrateChat('kept')
    expect(chat('kept')).toMatchObject({ loaded: true, stale: true })
    expect(seqs(chat('kept')!.items)).toHaveLength(2)
    reset('kept', [item.user('c')])
    expect(chat('kept')!.stale).toBeUndefined()
    expect(seqs(chat('kept')!.items)).toHaveLength(1)
  })
})
