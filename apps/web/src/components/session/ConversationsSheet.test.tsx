import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { act, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { AGENTS, type PastConversation } from '@inyon/protocol'
import { ConversationsSheet } from './ConversationsSheet.tsx'
import { SessionHeader } from './SessionHeader.tsx'
import type { SessionModel } from './useSession.ts'
import { Composer } from '../chat/Composer.tsx'
import { ConfirmHost } from '../Confirm.tsx'
import { openMenu } from '../ui/primitives.tsx'
import { allPanes } from '../../lib/overview.ts'
import { agentPane, host } from '../../test/fixtures.ts'
import { useStore } from '../../lib/store.ts'
import { api } from '../../test/fakes.ts'
import { flush, renderSettled as render } from '../../test/render.tsx'

const now = Date.now()
const list: PastConversation[] = [
  { id: 'a1', title: 'Fix the login page', at: now - 5 * 60_000, current: true },
  { id: 'b2', title: 'Write the README', at: now - 3 * 3600_000 },
]
const answer = (conversations = list) => api.conversations.mockImplementation(async () => ({ agent: 'claude', cwd: '/srv/app', conversations }))

let closed = 0
const sheet = (p: { working?: boolean; open?: boolean } = {}) => (
  <>
    <ConversationsSheet host="box" pane="%4" agent="claude" working={p.working ?? false} open={p.open ?? true} onClose={() => closed++} />
    <ConfirmHost />
  </>
)
const row = (title: string) => screen.getByText(title).closest('button') as HTMLButtonElement
const toasts = () => useStore.getState().toasts.map((t) => [t.title, t.tone])

beforeEach(() => {
  closed = 0
  useStore.setState({ toasts: [] })
})

describe('Past conversations', () => {
  test('lists them for the pane, newest first, with the open one marked', async () => {
    answer()
    await render(sheet())
    expect(api.conversations.mock.calls).toEqual([['box', '%4', 'claude']])
    expect(screen.getByText('Claude Code · /srv/app')).toBeTruthy()
    expect(screen.getByText('Open now · 5m ago')).toBeTruthy()
    expect(screen.getByText('3h ago')).toBeTruthy()
    expect(row('Fix the login page').getAttribute('aria-current')).toBe('true')
    expect(row('Write the README').getAttribute('aria-current')).toBeNull()
  })

  test('shows that it is reading, then an empty state when there are none', async () => {
    let finish!: () => void
    api.conversations.mockImplementation(() => new Promise((r) => (finish = () => r({ agent: 'claude', cwd: '/srv/app', conversations: [] }))))
    await render(sheet())
    expect(screen.getByText("Reading Claude Code's conversations…")).toBeTruthy()
    await act(async () => finish())
    expect(screen.getByText('No past conversations')).toBeTruthy()
    expect(screen.getByText('Conversations with Claude Code in /srv/app show up here.')).toBeTruthy()
  })

  test('an error says why and can be tried again', async () => {
    api.conversations.mockImplementation(async () => {
      throw new Error('Pane not found')
    })
    const user = userEvent.setup()
    await render(sheet())
    expect(screen.getByText("Couldn't load conversations")).toBeTruthy()
    expect(screen.getByText('Pane not found')).toBeTruthy()
    answer()
    await user.click(screen.getByRole('button', { name: 'Try again' }))
    await flush()
    expect(screen.getByText('Write the README')).toBeTruthy()
  })

  test('picking one while the agent is idle switches at once, then closes', async () => {
    answer()
    const user = userEvent.setup()
    await render(sheet())
    await user.click(row('Write the README'))
    await flush()
    expect(api.switchConversation.mock.calls).toEqual([['box', '%4', 'claude', 'b2']])
    expect(screen.queryByText('Switch conversation?')).toBeNull()
    expect(closed).toBe(1)
    expect(toasts()).toEqual([['Conversation switched', 'success']])
  })

  test('shows progress while it switches, and nothing else can be picked meanwhile', async () => {
    answer([...list, { id: 'c3', title: 'Plan the release', at: now - 86_400_000 * 3 }])
    let finish!: () => void
    api.switchConversation.mockImplementation(() => new Promise((r) => (finish = () => r({ switched: true }))))
    const user = userEvent.setup()
    await render(sheet())
    await user.click(row('Write the README'))
    expect(screen.getByText('Switching…')).toBeTruthy()
    expect(screen.getByRole('status').textContent).toContain('Saving this conversation and opening the other')
    expect(row('Plan the release').disabled).toBe(true)
    await act(async () => finish())
    expect(closed).toBe(1)
  })

  test('while the agent works it asks first, and No leaves everything as it was', async () => {
    answer()
    const user = userEvent.setup()
    await render(sheet({ working: true }))
    await user.click(row('Write the README'))
    const ask = screen.getAllByRole('dialog').at(-1)!
    expect(within(ask).getByText('Switch conversation?')).toBeTruthy()
    expect(within(ask).getByText('The current one is saved and can be resumed later.')).toBeTruthy()
    await user.click(within(ask).getByRole('button', { name: 'Cancel' }))
    await flush()
    expect(api.switchConversation.mock.calls).toEqual([])
    await user.click(row('Write the README'))
    await user.click(within(screen.getAllByRole('dialog').at(-1)!).getByRole('button', { name: 'Switch' }))
    await flush()
    expect(api.switchConversation.mock.calls).toEqual([['box', '%4', 'claude', 'b2']])
  })

  test('the open conversation is not switched to again', async () => {
    answer()
    const user = userEvent.setup()
    await render(sheet())
    await user.click(row('Fix the login page'))
    await flush()
    expect(api.switchConversation.mock.calls).toEqual([])
    expect(closed).toBe(0)
  })

  test('a switch that fails says why and keeps the sheet open', async () => {
    answer()
    api.switchConversation.mockImplementation(async () => {
      throw new Error('Claude Code did not exit; close it from the terminal')
    })
    const user = userEvent.setup()
    await render(sheet())
    await user.click(row('Write the README'))
    await flush()
    expect(closed).toBe(0)
    expect(toasts()).toEqual([['Could not switch', 'error']])
    expect(row('Write the README').disabled).toBe(false)
  })

  test('↓ and ↑ move between conversations', async () => {
    answer()
    const user = userEvent.setup()
    await render(sheet())
    row('Fix the login page').focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(row('Write the README'))
    await user.keyboard('{ArrowUp}')
    expect(document.activeElement).toBe(row('Fix the login page'))
  })
})

describe('typing the agent\'s resume command', () => {
  let opened: string[] = []
  const listen = (e: Event) => {
    opened.push((e as CustomEvent<string>).detail)
    if ((e as CustomEvent<string>).detail === 'conversations') e.preventDefault()
  }
  let n = 0
  const box = () => screen.getByRole('textbox') as HTMLTextAreaElement
  beforeEach(() => {
    opened = []
    window.addEventListener('inyon:open-menu', listen)
    api.commands.mockImplementation(async () => [])
  })
  afterEach(() => window.removeEventListener('inyon:open-menu', listen))

  test('/resume opens past conversations instead of the terminal-only picker', async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={`%r${++n}`} agent="claude" working={false} />)
    await user.type(box(), '/resume')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    expect(opened).toEqual(['conversations'])
    expect(api.send.mock.calls).toEqual([])
    expect(box().value).toBe('')
  })

  test('picking /resume from the command list does the same', async () => {
    const user = userEvent.setup()
    api.commands.mockImplementation(async () => [{ name: '/resume', description: 'Resume a previous conversation', source: 'built-in' as const, instant: true }])
    await render(<Composer host="box" pane={`%r${++n}`} agent="claude" working={false} />)
    await user.type(box(), '/resu')
    await flush()
    await user.keyboard('{Enter}')
    await flush()
    expect(opened).toEqual(['conversations'])
    expect(api.send.mock.calls).toEqual([])
  })

  test("Copilot's /switch opens it too", async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={`%r${++n}`} agent="copilot" working={false} />)
    await user.type(box(), '/switch')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    expect(opened).toEqual(['conversations'])
    expect(api.send.mock.calls).toEqual([])
  })

  test("/resume with an id, and a command that isn't the agent's picker, are sent as typed", async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={`%r${++n}`} agent="claude" working={false} />)
    await user.type(box(), '/resume 1234')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    await user.type(box(), '/switch')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    expect(api.send.mock.calls.map((c) => c[2])).toEqual(['/resume 1234', '/switch'])
    expect(opened).toEqual([])
  })

  test('for an agent Inyon cannot list, /resume goes to the agent', async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={`%r${++n}`} agent="gemini" working={false} />)
    await user.type(box(), '/resume')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    expect(api.send.mock.calls.map((c) => c[2])).toEqual(['/resume'])
    expect(opened).toEqual([])
  })

  test('with no sheet to open, it goes to the agent as before', async () => {
    window.removeEventListener('inyon:open-menu', listen)
    const user = userEvent.setup()
    await render(<Composer host="box" pane={`%r${++n}`} agent="claude" working={false} />)
    await user.type(box(), '/resume')
    await user.keyboard('{Escape}{Enter}')
    await flush()
    expect(api.send.mock.calls.map((c) => c[2])).toEqual(['/resume'])
  })
})

describe('the session header', () => {
  const session = (kind: 'claude' | 'gemini') => {
    const h = host('box', { panes: [agentPane('%4', kind)] })
    return { ref: allPanes([h])[0], def: AGENTS[kind], agent: kind, info: undefined, screen: undefined, status: undefined, actions: {} } as unknown as SessionModel
  }
  const header = (kind: 'claude' | 'gemini') => (
    <MemoryRouter>
      <SessionHeader s={session(kind)} view="chat" setView={() => {}} panelOpen={false} togglePanel={() => {}} openSheet={() => {}} />
    </MemoryRouter>
  )

  test('opens past conversations from its menu, and when /resume asks for them', async () => {
    answer()
    const user = userEvent.setup()
    await render(header('claude'))
    await user.click(screen.getAllByRole('button', { name: 'More' }).at(-1)!)
    await user.click(screen.getByRole('menuitem', { name: /Past conversations/ }))
    await flush()
    expect(screen.getByText('Write the README')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'Close' }))
    await act(() => Bun.sleep(250))
    expect(screen.queryByText('Write the README')).toBeNull()
    // what the composer does for a bare /resume
    expect(openMenu('conversations')).toBe(true)
    await act(() => Bun.sleep(10))
    expect(screen.getByText('Write the README')).toBeTruthy()
  })

  test("isn't offered for an agent Inyon can't list", async () => {
    const user = userEvent.setup()
    await render(header('gemini'))
    await user.click(screen.getAllByRole('button', { name: 'More' }).at(-1)!)
    expect(screen.queryByRole('menuitem', { name: /Past conversations/ })).toBeNull()
    expect(openMenu('conversations')).toBe(false)
  })
})
