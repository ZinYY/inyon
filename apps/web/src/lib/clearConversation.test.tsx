import { expect, test } from 'bun:test'
import { act, fireEvent, renderHook, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import type { AgentKind } from '@inyon/protocol'
import { clearConversation, changeConversation, sendTransition, isClearCommand, transitionCommand, refreshClearedConversation, waitForConnection, recoverConnection } from './clearConversation.ts'
import { patchChat, useStore } from './store.ts'
import { api, emit, socket } from '../test/fakes.ts'
import { agentPane, chatInfo, host, item } from '../test/fixtures.ts'
import { flush, renderSettled } from '../test/render.tsx'
import { PanePage } from '../pages/PanePage.tsx'
import { useSession } from '../components/session/useSession.ts'

let next = 700
function conversation(agent: AgentKind = 'codex') {
  api.commands.mockResolvedValue([])
  const pane = `%${++next}`
  const key = `box:${pane}`
  const info = chatInfo({ paneId: pane, agent, sessionId: 'before' })
  useStore.setState({ connected: true, hostsLoaded: true, hosts: [host('box', { panes: [agentPane(pane, agent, { agent: { kind: agent, pid: 1, state: 'idle', sessionId: 'before' } })] })] })
  emit({ t: 'chat.reset', key, info, items: [item.user('Previous conversation')], hasMore: false })
  emit({ t: 'chat.screen', key, screen: { working: false, width: 100, inputVisible: true } })
  let resolve!: (value: any) => void
  api.refreshConversation.mockImplementation(() => new Promise((r) => { resolve = r }))
  const refreshed = (sessionId = 'after', items = [] as any[], draft = '') => resolve({ reset: { t: 'chat.reset', key, info: { ...info, sessionId }, items, hasMore: false }, screen: { working: false, width: 100, inputVisible: true }, draft: { text: draft, source: 'terminal', at: Date.now() } })
  return { pane, key, info, refreshed }
}

test('only bare clear/new commands start a conversation reset', () => {
  for (const command of ['/clear', ' /new\n']) expect(isClearCommand('codex', command)).toBe(true)
  expect(isClearCommand('claude', '/clear')).toBe(true)
  for (const command of ['/clear extra', 'Please explain /clear', '/goal clear', '/clear\nhello']) expect(isClearCommand('codex', command)).toBe(false)
  expect(isClearCommand('claude', '/new')).toBe(false)
})

test('lock is immediate, duplicate clear is ignored, and send completion alone does not unlock', async () => {
  const { pane, key, info, refreshed } = conversation()
  let sent!: () => void
  api.send.mockImplementation(() => new Promise<void>((resolve) => { sent = resolve }))
  const operation = clearConversation('box', pane, '/clear')
  expect(useStore.getState().chats[key].clearing?.phase).toBe('clearing')
  await clearConversation('box', pane, '/clear')
  expect(api.send).toHaveBeenCalledTimes(1)
  sent()
  await flush()
  expect(useStore.getState().chats[key].clearing?.phase).toBe('refreshing')
  expect(api.refreshConversation).toHaveBeenCalledWith('box', pane, { agent: 'codex', differentFrom: 'before' })
  emit({ t: 'chat.reset', key, info, items: [], hasMore: false })
  await flush()
  expect(useStore.getState().chats[key].clearing).toBeTruthy()
  refreshed()
  await operation
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
  expect(useStore.getState().chats[key].info?.sessionId).toBe('after')
  expect(useStore.getState().chats[key].items).toHaveLength(0)
})

test('a lost response keeps the lock; recovery refreshes without resending clear', async () => {
  const { pane, key, info, refreshed } = conversation('claude')
  api.send.mockRejectedValue(new Error('Connection lost'))
  await clearConversation('box', pane, '/clear')
  expect(useStore.getState().chats[key].clearing).toMatchObject({ phase: 'failed', error: 'Connection lost' })
  const recovery = refreshClearedConversation('box', pane)
  await flush()
  expect(api.refreshConversation).toHaveBeenCalledTimes(1)
  expect(api.send).toHaveBeenCalledTimes(1)
  refreshed()
  await recovery
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
})

test('reconnect preserves cached history, loaded older pages and unsent draft until a complete read', async () => {
  const { pane, key, info } = conversation('claude')
  const older = item.user('Older readable message')
  const head = item.user('Recent readable message')
  patchChat(key, () => ({ items: [older, head], hasMore: false, draft: { text: 'My unsent question', source: 'app', at: 1 } }))
  useStore.setState({ connected: false })
  waitForConnection('box', pane)
  emit({ t: 'chat.reset', key, info, items: [], hasMore: false })
  expect(useStore.getState().chats[key].clearing?.retained?.items).toEqual([older, head])
  let ready!: (v: any) => void
  api.refreshConversation.mockImplementation(() => new Promise((r) => { ready = r }))
  useStore.setState({ connected: true })
  const recovery = recoverConnection('box', pane)
  expect(api.refreshConversation.mock.calls[0][2]).toEqual({ allowWorking: true, allowPrompt: true, readOnly: true })
  ready({ reset: { t: 'chat.reset', key, info, items: [head, item.text('New reply')], hasMore: true }, screen: { working: false, width: 80 } })
  await recovery
  const result = useStore.getState().chats[key]
  expect(result.clearing).toBeUndefined()
  expect(result.items[0]).toEqual(older)
  expect(result.hasMore).toBe(false)
  expect(result.draft?.text).toBe('My unsent question')
  expect(api.send).not.toHaveBeenCalled()
})

test('another disconnect invalidates the in-flight read and serializes recovery without replaying actions', async () => {
  const { pane, key, info } = conversation()
  const reads: ((v: any) => void)[] = []
  api.refreshConversation.mockImplementation(() => new Promise((r) => { reads.push(r) }))
  useStore.setState({ connected: false }); waitForConnection('box', pane)
  useStore.setState({ connected: true }); const first = recoverConnection('box', pane)
  useStore.setState({ connected: false }); waitForConnection('box', pane)
  useStore.setState({ connected: true }); recoverConnection('box', pane)
  expect(reads).toHaveLength(1)
  reads[0]({ reset: { t: 'chat.reset', key, info, items: [item.user('Stale response')], hasMore: false }, screen: { working: false, width: 80 } })
  await first; await flush()
  expect(reads).toHaveLength(2)
  expect(useStore.getState().chats[key].clearing?.retained?.items[0]).toMatchObject({ text: 'Previous conversation' })
  reads[1]({ reset: { t: 'chat.reset', key, info, items: [item.user('Fresh response')], hasMore: false }, screen: { working: false, width: 80 } })
  await flush()
  expect(useStore.getState().chats[key].items[0]).toMatchObject({ text: 'Fresh response' })
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
  expect(api.send).not.toHaveBeenCalled()
})

test('reconnect never automatically retries an old pending message into a different conversation', async () => {
  const { pane, key, info } = conversation()
  patchChat(key, () => ({ pending: [{ id: 'unsure', text: 'Old question', at: Date.now(), status: 'failed', retryable: true }] }))
  useStore.setState({ connected: false }); waitForConnection('box', pane)
  api.refreshConversation.mockResolvedValue({ reset: { t: 'chat.reset', key, info: { ...info, sessionId: 'different' }, items: [], hasMore: false }, screen: { working: false, width: 80 } })
  useStore.setState({ connected: true }); await recoverConnection('box', pane)
  expect(useStore.getState().chats[key].pending[0]).toMatchObject({ status: 'failed', retryable: false })
  expect(api.send).not.toHaveBeenCalled()
})

test('refresh restores the latest reading anchor after DOM replacement, including scrolling during the wait', async () => {
  const { pane, key, info, refreshed } = conversation()
  const items = Array.from({ length: 12 }, (_, i) => item.user(`Readable message ${i}`))
  patchChat(key, () => ({ items }))
  await renderSettled(<MemoryRouter initialEntries={[`/m/box/p/${pane.slice(1)}`]}><Routes><Route path="/m/:host/p/:pane" element={<PanePage />} /></Routes></MemoryRouter>)
  const scroller = (await screen.findByText('Readable message 0')).closest('.overflow-y-auto') as HTMLElement
  const original = HTMLElement.prototype.getBoundingClientRect
  const anchors = () => [...scroller.querySelectorAll<HTMLElement>('[data-chat-anchor]')]
  Object.defineProperties(scroller, { scrollHeight: { get: () => anchors().length * 100 }, clientHeight: { value: 200 } })
  HTMLElement.prototype.getBoundingClientRect = function () {
    const index = anchors().indexOf(this)
    const top = index < 0 ? 0 : index * 100 - scroller.scrollTop
    return { top, bottom: top + 100, height: 100, width: 200, left: 0, right: 200, x: 0, y: top, toJSON() {} }
  }
  try {
    fireEvent.wheel(scroller, { deltaY: -100 }); scroller.scrollTop = 500; fireEvent.scroll(scroller)
    let operation!: Promise<unknown>
    act(() => { operation = changeConversation('box', pane, 'Reloading conversation', async () => {}) })
    await flush()
    act(() => emit({ t: 'chat.reset', key, info, items: [], hasMore: false }))
    expect(screen.getByText('Readable message 5')).toBeTruthy()
    scroller.scrollTop = 200; fireEvent.scroll(scroller)
    const id = anchors()[2].dataset.chatAnchor
    act(() => refreshed('before', [item.user('New earlier one'), item.user('New earlier two'), ...items.map((i) => ({ ...i, seq: i.seq + 100 }))]))
    await act(() => operation)
    expect(scroller.scrollTop).toBe(400)
    expect(anchors().find((a) => a.dataset.chatAnchor === id)!.getBoundingClientRect().top).toBe(0)
    expect(screen.getByText('Readable message 2').closest('[inert]')).toBeNull()
  } finally { HTMLElement.prototype.getBoundingClientRect = original }
})

test('the menu uses the same lock and settings cannot enter while clear is running', async () => {
  const { pane, key, info, refreshed } = conversation('claude')
  const { result } = renderHook(() => useSession('box', pane))
  let operation!: Promise<unknown>
  act(() => { operation = result.current.actions.clear() as Promise<unknown> })
  await flush()
  await result.current.actions.setModel('sonnet')
  await result.current.actions.interrupt()
  expect(api.setting).not.toHaveBeenCalled()
  expect(api.interrupt).not.toHaveBeenCalled()
  act(() => refreshed())
  await act(() => operation)
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
})

for (const agent of ['claude', 'codex'] as const) {
  test(`${agent}: clear locks only controls and preserves readable history until its complete snapshot`, async () => {
    const { pane, key, info, refreshed } = conversation(agent)
    let sent!: () => void
    api.send.mockImplementation(() => new Promise<void>((resolve) => { sent = resolve }))
    await renderSettled(<MemoryRouter initialEntries={[`/m/box/p/${pane.slice(1)}`]}><Routes>
      <Route path="/m/:host/p/:pane" element={<PanePage />} />
    </Routes></MemoryRouter>)
    const input = await screen.findByRole('textbox', { name: `Message ${agent === 'claude' ? 'Claude Code' : 'Codex'}` }) as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: '/clear' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await flush()
    expect(input.disabled).toBe(true)
    expect(input.closest('[inert]')).not.toBeNull()
    expect(screen.getByText('Previous conversation').closest('[inert]')).toBeNull()
    expect(screen.getByText('Clearing conversation…')).toBeTruthy()
    // A second event before/after React paints must not dispatch another request.
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(api.send).toHaveBeenCalledTimes(1)
    await act(async () => { sent(); await flush() })
    expect(screen.getByText('Refreshing conversation…')).toBeTruthy()
    expect(input.disabled).toBe(true)
    act(() => emit({ t: 'chat.reset', key, info: { ...info, sessionId: 'after' }, items: [item.user('Incomplete replacement')], hasMore: false }))
    expect(screen.getByText('Previous conversation')).toBeTruthy()
    expect(screen.queryByText('Incomplete replacement')).toBeNull()
    act(() => {
      emit({ t: 'overview', hosts: [host('box', { panes: [agentPane(pane, agent, { agent: { kind: agent, pid: 1, state: 'idle', sessionId: 'after' } })] })] })
      refreshed()
    })
    await flush()
    expect(input.disabled).toBe(false)
    expect(input.closest('[inert]')).toBeNull()
    expect(screen.queryByText('Previous conversation')).toBeNull()
    expect(screen.queryByText('Refreshing conversation…')).toBeNull()
    fireEvent.change(input, { target: { value: 'A new message' } })
    api.send.mockResolvedValue({})
    fireEvent.keyDown(input, { key: 'Enter' })
    await flush()
    expect(api.send).toHaveBeenCalledTimes(2)
    expect(api.send.mock.calls[1][2]).toBe('A new message')
    act(() => patchChat(key, () => ({ pending: [] })))
  })
}


test('rewind replaces history and preserves restored input', async () => {
  const { pane, key, refreshed } = conversation()
  const operation = changeConversation('box', pane, 'Rewinding conversation', () => api.rewind('box', pane), { agent: 'codex' })
  await flush()
  const retained = item.user('Retained message')
  refreshed('before', [retained], 'Restored question')
  expect(await operation).toBe(true)
  expect(useStore.getState().chats[key].items).toEqual([retained])
  expect(useStore.getState().chats[key].draft?.text).toBe('Restored question')
})

test('fork waits for destination before navigating and preserves source', async () => {
  const { pane, key, refreshed } = conversation()
  let opened: string | undefined
  api.fork.mockResolvedValue({ pane: '%999' })
  const operation = changeConversation('box', pane, 'Forking conversation', () => api.fork('box', pane), { differentFrom: 'before' }, { onReady: (p) => { opened = p } })
  await flush()
  expect(opened).toBeUndefined()
  expect(api.refreshConversation).toHaveBeenCalledWith('box', '%999', { differentFrom: 'before' })
  refreshed('forked', [item.user('Fork history')])
  await operation
  expect(opened).toBe('%999')
  expect(useStore.getState().chats[key].items[0]).toMatchObject({ text: 'Previous conversation' })
  expect(useStore.getState().chats['box:%999'].info?.sessionId).toBe('forked')
})

test('Claude rewind leaves its next confirmation interactive', async () => {
  const { pane, key, info } = conversation('claude')
  api.refreshConversation.mockResolvedValue({ reset: { t: 'chat.reset', key, info, items: [], hasMore: false }, screen: { width: 100, working: false, prompt: { body: ['Rewind'], options: [], multi: false } } })
  await changeConversation('box', pane, 'Rewinding conversation', async () => {}, { allowPrompt: true }, { rewind: true })
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
  expect(useStore.getState().chats[key].rewindFlow).toBe(true)
})

test('only native state-changing commands are intercepted', () => {
  for (const cmd of ['/rewind', '/compact', '/resume thread', '/fork', '/exit', '/btw question']) expect(transitionCommand('codex', cmd)).toBeTruthy()
  for (const cmd of ['explain /fork', '/goal clear', '/rewind extra', '/compact\nordinary message']) expect(transitionCommand('codex', cmd)).toBeUndefined()
})


test('opening BTW waits for a different thread and accepts multiline questions', async () => {
  const { pane, key, refreshed } = conversation()
  const text = '/btw First line\nSecond line'
  expect(transitionCommand('codex', text)).toBe('Opening side conversation')
  const operation = sendTransition('box', pane, 'codex', text)
  await flush()
  expect(api.refreshConversation).toHaveBeenCalledWith('box', pane, expect.objectContaining({ differentFrom: 'before', allowWorking: true }))
  expect(useStore.getState().chats[key].clearing).toBeTruthy()
  refreshed('side')
  await operation
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
})
