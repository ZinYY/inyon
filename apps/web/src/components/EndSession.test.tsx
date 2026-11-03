import { beforeEach, describe, expect, test } from 'bun:test'
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import type { HostState } from '@inyon/protocol'
import { Home } from '../pages/Home.tsx'
import { ConfirmHost } from './Confirm.tsx'
import { hidePane, useStore, withoutEnded } from '../lib/store.ts'
import { endText } from '../lib/end.ts'
import { allPanes } from '../lib/overview.ts'
import { api, emit } from '../test/fakes.ts'
import { agentPane, fresh, host, pane } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'

/** One machine: "api" holds the login session and a shell, split in one window; "docs" has one window. */
function box(): HostState {
  const h = host('box')
  h.config.name = 'Studio'
  h.sessions = [
    { id: '$1', name: 'api', attached: 0, windows: [{ id: '@1', index: 1, name: '', active: true, activity: 9, panes: [agentPane('%1', 'claude', { title: 'Fix the login test' }), pane('%2', { index: 1 })] }] },
    { id: '$2', name: 'docs', attached: 0, windows: [{ id: '@2', index: 1, name: '', active: true, activity: 5, panes: [agentPane('%3', 'claude', { title: 'Write the docs' })] }] },
  ]
  return h
}

const card = (title: string) => screen.queryByRole('link', { name: title })

beforeEach(() => useStore.setState({ hosts: [box()], hostsLoaded: true, toasts: [] }))

async function endFromCard(title: string) {
  const user = userEvent.setup()
  await render(<MemoryRouter><Home /><ConfirmHost /></MemoryRouter>)
  await user.click(screen.getByRole('button', { name: `Actions for ${title}` }))
  await user.click(screen.getByRole('menuitem', { name: /End session/ }))
  return user
}

describe('ending a session', () => {
  test('the sheet names the session, machine and tmux target, and the card goes at once', async () => {
    let finish!: () => void
    api.end.mockImplementation(() => new Promise((r) => (finish = () => r({ closed: 'pane' }))))
    const user = await endFromCard('Fix the login test')
    const sheet = screen.getByRole('dialog')
    expect(within(sheet).getByText('End this session?')).toBeTruthy()
    expect(within(sheet).getByText('Studio')).toBeTruthy()
    expect(within(sheet).getByText('api:1.0')).toBeTruthy()
    expect(within(sheet).getByText(/Claude Code exits, then its tmux pane closes\. Your other windows stay/)).toBeTruthy()
    await user.click(within(sheet).getByRole('button', { name: 'End session' }))
    // on its way out before the machine has even answered: fading, and gone a moment later
    expect(card('Fix the login test')!.closest('[data-leaving]')).toBeTruthy()
    await new Promise((r) => setTimeout(r, LEAVE_MS + 30))
    await flush()
    expect(card('Fix the login test')).toBeNull()
    expect(card('Write the docs')).toBeTruthy()
    expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['Session ended'])
    expect(api.end.mock.calls).toEqual([['box', '%1']])
    // an update that still lists it, sent before tmux closed it, doesn't bring it back
    emit({ t: 'overview', hosts: [fresh(box())] } as any)
    await flush()
    expect(card('Fix the login test')).toBeNull()
    finish()
    await flush()
  })

  test('cancel leaves it alone', async () => {
    const user = await endFromCard('Write the docs')
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(api.end).not.toHaveBeenCalled()
    expect(card('Write the docs')).toBeTruthy()
  })

  test('a failure brings the card back with the reason', async () => {
    api.end.mockImplementation(async () => {
      throw new Error('The machine is offline')
    })
    const user = await endFromCard('Write the docs')
    await user.click(screen.getByRole('button', { name: 'End session' }))
    await flush()
    emit({ t: 'overview', hosts: [fresh(box())] } as any)
    await flush()
    expect(card('Write the docs')).toBeTruthy()
    expect(useStore.getState().toasts.map((t) => [t.title, t.body])).toContainEqual(['Could not end the session', 'The machine is offline'])
  })

  test('the sheet says when the window or tmux session closes with it', () => {
    const [login, , docs] = allPanes([box()])
    expect(endText(login)).not.toContain('with it')
    expect(endText(docs)).toContain('the tmux session “docs” with it')
    const h = box()
    h.sessions[1].windows.push({ ...h.sessions[1].windows[0], id: '@3', index: 2, panes: [pane('%4')] })
    expect(endText(allPanes([h])[2])).toContain('window docs:1 with it')
  })

  test('hidden panes drop their empty window and session from the lists', () => {
    expect(withoutEnded([box()])).toEqual([box()])
    hidePane('box', '%3')
    hidePane('box', '%2')
    const [h] = withoutEnded([box()])
    expect(h.sessions.map((s) => [s.name, s.windows.map((w) => w.panes.map((p) => p.id))])).toEqual([['api', [['%1']]]])
    // they come back after a while, in case tmux never closed them
    expect(withoutEnded([box()], Date.now() + 16_000)).toEqual([box()])
  })
})
