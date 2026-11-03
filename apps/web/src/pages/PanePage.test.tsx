import { afterEach, expect, test } from 'bun:test'
import { act, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { PanePage } from './PanePage.tsx'
import { LEAVE_MS } from '../components/ui/AnimatedList.tsx'
import { useStore } from '../lib/store.ts'
import { agentPane, host } from '../test/fixtures.ts'
import { renderSettled as render } from '../test/render.tsx'

const viewport = (width: number, height: number) => act(() => {
  ;(window as any).happyDOM.setViewport({ width, height })
  window.dispatchEvent(new Event('resize'))
})
afterEach(() => viewport(1024, 768))

const page = () =>
  render(
    <MemoryRouter initialEntries={['/m/box/p/1']}>
      <Routes>
        <Route path="/m/:host/p/:pane" element={<PanePage />} />
      </Routes>
    </MemoryRouter>,
  )

test('the side panel holds its room while it loads, comes in from its edge, and fades out before it goes', async () => {
  viewport(1440, 900)
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude')] })], hostsLoaded: true, connected: true })
  useStore.getState().setSettings({ sidePanel: true })
  const { container } = await page()
  // an aside from the first paint: empty until the panel's code is in, then the real one rising in
  expect(container.querySelector('aside')).not.toBeNull()
  await waitFor(() => expect(screen.getByRole('tablist')).toBeTruthy())
  const aside = container.querySelector('aside')!
  expect(aside.className).toContain('animate-panel-in')
  act(() => useStore.getState().setSettings({ sidePanel: false }))
  expect(container.querySelector('aside')).toBe(aside)
  expect(aside.className).toContain('animate-fade-out')
  await act(() => new Promise((r) => setTimeout(r, LEAVE_MS + 20)))
  expect(container.querySelector('aside')).toBeNull()
})

test('the chat comes in the way the terminal does, and again for another session', async () => {
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude'), agentPane('%2', 'claude')] })], hostsLoaded: true, connected: true })
  const { container } = await page()
  await waitFor(() => expect(screen.getByText('Loading the conversation')).toBeTruthy())
  const chat = screen.getByText('Loading the conversation').closest('.animate-page')
  expect(chat).not.toBeNull()
  expect(screen.getByRole('radiogroup', { name: 'View' })).toBeTruthy()
  expect(screen.getByRole('radio', { name: 'Chat' }).getAttribute('aria-checked')).toBe('true')
  expect(screen.getByRole('radio', { name: 'Terminal' })).toBeTruthy()
  expect(container.querySelector('.animate-page')).toBe(chat)
})

test('the companion goes by that name in its header, whatever its conversation is called', async () => {
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude', { companion: true, title: 'Status check' })] })], hostsLoaded: true, connected: true })
  await page()
  await waitFor(() => expect(screen.getAllByText('Companion').length).toBeGreaterThan(0))
  expect(screen.queryByText('Status check')).toBeNull()
})

test('a terminal-first agent opens on its terminal, and its chat tab is still there', async () => {
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'pi')] })], hostsLoaded: true, connected: true })
  await page()
  await waitFor(() => expect(screen.getByRole('radio', { name: 'Terminal' }).getAttribute('aria-checked')).toBe('true'))
  expect(screen.getByRole('radio', { name: 'Chat' }).getAttribute('aria-checked')).toBe('false')
  expect(screen.queryByText('Pi runs in the terminal')).toBeNull()
})
