import { describe, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import type { HostState, NodeUpdate as Update } from '@inyon/protocol'
import { NodeUpdate } from './NodeUpdate.tsx'
import { MachinePage, MachinesPage } from '../pages/Machines.tsx'
import { ApiError } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'
import { api, emit } from '../test/fakes.ts'
import { fresh, host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const laptop = (nodeUpdate?: Update, more: Partial<HostState> = {}) =>
  host('laptop', { config: { id: 'laptop', name: 'Laptop', kind: 'node' }, nodeVersion: '0.3.0', nodeUpdate, ...more })
const update = () => screen.queryByRole('button', { name: 'Update' })

describe('a machine whose inyon is behind', () => {
  test('shows the version and an Update button, and only updates on a tap', async () => {
    const user = userEvent.setup()
    await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true })} />)
    expect(screen.getByText('inyon 0.3.0 · update available')).toBeTruthy()
    expect(screen.getByText('Version 0.6.3 is ready. inyon restarts for a moment while it updates.')).toBeTruthy()
    expect(api.nodeUpdate).not.toHaveBeenCalled()
    await user.click(update()!)
    await flush()
    expect(api.nodeUpdate.mock.calls).toEqual([['laptop']])
  })

  test('says it is updating in a sentence with a pulsing dot, without a button', async () => {
    const { container } = await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true, state: 'updating' })} />)
    expect(screen.getByText('Updating inyon…')).toBeTruthy()
    expect(screen.getByText('It restarts and reconnects by itself, usually within a minute.')).toBeTruthy()
    expect(update()).toBeNull()
    expect(container.querySelector('.pulse-soft')).toBeTruthy()
    expect(container.querySelector('.animate-spin')).toBeNull()
  })

  test('says when it worked', async () => {
    await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true, state: 'updated' }, { nodeVersion: '0.6.3' })} />)
    expect(screen.getByText('Updated to inyon 0.6.3')).toBeTruthy()
    expect(screen.queryByRole('button')).toBeNull()
  })

  test('shows what went wrong, with a way to try again', async () => {
    const user = userEvent.setup()
    await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true, state: 'failed', error: "the download didn't match the hub's checksum" })} />)
    expect(screen.getByText("Couldn't update inyon")).toBeTruthy()
    expect(screen.getByText("the download didn't match the hub's checksum")).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'Try again' }))
    expect(api.nodeUpdate).toHaveBeenCalledTimes(1)
  })

  test("the hub's refusal is shown in place", async () => {
    const user = userEvent.setup()
    api.nodeUpdate.mockRejectedValue(new ApiError(409, "Laptop isn't connected. Update it once it's back online."))
    await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true })} />)
    await user.click(update()!)
    await flush()
    expect(screen.getByText("Laptop isn't connected. Update it once it's back online.")).toBeTruthy()
  })

  test('too old to update itself: says to run setup again, no button', async () => {
    await render(<NodeUpdate host={laptop({ latest: '0.7.0', supported: false }, { config: { id: 'laptop', name: 'Laptop', kind: 'link' }, nodeVersion: '0.6.3' })} />)
    expect(screen.getByText("This version can't update itself. Run the setup command on Laptop again to get 0.7.0.")).toBeTruthy()
    expect(update()).toBeNull()
  })

  test('offline: the button waits for the machine', async () => {
    await render(<NodeUpdate host={laptop({ latest: '0.6.3', supported: true }, { status: 'connecting' })} />)
    expect((update() as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText('Version 0.6.3 is ready. Update it once Laptop is back online.')).toBeTruthy()
  })

  test('nothing to show when it is current', async () => {
    const { container } = await render(<NodeUpdate host={laptop(undefined, { nodeVersion: '0.6.3' })} />)
    expect(container.textContent).toBe('')
  })
})

describe('on the Machines pages', () => {
  test('the machine page follows the update as the hub reports it', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [laptop({ latest: '0.6.3', supported: true })] })
    await render(
      <MemoryRouter initialEntries={['/machines/laptop']}>
        <Routes>
          <Route path="/machines/:host" element={<MachinePage />} />
        </Routes>
      </MemoryRouter>,
    )
    await user.click(update()!)
    await flush()
    expect(api.nodeUpdate.mock.calls).toEqual([['laptop']])
    act(() => emit({ t: 'overview', hosts: fresh([laptop({ latest: '0.6.3', supported: true, state: 'updating' }, { status: 'connecting' })]) }))
    expect(screen.getByText('Updating inyon…')).toBeTruthy()
    act(() => emit({ t: 'overview', hosts: fresh([laptop({ latest: '0.6.3', supported: true, state: 'updated' }, { nodeVersion: '0.6.3' })]) }))
    expect(screen.getByText('Updated to inyon 0.6.3')).toBeTruthy()
  })

  test('the list marks a machine with an update', async () => {
    useStore.setState({ hosts: [laptop({ latest: '0.6.3', supported: true }), host('box')] })
    await render(
      <MemoryRouter>
        <MachinesPage />
      </MemoryRouter>,
    )
    expect(screen.getAllByText('Update available')).toHaveLength(1)
  })
})
