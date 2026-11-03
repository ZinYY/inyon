import { beforeEach, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { fireEvent } from '@testing-library/react'
import { ActivityFeed, LimitsCard, MachinesTable, SessionCard, activityLabel } from './Overview.tsx'
import { ContextMenuHost } from './ui/ContextMenu.tsx'
import { ActionSheets } from './menus.tsx'
import { useStore } from '../lib/store.ts'
import { allPanes } from '../lib/overview.ts'
import { api } from '../test/fakes.ts'
import { agentPane, host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const asking = () => host('box', { panes: [agentPane('%1', 'codex', { title: 'Migrate auth', agent: { kind: 'codex', pid: 1, state: 'waiting', waitingFor: 'Run the tests?', options: [{ n: 1, label: 'Yes' }, { n: 2, label: 'No' }] } })] })

beforeEach(() => useStore.setState({ hosts: [asking()], hostsLoaded: true, toasts: [] }))

// "needs you" stayed on a question long after it was answered at the keyboard
test('recent activity says needs you only while the session is still on that question', () => {
  const ev = (id: number, pane = '%1', kind: 'waiting' | 'done' = 'waiting') => ({ id, at: id, host: 'box', pane, kind, title: 'Migrate auth', body: '' })
  const [newer, older] = [ev(2), ev(1)]
  const feed = [newer, older]
  const waiting = [asking()]
  expect(activityLabel(newer, feed, waiting)).toBe('needs you')
  expect(activityLabel(older, feed, waiting)).toBe('answered')
  const moved = [host('box', { panes: [agentPane('%1', 'codex', { agent: { kind: 'codex', pid: 1, state: 'working' } })] })]
  expect(activityLabel(newer, feed, moved)).toBe('answered')
  expect(activityLabel(ev(3, '%7'), feed, waiting)).toBe('asked')
  expect(activityLabel(ev(4, '%1', 'done'), feed, waiting)).toBe('finished')
})

test('an answer reads Sending…, then Sent', async () => {
  let release!: () => void
  api.answer.mockImplementationOnce(() => new Promise<any>((r) => (release = () => r({}))))
  const r = allPanes(useStore.getState().hosts)[0]!
  await render(
    <MemoryRouter>
      <SessionCard r={r} />
    </MemoryRouter>,
  )
  await userEvent.click(screen.getByRole('button', { name: 'Yes' }))
  expect(screen.getByRole('button', { name: 'Sending…' })).toBeTruthy()
  expect(api.answer).toHaveBeenCalledWith('box', '%1', 1)
  release()
  await flush()
  expect(screen.getByText('Sent')).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'No' })).toBeNull()
})

// rows of Recent activity slid down under the pointer as new events came in
test('recent activity holds still while the pointer is on it, and offers what came in meanwhile', async () => {
  const ev = (id: number, title: string) => ({ id, at: id, host: 'box', pane: '%1', kind: 'done' as const, title, body: '' })
  useStore.setState({ activity: [ev(2, 'Second'), ev(1, 'First')] })
  await render(
    <MemoryRouter>
      <ActivityFeed />
    </MemoryRouter>,
  )
  const rows = () => [...document.querySelectorAll('section a .font-medium')].map((e) => e.textContent)
  const feed = screen.getByText('Recent activity').closest('section')!
  fireEvent.pointerEnter(feed)
  await act(() => useStore.setState({ activity: [ev(3, 'Third'), ev(2, 'Second'), ev(1, 'First')] }))
  expect(rows()).toEqual(['Second', 'First'])
  await userEvent.click(screen.getByRole('button', { name: '1 new' }))
  expect(rows()).toEqual(['Third', 'Second', 'First'])
  fireEvent.pointerLeave(feed)
  await act(() => useStore.setState({ activity: [ev(4, 'Fourth'), ev(3, 'Third'), ev(2, 'Second'), ev(1, 'First')] }))
  expect(rows()[0]).toBe('Fourth')
})

test('a session card and a machine offer the same actions on a right-click as in their menus', async () => {
  const working = host('box', { panes: [agentPane('%1', 'claude', { title: 'Fix login', agent: { kind: 'claude', pid: 1, state: 'idle', sessionId: '0b7c2f8e-1d2a-4c1e-9a55-3f2e6d1c9b10' } })] })
  useStore.setState({ hosts: [working] })
  const r = allPanes(useStore.getState().hosts)[0]!
  await render(
    <MemoryRouter>
      <SessionCard r={r} />
      <MachinesTable hosts={[working]} />
      <ActionSheets />
      <ContextMenuHost />
    </MemoryRouter>,
  )
  const items = () => screen.getAllByRole('menuitem').map((b) => b.textContent)
  fireEvent.contextMenu(screen.getByText('Fix login'), { clientX: 10, clientY: 10 })
  expect(items()).toEqual(['Open', 'Open terminal', 'Open in new tab', 'Rename…', 'Rename tmux session…', 'Past conversations…', 'Copy session ID', 'End session…'])
  // the same list as the card's ⋯ button
  fireEvent.keyDown(window, { key: 'Escape' })
  await userEvent.click(screen.getByRole('button', { name: 'Actions for Fix login' }))
  expect(items()).toEqual(['Open', 'Open terminal', 'Open in new tab', 'Rename…', 'Rename tmux session…', 'Past conversations…', 'Copy session ID', 'End session…'])
  await userEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }))
  expect(screen.getByRole('dialog', { name: 'Rename session' })).toBeTruthy()

  fireEvent.contextMenu(screen.getAllByText('box')[0]!, { clientX: 10, clientY: 10 })
  expect(items()).toEqual(['Open', 'New session', 'Open in new tab', 'Temp files', 'Edit…', 'Remove…'])
})

test('plan limits show with no session running, and a newer session reading wins', async () => {
  const later = Math.round(Date.now() / 1000) + 3600
  const idle = host('laptop', {
    agents: ['codex'],
    limits: [{ kind: 'codex', fiveHour: { pct: 41, resetsAt: later }, sevenDay: { pct: 7, resetsAt: later }, plan: 'plus', resets: 2, updatedAt: 1000 }],
  })
  const { unmount } = await render(<LimitsCard hosts={[idle]} />)
  expect(screen.getByText('Plan limits')).toBeTruthy()
  expect(screen.getByText('41%')).toBeTruthy()
  expect(screen.getByText('7%')).toBeTruthy()
  expect(screen.getByText('2 resets banked')).toBeTruthy()
  unmount()
  const busy = host('laptop', {
    agents: ['codex'],
    limits: idle.limits,
    panes: [agentPane('%1', 'codex', { agent: { kind: 'codex', pid: 1, state: 'working', status: { fiveHour: { pct: 55, resetsAt: later }, updatedAt: 2000 } } })],
  })
  await render(<LimitsCard hosts={[busy]} />)
  expect(screen.getByText('55%')).toBeTruthy()
  expect(screen.queryByText('41%')).toBeNull()
})

test("a custom agent's card names it, with the logo of the agent it works like", async () => {
  const yolo = host('box', { panes: [agentPane('%1', 'claude', { title: 'Fix login', agent: { kind: 'claude', custom: 'Claude YOLO', pid: 1, state: 'idle' } })] })
  await render(
    <MemoryRouter>
      <SessionCard r={allPanes([yolo])[0]!} />
    </MemoryRouter>,
  )
  expect(screen.getByText('Claude YOLO')).toBeTruthy()
  expect(screen.queryByText('Claude Code', { selector: 'span.truncate' })).toBeNull()
  expect(screen.getByRole('img', { name: 'Claude Code' })).toBeTruthy()
})
