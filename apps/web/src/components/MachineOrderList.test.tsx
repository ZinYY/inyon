import { expect, test } from 'bun:test'
import { act, fireEvent, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { MachinesPage } from '../pages/Machines.tsx'
import { useStore } from '../lib/store.ts'
import { api, emit } from '../test/fakes.ts'
import { host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const rows = () => [...document.querySelectorAll('[data-machine-id]')].map((e) => (e as HTMLElement).dataset.machineId)
const setup = async () => {
  useStore.setState({ hosts: [host('a'), host('b'), host('c')], hostsLoaded: true })
  return render(<MemoryRouter><MachinesPage /></MemoryRouter>)
}

test('moving a handle saves hub order, which survives a fresh overview and page remount', async () => {
  const view = await setup()
  fireEvent.keyDown(screen.getByRole('button', { name: 'Reorder c' }), { key: 'Home' })
  await flush()
  expect(api.orderHost).toHaveBeenCalledWith('c', 'a')
  expect(rows()).toEqual(['c', 'a', 'b'])
  expect(useStore.getState().hosts.map((h) => h.config.id)).toEqual(['c', 'a', 'b'])
  act(() => emit({ t: 'overview', hosts: useStore.getState().hosts }))
  view.unmount()
  await render(<MemoryRouter><MachinesPage /></MemoryRouter>)
  expect(rows()).toEqual(['c', 'a', 'b'])
})

test('a failed save restores the list', async () => {
  await setup()
  api.orderHost.mockRejectedValueOnce(new Error('disk full'))
  fireEvent.keyDown(screen.getByRole('button', { name: 'Reorder c' }), { key: 'Home' })
  await flush()
  expect(rows()).toEqual(['a', 'b', 'c'])
  expect(useStore.getState().toasts.at(-1)?.title).toBe('Could not save machine order')
})

test('touch dragging saves on release; cancellation and tapping do not reorder', async () => {
  await setup()
  const handle = screen.getByRole('button', { name: 'Reorder c' })
  for (const el of document.querySelectorAll('[data-machine-id]')) {
    el.getBoundingClientRect = () => {
      const top = rows().indexOf((el as HTMLElement).dataset.machineId) * 60
      return { top, bottom: top + 60, height: 60, left: 0, right: 300, width: 300, x: 0, y: top, toJSON() {} }
    }
  }
  const down = () => fireEvent.pointerDown(handle, { pointerType: 'touch', pointerId: 1, button: 0, clientY: 150 })
  const up = () => fireEvent.pointerUp(handle, { pointerType: 'touch', pointerId: 1, clientY: 10 })
  down(); up()
  expect(api.orderHost).not.toHaveBeenCalled()
  down()
  fireEvent.pointerMove(handle, { pointerId: 1, clientY: 10 })
  expect(rows()).toEqual(['c', 'a', 'b'])
  expect(api.orderHost).not.toHaveBeenCalled()
  fireEvent.pointerCancel(handle, { pointerId: 1 })
  expect(rows()).toEqual(['a', 'b', 'c'])
  down(); fireEvent.pointerMove(handle, { pointerId: 1, clientY: 10 }); up()
  await flush()
  expect(api.orderHost).toHaveBeenCalledWith('c', 'a')
  expect(rows()).toEqual(['c', 'a', 'b'])
})
