import { beforeEach, expect, test } from 'bun:test'
import { act, fireEvent, screen } from '@testing-library/react'
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
