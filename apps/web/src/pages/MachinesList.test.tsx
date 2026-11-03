import { describe, expect, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import type { HostState, MachineKind } from '@inyon/protocol'
import { MachinePage, MachinesPage, metaLine } from './Machines.tsx'
import { useStore } from '../lib/store.ts'
import { api } from '../test/fakes.ts'
import { host } from '../test/fixtures.ts'
import { renderSettled as render } from '../test/render.tsx'

const m = (name: string, machineKind?: MachineKind, status: HostState['status'] = 'online') => host(name, { machineKind, status })
const rows = () => screen.getAllByRole('link').filter((a) => a.getAttribute('href')?.startsWith('/machines/')).map((a) => a.getAttribute('href')!.slice(10))
const page = () => render(<MemoryRouter><MachinesPage /></MemoryRouter>)

describe('the machines list', () => {
  test('machines that answer come first, each with the icon of what it is', async () => {
    useStore.setState({ hosts: [m('nas', 'server', 'error'), m('studio', 'desktop'), m('air', 'laptop'), m('vm', 'server')] })
    await page()
    expect(rows()).toEqual(['studio', 'air', 'vm', 'nas'])
    expect(screen.getAllByRole('img', { name: 'Server' })).toHaveLength(2)
    expect(screen.getByRole('img', { name: 'Laptop' })).toBeTruthy()
    expect(screen.getByRole('img', { name: 'Desktop' })).toBeTruthy()
  })

  test('past eight machines a search narrows the list', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: Array.from({ length: 9 }, (_, i) => m(`box-${i}`, i === 4 ? 'laptop' : 'server')) })
    await page()
    await user.type(screen.getByRole('searchbox', { name: 'Search 9 machines' }), 'laptop')
    expect(rows()).toEqual(['box-4'])
    await user.clear(screen.getByRole('searchbox'))
    await user.type(screen.getByRole('searchbox'), 'nothing')
    expect(screen.getByText('No machine matches “nothing”')).toBeTruthy()
  })

  test("before the first answer: rows in the list's shape", async () => {
    useStore.setState({ hosts: [], hostsLoaded: false })
    const { container } = await page()
    expect(screen.getByRole('status', { name: 'Loading machines' })).toBeTruthy()
    expect(container.querySelectorAll('.skeleton').length).toBeGreaterThan(0)
    expect(screen.queryAllByRole('link').filter((a) => a.getAttribute('href')?.startsWith('/machines/'))).toEqual([])
  })

  test('no machines: an invitation to add one', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [], hostsLoaded: true })
    await page()
    expect(screen.getByText('No machines yet')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'Add a machine' }))
    expect(screen.getByRole('dialog')).toBeTruthy()
  })

  test("a paired hub that hasn't answered yet: the page says it's connecting to it", async () => {
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'studio', url: 'http://box:4777', token: 't', pairedAt: 1 }]))
    useStore.setState({ hosts: [], hostsLoaded: true })
    await page()
    localStorage.removeItem('inyon.hubs.v1')
    expect(screen.getByText('Connecting to studio…')).toBeTruthy()
    expect(screen.getByText(/Its computers appear as soon as it answers\. On a VPN, make sure this device is connected to it too\./)).toBeTruthy()
  })

  test("a hub's machine shows why it isn't answering while the hub is out of reach", async () => {
    useStore.setState({ hosts: [{ ...m('srv', 'server', 'connecting'), error: 'Reconnecting to studio' }], hostsLoaded: true })
    await page()
    expect(screen.getByText('Reconnecting to studio')).toBeTruthy()
  })

  test('a few machines, no search box', async () => {
    useStore.setState({ hosts: [m('one'), m('two')] })
    await page()
    expect(screen.queryByRole('searchbox')).toBeNull()
  })

  test('the meta line wraps only between items, never before a dot or inside a count', () => {
    const line = metaLine('build box', 2, 1, 38)
    // the time is how long the machine took to answer, not a network round trip, and says so
    expect(line.replace(/\u00a0/g, ' ')).toBe('build box · 2 sessions · 1 agent · responds in 38 ms')
    // the only ordinary spaces left are the ones after each dot, and inside the address
    expect(line.split(' ')).toEqual(['build', 'box\u00a0·', '2\u00a0sessions\u00a0·', '1\u00a0agent\u00a0·', 'responds\u00a0in\u00a038\u00a0ms'])
    expect(metaLine('studio', 1, 0)).toBe('studio\u00a0· 1\u00a0session')
    expect(metaLine('studio', 1, 0, 1373).replace(/\u00a0/g, ' ')).toBe('studio · 1 session · responds in 1.4 s')
  })
})

describe('a machine page while things load', () => {
  const machine = () => render(
    <MemoryRouter initialEntries={['/machines/studio']}>
      <Routes>
        <Route path="/machines/:host" element={<MachinePage />} />
      </Routes>
    </MemoryRouter>,
  )

  test('before the machines arrive: rows in its shape, and no spinner', async () => {
    const { container } = await machine()
    expect(screen.getByText('Loading this machine')).toBeTruthy()
    expect(container.querySelectorAll('.skeleton').length).toBeGreaterThan(0)
    expect(container.querySelector('.animate-spin')).toBeNull()
  })

  test('the config tab names the machine it is looking on', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [host('studio', { config: { id: 'studio', name: 'Studio', kind: 'node' } })] })
    api.configs.mockReturnValue(new Promise(() => {}))
    const { container } = await machine()
    await user.click(screen.getByRole('radio', { name: 'Config' }))
    expect(screen.getByText('Looking for config files on Studio')).toBeTruthy()
    expect(container.querySelector('.animate-spin')).toBeNull()
  })

  // "abhash-lapto" next to New session on a phone: there the button is its icon, the name gets the room
  test('New session folds to its icon on a phone, and an empty machine offers it in the page too', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [host('studio', { config: { id: 'studio', name: 'abhash-laptop', kind: 'link' }, latencyMs: 1373, sessions: [] })] })
    await machine()
    const [header, empty] = screen.getAllByRole('button', { name: 'New session' })
    expect(header!.querySelector('span')!.className).toContain('max-sm:hidden')
    expect(screen.getByText('No tmux sessions')).toBeTruthy()
    // the time the machine took to answer, said as such
    expect(screen.getByText(/responds in 1\.4 s/)).toBeTruthy()
    await user.click(empty!)
    expect(screen.getByRole('dialog', { name: 'New session' })).toBeTruthy()
  })
})
