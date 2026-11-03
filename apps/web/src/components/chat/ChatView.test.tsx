import { afterAll, describe, expect, mock, spyOn, test } from 'bun:test'
import { act, render, screen } from '@testing-library/react'
import type { ChatItem, SendState } from '@inyon/protocol'
import { ChatView, reconnect } from './ChatView.tsx'
import { GoalRow } from './Messages.tsx'
import { buildRows } from '../../lib/chat.ts'
import { patchChat, useStore } from '../../lib/store.ts'
import { api } from '../../lib/api.ts'
import { socket } from '../../lib/socket.ts'
import { emit } from '../../test/fakes.ts'
import { agentPane, chatInfo, host, item } from '../../test/fixtures.ts'
import { flush } from '../../test/render.tsx'

// bun keeps a spy in place for every test file after this one
afterAll(() => mock.restore())

const goalRow = (i: ChatItem) => {
  const [row] = buildRows([i])
  if (row.type !== 'goal') throw new Error('not a goal row')
  return <GoalRow row={row} />
}

describe('GoalRow', () => {
  test('set, met and cleared', () => {
    const { rerender } = render(goalRow(item.goal('all tests pass')))
    expect(screen.getByText('Goal set')).toBeTruthy()
    expect(screen.getByText('all tests pass')).toBeTruthy()
    rerender(goalRow(item.goal('all tests pass', { met: true, reason: '42 passed' })))
    expect(screen.getByText('Goal met')).toBeTruthy()
    expect(screen.getByText('42 passed')).toBeTruthy()
    rerender(goalRow(item.goal('all tests pass', { cleared: true })))
    expect(screen.getByText('Goal cleared')).toBeTruthy()
  })

  test('met wins over cleared, and a reason only shows once met', () => {
    const { rerender } = render(goalRow(item.goal('x', { met: true, cleared: true })))
    expect(screen.getByText('Goal met')).toBeTruthy()
    rerender(goalRow(item.goal('x', { reason: 'not yet' })))
    expect(screen.queryByText('not yet')).toBeNull()
  })
})

/** A conversation with every kind of row, many times over. */
function longConversation(turns: number): ChatItem[] {
  const items: ChatItem[] = []
  for (let t = 0; t < turns; t++) {
    items.push(
      item.user(`Question ${t}: look at /home/me/code/app.ts please`),
      item.thinking('Let me look.'),
      item.tool(`b${t}`, 'Bash', { command: 'ls -la', description: 'List files' }),
      item.result(`b${t}`, 'total 0'),
      item.tool(`e${t}`, 'Edit', { file_path: '/home/me/code/app.ts', old_string: 'a', new_string: 'b' }),
      { ...item.result(`e${t}`, 'ok'), meta: { filePath: '/home/me/code/app.ts', patch: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ['-a', '+b'] }] } } as ChatItem,
      item.tool(`r${t}`, 'Read', { file_path: '/home/me/code/app.ts' }),
      item.text(`Answer ${t}:\n\n\`\`\`ts\nconst x = ${t}\n\`\`\`\n\n| a | b |\n|---|---|\n| 1 | 2 |`),
      item.end(1500),
    )
    if (t % 10 === 0) items.push(item.command('compact', '', 'Compacted'), item.notice('compact', 'Conversation compacted'))
    if (t % 15 === 0) items.push(item.goal('tests pass'), item.text('working on the goal'), item.notice('task', 'Background job finished'), item.notice('interrupted', 'Interrupted'))
    if (t % 20 === 0) {
      items.push(
        item.tool(`todo${t}`, 'TodoWrite', { todos: [{ content: 'Write tests', status: 'completed', activeForm: 'Writing tests' }] }),
        item.tool(`ask${t}`, 'AskUserQuestion', { questions: [{ question: 'Which?', options: [{ label: 'A' }, { label: 'B' }] }] }),
        item.tool(`plan${t}`, 'ExitPlanMode', { plan: '1. Do it' }),
        item.goal('tests pass', { met: true, reason: 'green' }),
        item.user('ls', { shell: true }),
      )
    }
  }
  return items
}

describe('ChatView', () => {
  const view = () => <ChatView host="box" pane="%1" agent="claude" onTerminal={() => {}} />
  const load = (items: ChatItem[], info = chatInfo()) => act(() => emit({ t: 'chat.reset', key: 'box:%1', info, items, hasMore: true }))

  test('a message that failed because the connection dropped goes out again once it is back', async () => {
    useStore.setState({ connected: false, hosts: [host('box')] })
    const send = spyOn(api, 'send').mockResolvedValue({ ok: true } as any)
    render(view())
    load(longConversation(1))
    act(() => patchChat('box:%1', () => ({ pending: [
      { id: 'lost', text: 'run the tests', status: 'failed', at: Date.now(), error: 'Network error', retryable: true },
      { id: 'refused', text: 'nope', status: 'failed', at: Date.now(), error: 'pane is gone' },
    ] })))
    await flush()
    expect(send).not.toHaveBeenCalled()
    act(() => useStore.setState({ connected: true }))
    await flush()
    expect(send.mock.calls.map((c) => c[3])).toEqual(['lost'])
    expect(useStore.getState().chats['box:%1'].pending.find((p) => p.id === 'lost')?.status).toBe('sent')
    send.mockRestore()
  })

  test('a message whose resend fails again waits for a tap instead of going round on every reconnect', async () => {
    // a request the browser refuses outright looks like a dropped connection (a TypeError) too
    useStore.setState({ connected: false, hosts: [host('box')] })
    const send = spyOn(api, 'send').mockRejectedValue(new TypeError('Failed to fetch'))
    render(view())
    load(longConversation(1))
    act(() => patchChat('box:%1', () => ({ pending: [{ id: 'lost', text: 'run the tests', status: 'failed', at: Date.now(), error: 'Network error', retryable: true }] })))
    for (let i = 0; i < 3; i++) {
      act(() => useStore.setState({ connected: true }))
      await flush()
      act(() => useStore.setState({ connected: false }))
      await flush()
    }
    expect(send).toHaveBeenCalledTimes(1)
    const p = useStore.getState().chats['box:%1'].pending[0]!
    expect(p).toMatchObject({ status: 'failed', error: 'Failed to fetch', retryable: false })
    send.mockRestore()
  })

  test("Codex's reply shows as it is written, and the finished message takes its place once", async () => {
    useStore.setState({ connected: true, hosts: [host('box', { panes: [agentPane('%1', 'codex')] })] })
    const { container } = render(<ChatView host="box" pane="%1" agent="codex" onTerminal={() => {}} />)
    load([item.user('say hi again')], chatInfo({ agent: 'codex', turn: { id: 't1', startedAt: Date.now() - 5000 } }))
    act(() => emit({ t: 'chat.screen', key: 'box:%1', screen: { working: true, activity: 'Working', width: 120 } }))
    act(() => emit({ t: 'chat.live', key: 'box:%1', items: [{ id: 'rs_1', kind: 'thinking', text: 'Keeping it short' }, { id: 'msg_1', kind: 'text', text: 'Hi ag' }] }))
    await flush()
    expect(container.querySelector('[data-live]')).toBeTruthy()
    expect(screen.getByText('Hi ag')).toBeTruthy()
    expect(screen.getByText('Thinking')).toBeTruthy()
    // the working line counts the turn's time from the agent's own record
    expect(screen.getByText('Working')).toBeTruthy()
    expect(screen.getByText(/^\s*5s$/)).toBeTruthy()
    act(() => emit({ t: 'chat.live', key: 'box:%1', items: [{ id: 'rs_1', kind: 'thinking', text: 'Keeping it short' }, { id: 'msg_1', kind: 'text', text: 'Hi again!' }] }))
    await flush()
    expect(screen.getAllByText('Hi again!')).toHaveLength(1)
    // the record arrives with the same id: one copy on screen, from the record, no live bubble left for it
    const recorded = { ...item.text('Hi again!'), uuid: 'msg_1', msgId: 'msg_1' }
    act(() => emit({ t: 'chat.append', key: 'box:%1', items: [recorded, { ...item.thinking('Keeping it short'), uuid: 'rs_1' }, item.end(1239)] }))
    act(() => emit({ t: 'chat.screen', key: 'box:%1', screen: { working: false, width: 120 } }))
    await flush()
    expect(screen.getAllByText('Hi again!')).toHaveLength(1)
    expect(container.querySelector('[data-live]')).toBeNull()
    expect(screen.queryByText('Working')).toBeNull()
  })

  test("a question Codex asked mid-turn is told apart from a prompt while it waits", async () => {
    useStore.setState({ connected: true, hosts: [host('box', { panes: [agentPane('%1', 'codex')] })] })
    render(<ChatView host="box" pane="%1" agent="codex" onTerminal={() => {}} />)
    load([item.user('ask me'), item.tool('call_q', 'AskUserQuestion', { questions: [{ question: 'Colour', options: [{ label: 'Red' }, { label: 'Blue' }] }] })], chatInfo({ agent: 'codex' }))
    act(() => emit({ t: 'chat.screen', key: 'box:%1', screen: { working: true, activity: 'Working', questions: 1, width: 120 } }))
    await flush()
    expect(screen.getByText('Codex is asking')).toBeTruthy()
    expect(screen.getByText('Red · Blue')).toBeTruthy()
    act(() => emit({ t: 'chat.append', key: 'box:%1', items: [{ ...item.result('call_q', 'Colour: Red'), meta: { answers: { Colour: 'Red' } } } as ChatItem] }))
    await flush()
    expect(screen.getByText('Question answered')).toBeTruthy()
    expect(screen.getByText('Red')).toBeTruthy()
  })

  // the terminal draws only the highlighted option's preview, so the card shows every one
  test("a question's choices show what they mean and their previews", async () => {
    useStore.setState({ connected: true, hosts: [host('box', { panes: [agentPane('%1', 'claude')] })] })
    render(view())
    const options = [
      { label: 'Heading', description: 'The session name heads the group.', preview: '┌──────┐\n│ Legato │\n└──────┘' },
      { label: 'On each row', description: 'Each page says where it came from.' },
    ]
    load([item.user('ask me'), item.tool('call_p', 'AskUserQuestion', { questions: [{ question: 'Where should it show?', options }] })])
    await flush()
    expect(screen.getByText('The session name heads the group.')).toBeTruthy()
    expect(screen.getByText('Each page says where it came from.')).toBeTruthy()
    expect(screen.getByText(/│ Legato │/).tagName).toBe('PRE')
    expect(screen.queryByText('Heading · On each row')).toBeNull()
  })

  test('a turn that stopped on an error says why under the last message, until the next prompt', async () => {
    const stopped = (failure?: { error: string }) => host('box', { panes: [agentPane('%1', 'claude', { agent: { kind: 'claude', pid: 1, state: 'idle', failure } })] })
    useStore.setState({ connected: true, hosts: [stopped({ error: 'authentication_failed' })] })
    render(view())
    load([item.user('hi'), item.text('Hello')])
    await flush()
    expect(screen.getByRole('status').textContent).toBe('Stopped: signed out')
    act(() => useStore.setState({ hosts: [stopped()] }))
    expect(screen.queryByText('Stopped: signed out')).toBeNull()
  })

  test('renders a long conversation without throwing', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    load(longConversation(60))
    await flush()
    expect(screen.getByText(/^Question 0:/)).toBeTruthy()
    expect(screen.getByText(/^Answer 59:/)).toBeTruthy()
    expect(screen.getAllByText('Goal met').length).toBeGreaterThan(0)
    expect(screen.getByText('Earlier messages')).toBeTruthy()
    expect(screen.getByRole('textbox')).toBeTruthy()
  })

  test('scrolling up near the top loads earlier messages by itself', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    const sent = spyOn(socket, 'send').mockImplementation(() => {})
    const { container } = render(view())
    load(longConversation(10))
    await flush()
    const el = container.querySelector('.overflow-y-auto') as HTMLElement
    let top = 4000
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: 5000 })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: 800 })
    Object.defineProperty(el, 'scrollTop', { configurable: true, get: () => top, set: (v) => void (top = v) })
    const scrollTo = (y: number) => act(() => {
      top = y
      el.dispatchEvent(new Event('scroll'))
    })
    scrollTo(4200)
    // the reader wheels up: far from the top nothing is fetched, near it the earlier page is
    act(() => void el.dispatchEvent(new WheelEvent('wheel', { deltaY: -100 })))
    scrollTo(2500)
    expect(sent.mock.calls.some(([m]) => m.t === 'chat.more')).toBe(false)
    scrollTo(300)
    expect(sent.mock.calls.filter(([m]) => m.t === 'chat.more')).toHaveLength(1)
    // a skeleton of lines where the older messages land, not a spinner
    expect(screen.getByText('Loading earlier messages')).toBeTruthy()
    expect(container.querySelectorAll('.skeleton').length).toBeGreaterThan(0)
    expect(container.querySelector('.animate-spin')).toBeNull()
    // only one request at a time, and an empty answer still ends the wait
    scrollTo(100)
    expect(sent.mock.calls.filter(([m]) => m.t === 'chat.more')).toHaveLength(1)
    act(() => emit({ t: 'chat.more', key: 'box:%1', items: [], hasMore: true }))
    await flush()
    expect(screen.queryByText('Loading earlier messages')).toBeNull()
    expect(screen.getByText('Earlier messages')).toBeTruthy()
    sent.mockRestore()
  })

  test('keeps rendering as items stream in and the ledger moves', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    load(longConversation(5))
    const at = Date.now()
    act(() => patchChat('box:%1', () => ({ pending: [{ id: 's1', text: 'next step', status: 'sent', at }] })))
    expect(screen.getByText('next step')).toBeTruthy()
    const sends: SendState[] = [{ id: 's1', text: 'next step', at, state: 'delivered' }]
    act(() => {
      emit({ t: 'chat.info', key: 'box:%1', info: chatInfo({ sends, queue: ['queued thing'] }) })
      emit({ t: 'chat.append', key: 'box:%1', items: [item.user('next step'), item.text('streaming…')] })
      emit({ t: 'chat.screen', key: 'box:%1', screen: { working: true, width: 80, activity: 'Pondering… (3s · esc to interrupt)' } })
    })
    await flush()
    expect(screen.getAllByText('next step')).toHaveLength(1)
    expect(screen.getByText('queued thing')).toBeTruthy()
    expect(screen.getByText('Pondering…')).toBeTruthy()
  })

  test("another device's send shows until it arrives", async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    load([item.user('hi')], chatInfo({ sends: [{ id: 'other', text: 'from my laptop', at: Date.now(), state: 'sending' }] }))
    await flush()
    expect(screen.getByText('from my laptop')).toBeTruthy()
    expect(screen.getByText('Sending from another device')).toBeTruthy()
  })

  test('an empty conversation and an ended session', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    load([])
    expect(screen.getByText('New conversation')).toBeTruthy()
    load([], chatInfo({ running: false }))
    expect(screen.getByText('No agent is running in this pane')).toBeTruthy()
  })

  test('a message waiting to go still shows, with its buttons, when no agent is running', async () => {
    const sends = [{ id: 'd-1', host: 'box', pane: '%1', text: 'run the tests', at: Date.now() + 3600_000 }]
    act(() => emit({ t: 'overview', hosts: [host('box')], organize: { v: 2, sessions: [], projects: [], groups: [], spaces: [], sends } }))
    useStore.setState({ connected: true })
    render(view())
    load([], chatInfo({ running: false }))
    expect(screen.getByText('No agent is running in this pane')).toBeTruthy()
    expect(screen.getByText(/^Sends at /)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeTruthy()
    act(() => emit({ t: 'overview', hosts: [] }))
  })

  test('before the conversation arrives: lines in its shape, and the catch-up percentage on the label', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    const { container } = render(view())
    expect(screen.getByRole('status').textContent).toBe('Loading the conversation')
    expect(container.querySelectorAll('.skeleton')).toHaveLength(3)
    expect(container.querySelector('.animate-spin')).toBeNull()
    act(() => emit({ t: 'chat.progress', key: 'box:%1', done: 40, total: 100 }))
    expect(screen.getByRole('status').textContent).toBe('Loading the conversation · 40%')
    load([item.user('hello')])
    await flush()
    expect(screen.queryByText(/Loading the conversation/)).toBeNull()
    expect(screen.getByText('hello')).toBeTruthy()
  })

  test('catching up shows a quiet progress line, and the result lands in one go', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    load([item.user('before you left')])
    act(() => {
      emit({ t: 'chat.progress', key: 'box:%1', done: 40, total: 100 })
      emit({ t: 'chat.screen', key: 'box:%1', screen: { working: true, width: 80, activity: 'Pondering…' } })
    })
    expect(screen.getByRole('status').textContent).toContain('Catching up on what you missed')
    expect(screen.getByRole('status').textContent).toContain('40%')
    expect(screen.queryByText('Pondering…')).toBeNull()
    load([item.user('before you left'), item.text('done while you were away')])
    await flush()
    expect(screen.queryByText(/Catching up/)).toBeNull()
    expect(screen.getByText('done while you were away')).toBeTruthy()
  })

  test('rows added once the conversation is on screen rise in; the first batch, earlier messages and a confirmed bubble hold still', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    render(view())
    const first = [item.user('first question'), item.text('first reply'), item.end()]
    load(first)
    await flush()
    const rises = (text: string) => !!screen.getByText(text).closest('.animate-slide-up')
    expect(rises('first question')).toBe(false)
    expect(rises('first reply')).toBe(false)
    // typed at the keyboard: new to this screen
    act(() => emit({ t: 'chat.append', key: 'box:%1', items: [item.user('typed in tmux'), item.text('second reply')] }))
    expect(rises('typed in tmux')).toBe(true)
    expect(rises('second reply')).toBe(true)
    expect(rises('first question')).toBe(false)
    // older messages joining at the top were there before you looked
    act(() => emit({ t: 'chat.more', key: 'box:%1', items: [{ ...item.user('older'), seq: 0 }], hasMore: false }))
    expect(rises('older')).toBe(false)
    // sent from here: the bubble rises, and the message that takes its place doesn't move
    const at = Date.now()
    act(() => patchChat('box:%1', () => ({ pending: [{ id: 'p1', text: 'from here', status: 'sent', at }] })))
    expect(rises('from here')).toBe(true)
    act(() => {
      emit({ t: 'chat.info', key: 'box:%1', info: chatInfo({ sends: [{ id: 'p1', text: 'from here', at, state: 'delivered' }] }) })
      emit({ t: 'chat.append', key: 'box:%1', items: [item.user('from here')] })
    })
    expect(screen.getAllByText('from here')).toHaveLength(1)
    expect(rises('from here')).toBe(false)
  })

  test('Jump to latest is always there: hidden and inert at the bottom, shown once you read back', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    const { container } = render(view())
    load(longConversation(5))
    await flush()
    const jump = container.querySelector('[aria-label="Jump to latest"]') as HTMLElement
    expect(jump.hasAttribute('inert')).toBe(true)
    expect(jump.className).toContain('opacity-0')
    const el = container.querySelector('.overflow-y-auto') as HTMLElement
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: 5000 })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: 800 })
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 1000 })
    act(() => void el.dispatchEvent(new Event('scroll')))
    expect(jump.hasAttribute('inert')).toBe(false)
    expect(jump.className).toContain('opacity-100')
    expect(container.querySelector('[aria-label="Jump to latest"]')).toBe(jump)
  })

  test('coming back after a while jumps to the latest message', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    const { container } = render(view())
    load(longConversation(5))
    await flush()
    const scroller = container.querySelector('.overflow-y-auto') as HTMLElement
    const to = spyOn(scroller, 'scrollTo')
    // reading back stops following
    scroller.dispatchEvent(new WheelEvent('wheel', { deltaY: -100 }))
    const hidden = (v: boolean) => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => v })
      document.dispatchEvent(new Event('visibilitychange'))
    }
    const now = spyOn(Date, 'now')
    const t = Date.now()
    now.mockReturnValue(t)
    hidden(true)
    now.mockReturnValue(t + 60_000)
    hidden(false)
    await new Promise((r) => requestAnimationFrame(r))
    expect(to).not.toHaveBeenCalled()
    hidden(true)
    now.mockReturnValue(t + 60_000 + 3 * 3600_000)
    hidden(false)
    await new Promise((r) => requestAnimationFrame(r))
    expect(to).toHaveBeenCalled()
    now.mockRestore()
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
  })
})

describe('a long question card', () => {
  // happy-dom has no layout, so this checks the shape that keeps the answers on screen (the
  // browser check is a Playwright run on phone sizes): every box from the chat column down to
  // the card may shrink, the question scrolls on its own, and the answers sit outside it
  test('fits the chat column, with the question scrolling and the answers outside it', async () => {
    useStore.setState({ connected: true, hosts: [host('box')] })
    const body = ['Allow reading these files?', ...Array.from({ length: 22 }, (_, i) => `• file-${i}: read it to answer the question.`), '', 'Do you want to proceed?']
    render(<ChatView host="box" pane="%1" agent="claude" onTerminal={() => {}} />)
    act(() => {
      emit({ t: 'chat.reset', key: 'box:%1', info: chatInfo(), items: [], hasMore: false })
      emit({ t: 'chat.screen', key: 'box:%1', screen: { working: false, width: 80, prompt: { body, options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: 'No', selected: false }], multi: false } } })
    })
    await flush()
    const title = screen.getByText('Allow reading these files?')
    const card = title.closest('.animate-slide-up') as HTMLElement
    const scroller = title.closest('.overflow-y-auto') as HTMLElement
    expect(card.contains(scroller)).toBe(true)
    expect(scroller.textContent).toContain('file-21')

    const yes = screen.getByText('Yes').closest('button')!
    const cancel = screen.getByText('Cancel')
    expect(scroller.contains(yes)).toBe(false)
    expect(scroller.contains(cancel)).toBe(false)
    // Cancel never scrolls away; the choices scroll only when they alone don't fit
    expect(cancel.closest('.overflow-y-auto')).toBeNull()

    const column = card.closest('.flex-1.flex-col') as HTMLElement
    for (let el: HTMLElement | null = card; el && el !== column; el = el.parentElement) {
      expect(el.className).toContain('min-h-0')
      expect(el.className).toContain('flex-col')
    }
  })
})

describe('opened while the machine reconnects', () => {
  const view = () => <ChatView host="box" pane="%1" agent="claude" onTerminal={() => {}} />
  const asking = () =>
    host('box', { panes: [agentPane('%1', 'claude', { agent: { kind: 'claude', pid: 1, state: 'waiting', waitingFor: 'Bash command: npm test', options: [{ n: 1, label: 'Yes' }, { n: 2, label: 'No' }] } })] })
  // the app's own first picture after a cold start: the machine connecting, what it was doing unknown
  const connecting = () => host('box', { status: 'connecting', panes: [agentPane('%1', 'claude', { agent: { kind: 'claude', pid: 1, state: 'unknown' } })] })

  test('says it is reconnecting, not offline, and keeps the question it was asking, held until it is back', async () => {
    useStore.setState({ connected: true, hosts: [asking()] })
    useStore.setState({ hosts: [connecting()] })
    render(view())
    act(() => emit({ t: 'chat.reset', key: 'box:%1', info: chatInfo(), items: [item.user('run the tests')], hasMore: false }))
    await flush()
    expect(screen.getByText('Reconnecting to box…')).toBeTruthy()
    expect(screen.getByText('Reconnecting to box…').className).toContain('animate-fade-in')
    expect(screen.queryByText(/is offline/)).toBeNull()
    expect(screen.getByText('Bash command: npm test')).toBeTruthy()
    const yes = () => screen.getByText('Yes').closest('button')!
    expect(yes().disabled).toBe(true)
    expect(screen.getByText('Cancel').closest('button')!.disabled).toBe(true)

    // back: the session's own screen takes over, and its answers work
    act(() => {
      useStore.setState({ hosts: [asking()] })
      emit({ t: 'chat.screen', key: 'box:%1', screen: { working: false, width: 80, prompt: { body: ['Bash command', '╌', 'npm test', '╌', 'Do you want to proceed?'], options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: 'No', selected: false }], multi: false } } })
    })
    await flush()
    expect(screen.queryByText(/Reconnecting/)).toBeNull()
    expect(yes().disabled).toBe(false)
  })

  test('a machine still away after a while is called offline', async () => {
    const was = reconnect.graceMs
    reconnect.graceMs = 30
    try {
      useStore.setState({ connected: true, hosts: [connecting()] })
      render(view())
      act(() => emit({ t: 'chat.reset', key: 'box:%1', info: chatInfo(), items: [item.user('hello')], hasMore: false }))
      expect(screen.getByText('Reconnecting to box…')).toBeTruthy()
      await act(() => new Promise((r) => setTimeout(r, 60)))
      expect(screen.getByText(/box is offline/)).toBeTruthy()
    } finally {
      reconnect.graceMs = was
    }
  })

  test('once the machine is back, a question its screen no longer shows is gone', async () => {
    useStore.setState({ connected: true, hosts: [asking()] })
    useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude')] })] })
    render(view())
    act(() => emit({ t: 'chat.reset', key: 'box:%1', info: chatInfo(), items: [], hasMore: false }))
    await flush()
    expect(screen.queryByText('Bash command: npm test')).toBeNull()
    // the composer fades back into the card's place
    expect(screen.getByRole('textbox', { name: /Message/ }).closest('.animate-fade-in')).not.toBeNull()
  })
})

describe('a terminal-first agent', () => {
  const view = () => <ChatView host="box" pane="%1" agent="pi" onTerminal={() => {}} />

  test('points to the terminal at once, without waiting on a conversation the hub never loads', async () => {
    useStore.setState({ connected: true, hosts: [host('box', { panes: [agentPane('%1', 'pi')] })] })
    render(view())
    expect(screen.getByText('Pi runs in the terminal')).toBeTruthy()
    expect(screen.queryByText(/Loading the conversation/)).toBeNull()
    // the hub's first word on a pane it has read nothing from says it isn't running; the pane knows better
    act(() => emit({ t: 'chat.info', key: 'box:%1', info: chatInfo({ running: false }) }))
    expect(screen.getByText('Pi runs in the terminal')).toBeTruthy()
  })

  test('once it has quit, the pane offers to start an agent again', async () => {
    useStore.setState({ connected: true, hosts: [host('box', { panes: [agentPane('%1', 'pi', { agent: undefined })] })] })
    render(view())
    expect(screen.getByText('No agent is running in this pane')).toBeTruthy()
  })
})
