import { beforeEach, expect, test } from 'bun:test'
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { DevinPage } from './Devin.tsx'
import { api } from '../test/fakes.ts'
import { renderSettled as render, flush } from '../test/render.tsx'
import type { DevinUpdate } from '../lib/devin.ts'
import { useStore } from '../lib/store.ts'

const saved: DevinUpdate = {
  version: 1, reset: true, loading: false,
  session: { id: 'test-chat', title: 'Saved conversation', cwd: '/tmp/example', model: 'test-model', updated: 1 },
  state: { status: 'idle', source: 'cli', tmux: 'existing-manual-cli', pane: '%7' },
  order: ['m1'], messages: [{ id: 'm1', role: 'assistant', text: 'Saved answer', tools: [] }],
}
const open = () => render(<MemoryRouter initialEntries={['/m/example/devin/test-chat']}><Routes><Route path="m/:host/devin/:session" element={<DevinPage />} /></Routes></MemoryRouter>)
beforeEach(() => { api.devinRead.mockResolvedValue(saved); useStore.setState({ connected: true }) })

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
  expect(input.disabled).toBe(true)
  await waitFor(() => expect(screen.getByText('Saved answer')).toBeTruthy(), { timeout: 2000 })
  expect(input.value).toBe('')
  expect(input.disabled).toBe(false)
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

test('a loading reset leaves saved history and the draft intact until the complete update', async () => {
  api.devinRead.mockResolvedValueOnce(saved)
    .mockResolvedValueOnce({ ...saved, version: 2, loading: true, order: [], messages: [] })
    .mockResolvedValue({ ...saved, version: 3, reset: false, order: ['m1', 'm2'], messages: [{ id: 'm2', role: 'assistant', text: 'Complete update', tools: [] }] })
  await open()
  const input = screen.getByLabelText('Message Devin') as HTMLTextAreaElement
  fireEvent.change(input, { target: { value: 'Keep this draft' } })
  await waitFor(() => expect(input.disabled).toBe(true), { timeout: 2000 })
  expect(screen.getByText('Saved answer').closest('[inert]')).toBeNull()
  expect(screen.getByText('Reading saved history')).toBeTruthy()
  await waitFor(() => expect(screen.getByText('Complete update')).toBeTruthy(), { timeout: 2000 })
  expect(api.devinRead.mock.calls[2]).toEqual(['example', 'test-chat', 1])
  expect(input.disabled).toBe(false)
  expect(input.value).toBe('Keep this draft')
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

test('prepending older parts and resizing above them preserves the visible reading anchor', async () => {
  const messages = Array.from({ length: 171 }, (_, i) => ({ id: `part-${i}`, role: 'assistant' as const, text: `Reply part ${i}`, tools: [] }))
  api.devinRead.mockResolvedValue({ ...saved, order: messages.map((m) => m.id), messages })
  const Original = globalThis.ResizeObserver
  const rect = HTMLElement.prototype.getBoundingClientRect
  let resized = () => {}
  let shift = 0
  globalThis.ResizeObserver = class {
    callback: () => void
    constructor(callback: () => void) { this.callback = callback }
    observe(el: HTMLElement) { if (el.classList.contains('overflow-y-auto')) resized = this.callback }
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
  try {
    await open()
    const scroller = screen.getByText('Reply part 170').closest('.overflow-y-auto') as HTMLElement
    const anchors = () => [...scroller.querySelectorAll<HTMLElement>('[data-chat-anchor]')]
    Object.defineProperties(scroller, { scrollHeight: { get: () => anchors().length * 100 + shift }, clientHeight: { value: 200 } })
    HTMLElement.prototype.getBoundingClientRect = function () {
      const index = anchors().indexOf(this)
      const top = index < 0 ? 0 : index * 100 + shift - scroller.scrollTop
      return { top, bottom: top + 100, height: 100, width: 200, left: 0, right: 200, x: 0, y: top, toJSON() {} }
    }
    fireEvent.wheel(scroller, { deltaY: -100 })
    scroller.scrollTop = 100
    fireEvent.scroll(scroller)
    const visibleId = anchors()[1]!.dataset.chatAnchor
    fireEvent.click(screen.getByRole('button', { name: 'Load earlier messages' }))
    // The first assistant turn is rebuilt as earlier parts join it. Match
    // stable native part IDs, not old DOM nodes or the changing turn key.
    expect(anchors()).toHaveLength(160)
    expect(scroller.scrollTop).toBe(8100)
    expect(anchors().find((el) => el.dataset.chatAnchor === visibleId)!.getBoundingClientRect().top).toBe(0)
    shift = 500
    await act(async () => resized())
    expect(scroller.scrollTop).toBe(8600)
    expect(anchors().find((el) => el.dataset.chatAnchor === visibleId)!.getBoundingClientRect().top).toBe(0)
  } finally { globalThis.ResizeObserver = Original; HTMLElement.prototype.getBoundingClientRect = rect }
})
