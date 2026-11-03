import { describe, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import type { HostConfig, HostState } from '@inyon/protocol'
import { MachineSheet, NewSessionSheet, PairMachine } from './Sheets.tsx'
import { useStore } from '../lib/store.ts'
import { api, emit } from '../test/fakes.ts'
import { agentPane, fresh, host } from '../test/fixtures.ts'

const agentPaneIn = (cwd: string) => agentPane('%9', 'claude', { cwd })
import { flush, renderSettled as render } from '../test/render.tsx'

const noop = () => {}
const field = (placeholder: string) => screen.getByPlaceholderText(placeholder) as HTMLInputElement
/** A status refresh from the hub: the same machines, every object rebuilt. */
const poll = (hosts: HostState[]) => act(() => emit({ t: 'overview', hosts: fresh(hosts) }))

describe('MachineSheet (hub) keeps what you type while live data flows', () => {
  const work: HostConfig = { id: 'm1', name: 'Work', kind: 'ssh', target: 'deploy@work', port: 2222, identityFile: '~/.ssh/work' }
  const lab: HostConfig = { id: 'm2', name: 'Lab', kind: 'ssh', target: 'me@lab' }

  test('editing: name, destination, port and key file survive refreshes', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<MachineSheet open onClose={noop} edit={work} />)
    const name = field('Work server')
    await user.clear(name)
    await user.type(name, 'Bu')
    await user.type(field('deploy@work-server'), '-2')
    await user.clear(field('~/.ssh/id_ed25519'))
    await user.type(field('~/.ssh/id_ed25519'), '~/k')
    rerender(<MachineSheet open onClose={noop} edit={fresh(work)} />)
    await user.type(name, 'ild box')
    await user.type(field('22'), '0')
    expect(name.value).toBe('Build box')
    expect(field('deploy@work-server').value).toBe('deploy@work-2')
    expect(field('22').value).toBe('22220')
    expect(field('~/.ssh/id_ed25519').value).toBe('~/k')
  })

  test('adding over SSH: the chosen mode and fields survive refreshes', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<MachineSheet open onClose={noop} />)
    await user.click(screen.getByText('Reach it over SSH'))
    await user.type(field('Work server'), 'Ne')
    rerender(<MachineSheet open onClose={noop} edit={undefined} />)
    await user.type(field('Work server'), 'w')
    expect(field('Work server').value).toBe('New')
  })

  test('reopening shows current values; switching machines shows the other one', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<MachineSheet open onClose={noop} edit={work} />)
    await user.type(field('Work server'), ' draft')
    rerender(<MachineSheet open={false} onClose={noop} edit={work} />)
    rerender(<MachineSheet open onClose={noop} edit={{ ...work, name: 'Work 2' }} />)
    expect(field('Work server').value).toBe('Work 2')
    rerender(<MachineSheet open onClose={noop} edit={lab} />)
    expect(field('Work server').value).toBe('Lab')
    expect(field('22').value).toBe('')
    expect(field('~/.ssh/id_ed25519').value).toBe('')
  })

  test('saving sends what was typed', async () => {
    const user = userEvent.setup()
    await render(<MachineSheet open onClose={noop} edit={work} />)
    await user.clear(field('Work server'))
    await user.type(field('Work server'), 'Build')
    await user.click(screen.getByText('Save'))
    await flush()
    expect(api.updateHost.mock.calls[0]).toEqual(['m1', { name: 'Build', target: 'deploy@work', port: 2222, identityFile: '~/.ssh/work' }])
  })
})

describe('PairMachine', () => {
  test('the name survives the machine list refreshing', async () => {
    const user = userEvent.setup()
    const hosts = [host('a')]
    useStore.setState({ hosts })
    await render(<PairMachine />)
    await user.type(field('My laptop'), 'Lap')
    poll(hosts)
    await user.type(field('My laptop'), 'top 2')
    expect(field('My laptop').value).toBe('Laptop 2')
  })

  test('with the command shown, the mark draws its line while it waits for the machine, and a tick says when it came', async () => {
    const user = userEvent.setup()
    api.pair.mockResolvedValue({ id: 'lap', token: 't', command: 'curl -fsSL https://hub/pair | sh' })
    const { container } = await render(<PairMachine />)
    await user.click(screen.getByRole('button', { name: 'Create pairing command' }))
    await flush()
    expect(screen.getByText('Waiting for the machine to connect…')).toBeTruthy()
    expect(container.querySelector('.mark-connecting')).toBeTruthy()
    expect(container.querySelector('.animate-spin')).toBeNull()
    act(() => emit({ t: 'overview', hosts: fresh([host('lap', { config: { id: 'lap', name: 'Laptop', kind: 'node' } })]) }))
    expect(screen.getByText('Laptop is connected.')).toBeTruthy()
    expect(container.querySelector('.mark-connecting')).toBeNull()
  })
})

describe('NewSessionSheet', () => {
  const hosts = [host('box', { agents: ['claude', 'codex'] }), host('pi', { agents: ['claude'] })]
  const sheet = (open = true) => (
    <MemoryRouter>
      <NewSessionSheet open={open} onClose={noop} />
    </MemoryRouter>
  )
  const startButton = () => screen.getByText(/^Start /).closest('button') as HTMLButtonElement

  test('folder, agent, name and first message survive refreshes', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts })
    await render(sheet())
    await user.click(screen.getByText('Codex'))
    const folder = screen.getByDisplayValue('~') as HTMLInputElement
    await user.type(folder, '/co')
    await user.type(field('codex'), 'ap')
    await user.type(field('What should it work on?'), 'Fix the')
    poll(hosts)
    poll(hosts.map((h) => ({ ...h, latencyMs: 40 })))
    await user.type(folder, 'de')
    await user.type(field('codex'), 'i')
    await user.type(field('What should it work on?'), ' tests')
    expect(folder.value).toBe('~/code')
    expect(field('codex').value).toBe('api')
    expect(field('What should it work on?').value).toBe('Fix the tests')
    expect(startButton().textContent).toBe('Start Codex')
  })

  test('your own agents start with their own id, as the agent they work like', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts })
    api.settings.mockResolvedValue({ uploads: {} as any, customAgents: [{ id: 'yolo', name: 'Claude YOLO', command: 'claude --dangerously-skip-permissions', like: 'claude' }] })
    api.launch.mockResolvedValue({ pane: '%3' })
    await render(sheet())
    await flush()
    await user.click(screen.getByText('Claude YOLO'))
    expect(startButton().textContent).toBe('Start Claude YOLO')
    // picking a built-in one leaves yours
    await user.click(screen.getByText('Codex'))
    expect(startButton().textContent).toBe('Start Codex')
    await user.click(screen.getByText('Claude YOLO'))
    expect(field('Claude YOLO')).toBeTruthy()
    // the agent it works like still offers its permissions
    expect(screen.getByRole('combobox', { name: 'Permissions' })).toBeTruthy()
    await user.click(startButton())
    await flush()
    expect(api.launch.mock.calls[0][1]).toMatchObject({ agent: 'claude', custom: 'yolo' })
    api.settings.mockResolvedValue({} as any)
  })

  test('the machine you picked stays picked', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts })
    await render(sheet())
    const machine = screen.getByRole('combobox', { name: 'Machine' })
    expect(machine.textContent).toContain('box')
    await user.click(machine)
    await user.click(screen.getByRole('option', { name: /^pi/ }))
    await new Promise((r) => setTimeout(r, 250))
    poll(hosts)
    expect(screen.getByRole('combobox', { name: 'Machine' }).textContent).toContain('pi')
  })

  test('reopening starts a fresh message with the default agent', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts })
    const { rerender } = await render(sheet())
    await user.click(screen.getByText('Codex'))
    await user.type(field('codex'), 'x')
    await user.type(field('What should it work on?'), 'old')
    rerender(sheet(false))
    rerender(sheet(true))
    expect(startButton().textContent).toBe('Start Claude Code')
    expect(field('What should it work on?').value).toBe('')
  })

  // opened before the first overview arrives (app starting, phone reconnecting)
  test('opened before the machines load, it picks one once they arrive', async () => {
    await render(sheet())
    poll(hosts)
    expect(startButton().disabled).toBe(false)
    expect(startButton().textContent).toBe('Start Claude Code')
  })

  // it read "…/C:\Users\abhash\inyon-demo\checkout-service": a "/" cut on a path that has none
  test("a recent Windows folder's chip keeps its last two parts, with backslashes", async () => {
    const user = userEvent.setup()
    const windows = host('laptop', { panes: [agentPaneIn('C:\\Users\\abhash\\inyon-demo\\checkout-service')] })
    useStore.setState({ hosts: [windows] })
    await render(sheet())
    const chip = screen.getByRole('button', { name: '…\\inyon-demo\\checkout-service' })
    expect(chip.getAttribute('title')).toBe('C:\\Users\\abhash\\inyon-demo\\checkout-service')
    await user.click(chip)
    expect(screen.getByDisplayValue('C:\\Users\\abhash\\inyon-demo\\checkout-service')).toBeTruthy()
  })
})
