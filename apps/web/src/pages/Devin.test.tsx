import { beforeEach, expect, test } from 'bun:test'
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { DevinPage } from './Devin.tsx'
import { api } from '../test/fakes.ts'
import { renderSettled as render, flush } from '../test/render.tsx'
import type { DevinUpdate } from '../lib/devin.ts'

const saved: DevinUpdate = {
  version: 1, reset: true, loading: false,
  session: { id: 'test-chat', title: 'Saved conversation', cwd: '/tmp/example', model: 'test-model', updated: 1 },
  state: { status: 'idle', source: 'cli', tmux: 'existing-manual-cli', pane: '%7' },
  order: ['m1'], messages: [{ id: 'm1', role: 'assistant', text: 'Saved answer', tools: [] }],
}
const open = () => render(<MemoryRouter initialEntries={['/m/example/devin/test-chat']}><Routes><Route path="m/:host/devin/:session" element={<DevinPage />} /></Routes></MemoryRouter>)
beforeEach(() => { api.devinRead.mockResolvedValue(saved) })

test('an existing tmux CLI is visible and messages are sent by stable session ID', async () => {
  api.devinSend.mockResolvedValue({ status: 'sent', tmux: 'existing-manual-cli', pane: '%7' })
  await open()
  expect(screen.getByRole('status').textContent).toContain('CLI in tmux: existing-manual-cli')
  expect(screen.getByRole('radio', { name: 'Chat' }).getAttribute('aria-checked')).toBe('true')
  expect(screen.getByRole('radio', { name: 'Terminal' })).toBeTruthy()
  fireEvent.change(screen.getByLabelText('Message Devin'), { target: { value: 'next message' } })
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  await flush()
  expect(api.devinSend).toHaveBeenCalledTimes(1)
  expect(api.devinSend.mock.calls[0]!.slice(0, 3)).toEqual(['example', 'test-chat', 'next message'])
})

test('busy sessions cannot send and the unsent draft is preserved', async () => {
  api.devinRead.mockResolvedValue({ ...saved, state: { status: 'busy', source: 'app' } })
  await open()
  const input = screen.getByLabelText('Message Devin') as HTMLTextAreaElement
  fireEvent.change(input, { target: { value: 'keep this draft' } })
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  await flush()
  expect(api.devinSend).not.toHaveBeenCalled()
  expect(input.value).toBe('keep this draft')
  expect(screen.getByText('Saved answer')).toBeTruthy()
})

test('late history layout follows the tail until the reader scrolls up', async () => {
  const Original = globalThis.ResizeObserver
  const observers: { callback: () => void; elements: HTMLElement[] }[] = []
  globalThis.ResizeObserver = class {
    item: { callback: () => void; elements: HTMLElement[] }
    constructor(callback: () => void) { this.item = { callback, elements: [] }; observers.push(this.item) }
    observe(el: HTMLElement) { this.item.elements.push(el) }
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
  try {
    await open()
    const observer = observers.find((o) => o.elements.some((el) => el.classList.contains('overflow-y-auto')))!
    const scroller = observer.elements[0]!
    const resized = observer.callback
    let height = 1000
    Object.defineProperties(scroller, { scrollHeight: { get: () => height }, clientHeight: { value: 200 } })
    await act(async () => resized())
    expect(scroller.scrollTop).toBe(1000)
    height = 2000
    await act(async () => resized())
    expect(scroller.scrollTop).toBe(2000)
    fireEvent.wheel(scroller, { deltaY: -100 })
    scroller.scrollTop = 1500
    fireEvent.scroll(scroller)
    height = 2400
    await act(async () => resized())
    expect(scroller.scrollTop).toBe(1500)
    fireEvent.click(screen.getByRole('button', { name: 'Jump to latest' }))
    expect(scroller.scrollTop).toBe(2400)
  } finally { globalThis.ResizeObserver = Original }
})

test('an invalid first response retries without a blank page or losing the draft', async () => {
  api.devinRead.mockResolvedValueOnce({} as DevinUpdate).mockResolvedValue(saved)
  await open()
  expect(screen.getByRole('alert').textContent).toContain('Retrying')
  const input = screen.getByLabelText('Message Devin') as HTMLTextAreaElement
  fireEvent.change(input, { target: { value: 'keep draft while reconnecting' } })
  await waitFor(() => expect(screen.getByText('Saved answer')).toBeTruthy(), { timeout: 2000 })
  expect(input.value).toBe('keep draft while reconnecting')
  expect(screen.queryByRole('alert')).toBeNull()
  expect(api.devinRead.mock.calls[1]).toEqual(['example', 'test-chat', 0])
})

test('a malformed incremental response preserves history and its cursor until recovery', async () => {
  api.devinRead.mockResolvedValueOnce(saved).mockResolvedValueOnce({ ...saved, version: 99, messages: undefined } as unknown as DevinUpdate)
    .mockResolvedValue({ ...saved, version: 2, reset: false, order: ['m1', 'm2'], messages: [{ id: 'm2', role: 'assistant', text: 'Recovered answer', tools: [] }] })
  await open()
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Retrying'), { timeout: 2000 })
  expect(screen.getByText('Saved answer')).toBeTruthy()
  await waitFor(() => expect(screen.getByText('Recovered answer')).toBeTruthy(), { timeout: 2000 })
  expect(api.devinRead.mock.calls[2]).toEqual(['example', 'test-chat', 1])
})

test('tool calls and intermediate output share the final reply footer', async () => {
  api.devinRead.mockResolvedValue({ ...saved, order: ['a', 't', 'z'], messages: [
    { id: 'a', role: 'assistant', text: 'Checking the files', tools: [{ name: 'Read', input: {} }], time: '2026-01-01T01:01:00' },
    { id: 't', role: 'tool', text: 'Found file', tools: [], time: '2026-01-01T01:02:00' },
    { id: 'z', role: 'assistant', text: 'All done', tools: [], time: '2026-01-01T01:03:00' },
  ] })
  await open()
  expect(screen.getByText('All done')).toBeTruthy()
  expect(screen.queryByText('01:01')).toBeNull()
  expect(screen.queryByText('01:02')).toBeNull()
  expect(screen.getAllByRole('button', { name: 'Copy' })).toHaveLength(1)
})
