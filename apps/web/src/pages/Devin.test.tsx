import { beforeEach, expect, test } from 'bun:test'
import { fireEvent, screen } from '@testing-library/react'
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
  expect(screen.getByText(/CLI in tmux: existing-manual-cli/)).toBeTruthy()
  expect(screen.getByRole('link', { name: 'Terminal' }).getAttribute('href')).toBe('/m/example/p/7?view=terminal')
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
