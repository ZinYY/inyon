import { expect, test } from 'bun:test'
import { act, fireEvent, renderHook, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import type { AgentKind } from '@inyon/protocol'
import { clearConversation, isClearCommand, refreshClearedConversation } from './clearConversation.ts'
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
  return { pane, key, info }
}

test('only bare clear/new commands start a conversation reset', () => {
  for (const command of ['/clear', ' /new\n']) expect(isClearCommand('codex', command)).toBe(true)
  expect(isClearCommand('claude', '/clear')).toBe(true)
  for (const command of ['/clear extra', 'Please explain /clear', '/goal clear', '/clear\nhello']) expect(isClearCommand('codex', command)).toBe(false)
  expect(isClearCommand('claude', '/new')).toBe(false)
})

test('lock is immediate, duplicate clear is ignored, and send completion alone does not unlock', async () => {
  const { pane, key, info } = conversation()
  let sent!: () => void
  api.send.mockImplementation(() => new Promise<void>((resolve) => { sent = resolve }))
  const operation = clearConversation('box', pane, '/clear')
  expect(useStore.getState().chats[key].clearing?.phase).toBe('clearing')
  await clearConversation('box', pane, '/clear')
  expect(api.send).toHaveBeenCalledTimes(1)
  sent()
  await flush()
  expect(useStore.getState().chats[key].clearing?.phase).toBe('refreshing')
  expect(socket.refreshChat.mock.calls.filter(([h, p]) => h === 'box' && p === pane)).toHaveLength(1)
  emit({ t: 'chat.reset', key, info, items: [], hasMore: false })
  await flush()
  expect(useStore.getState().chats[key].clearing).toBeTruthy()
  emit({ t: 'chat.reset', key, info: { ...info, sessionId: 'after' }, items: [], hasMore: false })
  await operation
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
  expect(useStore.getState().chats[key].info?.sessionId).toBe('after')
  expect(useStore.getState().chats[key].items).toHaveLength(0)
})

test('a lost response keeps the lock; recovery refreshes without resending clear', async () => {
  const { pane, key, info } = conversation('claude')
  api.send.mockRejectedValue(new Error('Connection lost'))
  await clearConversation('box', pane, '/clear')
  expect(useStore.getState().chats[key].clearing).toMatchObject({ phase: 'failed', error: 'Connection lost' })
  const recovery = refreshClearedConversation('box', pane)
  await flush()
  expect(api.refresh).toHaveBeenCalledTimes(1)
  expect(api.send).toHaveBeenCalledTimes(1)
  emit({ t: 'chat.reset', key, info: { ...info, sessionId: 'after' }, items: [], hasMore: false })
  await recovery
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
})

test('the menu uses the same lock and settings cannot enter while clear is running', async () => {
  const { pane, key, info } = conversation('claude')
  const { result } = renderHook(() => useSession('box', pane))
  let operation!: Promise<unknown>
  act(() => { operation = result.current.actions.clear() as Promise<unknown> })
  await flush()
  await result.current.actions.setModel('sonnet')
  await result.current.actions.interrupt()
  expect(api.setting).not.toHaveBeenCalled()
  expect(api.interrupt).not.toHaveBeenCalled()
  act(() => emit({ t: 'chat.reset', key, info: { ...info, sessionId: 'after' }, items: [], hasMore: false }))
  await act(() => operation)
  expect(useStore.getState().chats[key].clearing).toBeUndefined()
})

for (const agent of ['claude', 'codex'] as const) {
  test(`${agent}: typing /clear disables input and the whole chat until its refreshed snapshot`, async () => {
    const { pane, key, info } = conversation(agent)
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
    expect(screen.getByText('Clearing conversation…')).toBeTruthy()
    // A second event before/after React paints must not dispatch another request.
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(api.send).toHaveBeenCalledTimes(1)
    await act(async () => { sent(); await flush() })
    expect(screen.getByText('Refreshing conversation…')).toBeTruthy()
    expect(input.disabled).toBe(true)
    act(() => {
      emit({ t: 'overview', hosts: [host('box', { panes: [agentPane(pane, agent, { agent: { kind: agent, pid: 1, state: 'idle', sessionId: 'after' } })] })] })
      emit({ t: 'chat.reset', key, info: { ...info, sessionId: 'after' }, items: [], hasMore: false })
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
