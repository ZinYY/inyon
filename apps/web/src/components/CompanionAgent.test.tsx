import { beforeEach, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { CompanionBar, CompanionNavItem, CompanionRows, OLD_HUB, companionPanes } from './CompanionAgent.tsx'
import { api } from '../test/fakes.ts'
import { agentPane, host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'
import { ApiError } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'
import { paneTitle } from '../lib/overview.ts'

const withCompanion = (state: 'idle' | 'waiting' | 'working' = 'idle', waitingFor?: string) =>
  host('local', { config: { id: 'local', name: 'abhash-arm', kind: 'local' }, panes: [agentPane('%1', 'claude'), agentPane('%9', 'claude', { companion: true, agent: { kind: 'claude', pid: 2, state, waitingFor } })] })

function Where() {
  return <span data-testid="at">{useLocation().pathname}</span>
}

const inRouter = (ui: React.ReactNode) => (
  <MemoryRouter initialEntries={['/settings']}>
    <Routes>
      <Route path="*" element={<>{ui}<Where /></>} />
    </Routes>
  </MemoryRouter>
)

beforeEach(() => useStore.setState({ hosts: [], toasts: [] }))

test('the companion is the pane the hub marks, and goes by that name', () => {
  const h = withCompanion()
  const [r] = companionPanes([h])
  expect(r.pane.id).toBe('%9')
  expect(paneTitle(r)).toBe('Companion')
  expect(companionPanes([host('local', { panes: [agentPane('%1', 'claude')] })])).toEqual([])
})

test('off: pick an agent and start it', async () => {
  api.companion.mockImplementation(async () => ({ agent: null, host: 'local', pane: null }))
  api.startCompanion.mockImplementation(async (agent) => ({ agent, host: 'local', pane: '%9' }))
  await render(inRouter(<CompanionRows />))
  await userEvent.click(screen.getByRole('radio', { name: 'Codex' }))
  await userEvent.click(screen.getByRole('button', { name: 'Start' }))
  await flush()
  expect(api.startCompanion).toHaveBeenCalledWith('codex')
  expect(useStore.getState().toasts.at(-1)).toMatchObject({ title: 'Companion started', href: '/m/local/p/9' })
})

test('running: shows its agent and state, opens its chat, and stops', async () => {
  useStore.setState({ hosts: [withCompanion('working')] })
  api.companion.mockImplementation(async () => ({ agent: 'claude', host: 'local', pane: '%9' }))
  await render(inRouter(<CompanionRows />))
  expect(screen.getByText('Claude Code')).toBeTruthy()
  expect(screen.getByText('On abhash-arm, in the tmux session companion')).toBeTruthy()
  expect(screen.getByText('Working')).toBeTruthy()
  await userEvent.click(screen.getByRole('button', { name: 'Open' }))
  expect(screen.getByTestId('at').textContent).toBe('/m/local/p/9')
  await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
  await flush()
  expect(api.stopCompanion).toHaveBeenCalled()
})

test('on, with its agent gone: start it again or turn it off', async () => {
  api.companion.mockImplementation(async () => ({ agent: 'codex', host: 'local', pane: null }))
  await render(inRouter(<CompanionRows />))
  expect(screen.getByText(/Codex isn.t running/)).toBeTruthy()
  await userEvent.click(screen.getByRole('button', { name: 'Start again' }))
  expect(api.startCompanion).toHaveBeenCalledWith('codex')
})

test('a Bun hub says it has no companion', async () => {
  api.companion.mockImplementation(async () => {
    throw new ApiError(404, 'Not found')
  })
  await render(inRouter(<CompanionRows />))
  expect(screen.getByText(OLD_HUB)).toBeTruthy()
})

test('a failed status read says so and tries again, instead of loading for ever', async () => {
  api.companion.mockImplementation(async () => {
    throw new ApiError(500, 'The hub is restarting')
  })
  await render(inRouter(<CompanionRows />))
  expect(screen.getByText('The hub is restarting')).toBeTruthy()
  api.companion.mockImplementation(async () => ({ agent: null, host: 'local', pane: null }))
  await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
  await flush()
  expect(screen.getByRole('button', { name: 'Start' })).toBeTruthy()
})

test('companions from two hubs each get a line and a sidebar entry, named by machine', async () => {
  const other = host('lap', { config: { id: 'lap', name: 'laptop', kind: 'local' }, panes: [agentPane('%4', 'codex', { companion: true })] })
  useStore.setState({ hosts: [withCompanion(), other] })
  await render(inRouter(<><CompanionBar /><CompanionNavItem className={() => ''} /></>))
  expect(screen.getAllByRole('link').map((l) => l.getAttribute('href'))).toEqual(['/m/local/p/9', '/m/lap/p/4', '/m/local/p/9', '/m/lap/p/4'])
  expect(screen.getAllByText('Companion on laptop')).toHaveLength(2)
  expect(screen.getAllByText('Companion on abhash-arm')).toHaveLength(2)
})

test("Settings picks this hub's own companion among others", async () => {
  const other = host('lap', { config: { id: 'lap', name: 'laptop', kind: 'local' }, panes: [agentPane('%4', 'codex', { companion: true })] })
  useStore.setState({ hosts: [other, withCompanion()] })
  api.companion.mockImplementation(async () => ({ agent: 'claude', host: 'local', pane: '%9' }))
  await render(inRouter(<CompanionRows />))
  expect(screen.getByText('On abhash-arm, in the tmux session companion')).toBeTruthy()
})

test('Home and the sidebar show it only while it runs, with what it needs', async () => {
  const { container } = await render(inRouter(<><CompanionBar /><CompanionNavItem className={() => ''} /></>))
  expect(container.textContent).toBe('/settings')
  await act(async () => useStore.setState({ hosts: [withCompanion('waiting', 'Allow inyon send?')] }))
  expect(screen.getByText('Allow inyon send?')).toBeTruthy()
  const links = screen.getAllByRole('link')
  expect(links.map((l) => l.getAttribute('href'))).toEqual(['/m/local/p/9', '/m/local/p/9'])
})
