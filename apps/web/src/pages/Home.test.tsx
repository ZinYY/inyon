import { beforeEach, describe, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { Home } from './Home.tsx'
import { useStore } from '../lib/store.ts'
import { api } from '../test/fakes.ts'
import { agentPane, host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const asking = (id = '%1', title = 'Failing tests') =>
  agentPane(id, 'claude', { title, agent: { kind: 'claude', pid: 1, state: 'waiting', waitingFor: 'Bash command: npm test', options: [{ n: 1, label: 'Yes' }, { n: 2, label: 'No' }] } })

const home = () =>
  render(
    <MemoryRouter>
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/m/:host/p/:pane" element={<p>chat open</p>} />
      </Routes>
    </MemoryRouter>,
  )

beforeEach(() => useStore.setState({ hostsLoaded: true, toasts: [], connected: true }))

describe('agent sessions', () => {
  // a Needs-you section above the list grew and shrank as agents asked, moving every card under the pointer
  test('a session that asks something answers from its own card, in its own place', async () => {
    useStore.setState({ hosts: [host('box', { panes: [agentPane('%2', 'claude', { title: 'Write docs' }), asking('%3')] })] })
    await home()
    // shown once, as its card, not again in a section of its own
    expect(screen.getAllByText('Failing tests')).toHaveLength(1)
    const titles = () => screen.getAllByRole('link').map((l) => l.getAttribute('aria-label')).filter(Boolean)
    expect(titles()).toEqual(['Write docs', 'Failing tests'])
    await userEvent.click(screen.getByRole('button', { name: 'Yes' }))
    expect(api.answer).toHaveBeenCalledWith('box', '%3', 1)
    // the agent moves on: the same two cards, in the same order
    await act(() => useStore.setState({ hosts: [host('box', { panes: [agentPane('%2', 'claude', { title: 'Write docs' }), agentPane('%3', 'claude', { title: 'Failing tests' })] })] }))
    expect(titles()).toEqual(['Write docs', 'Failing tests'])
  })

  test('Show brings the session that needs you into view and puts the focus on its answer', async () => {
    useStore.setState({ hosts: [host('box', { panes: [agentPane('%2', 'claude', { title: 'Write docs' }), asking('%3')] })] })
    await home()
    expect(screen.getByText('1 session needs you.')).toBeTruthy()
    const into: Element[] = []
    const was = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function () { into.push(this) }
    try {
      await userEvent.click(screen.getByRole('button', { name: 'Show' }))
    } finally {
      Element.prototype.scrollIntoView = was
    }
    expect(into[0]?.textContent).toContain('Failing tests')
    expect(document.activeElement?.textContent).toBe('Yes')
  })

  test("a waiting session's name opens its chat", async () => {
    useStore.setState({ hosts: [host('box', { panes: [asking()] })] })
    await home()
    await userEvent.click(screen.getByRole('link', { name: 'Failing tests' }))
    expect(screen.getByText('chat open')).toBeTruthy()
  })
})

describe('turning the integration on from the overview', () => {
  const off = () => host('box', { integration: 'missing' })

  test('the card keeps a spinner until the machine answers, then says it is on', async () => {
    let done!: () => void
    api.integration.mockImplementationOnce(() => new Promise<any>((r) => (done = () => r({ ok: true }))))
    useStore.setState({ hosts: [off()] })
    await home()
    await userEvent.click(screen.getByRole('button', { name: 'Turn on' }))
    expect(api.integration).toHaveBeenCalledWith('box', true)
    // the hub reports the new state before the request returns: the card stays, busy
    act(() => useStore.setState({ hosts: [host('box', { integration: 'installed' })] }))
    const button = screen.getByRole('button', { name: /Turning on/ }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(screen.getByText(/turn on the integration for live usage limits/)).toBeTruthy()
    done()
    await flush()
    expect(screen.queryByRole('button', { name: /Turn/ })).toBeNull()
    expect(useStore.getState().toasts.map((t) => [t.title, t.tone])).toEqual([['Claude integration on', 'success']])
  })

  test('a failure says so, and the card stays to try again', async () => {
    api.integration.mockImplementationOnce(async () => {
      throw new Error('settings.json is not valid JSON')
    })
    useStore.setState({ hosts: [off()] })
    await home()
    await userEvent.click(screen.getByRole('button', { name: 'Turn on' }))
    await flush()
    expect(useStore.getState().toasts.map((t) => [t.title, t.body, t.tone])).toEqual([['Could not turn on the integration', 'settings.json is not valid JSON', 'error']])
    expect(screen.getByRole('button', { name: 'Turn on' })).toBeTruthy()
  })
})
