import { afterEach, describe, expect, setSystemTime, test } from 'bun:test'
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import type { HostState, Organize, SessionMark } from '@inyon/protocol'
import { Home } from '../pages/Home.tsx'
import { ActionSheets } from './menus.tsx'
import { SendLaterSheet } from './session/OrganizeSheets.tsx'
import { ScheduledBubble } from './chat/ScheduledBubble.tsx'
import { useStore } from '../lib/store.ts'
import { api, emit } from '../test/fakes.ts'
import { agentPane, host } from '../test/fixtures.ts'
import { act } from '@testing-library/react'
import { renderSettled as render } from '../test/render.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'

/** One machine with three agent sessions, one window each. */
const box = (): HostState =>
  host('box', {
    panes: [agentPane('%1', 'claude', { title: 'Fix the login test' }), agentPane('%2', 'codex', { title: 'Tidy the CSS' }), agentPane('%3', 'claude', { title: 'Write the docs' })],
  })
const mark = (pane: string, more: Partial<SessionMark> = {}): SessionMark => ({ id: `s-${pane}`, host: 'box', pane, live: true, seen: 0, ...more })
const org = (...sessions: SessionMark[]): Organize => ({ v: 1, sessions, projects: [], groups: [], spaces: [] })

/** The session titles in the order the overview shows them, the folded ones left out. */
const TITLES = ['Fix the login test', 'Tidy the CSS', 'Write the docs']
const shown = () =>
  screen
    .getAllByRole('link')
    .filter((l) => TITLES.includes(l.getAttribute('aria-label') ?? '') && !l.closest('[inert]'))
    .map((l) => l.getAttribute('aria-label'))
/** A message from the hub once the screen is up. */
const hear = (m: Parameters<typeof emit>[0]) => act(async () => emit(m))

// the hub's organize outlives the store's reset between tests: an overview without one clears it
afterEach(() => {
  emit({ t: 'overview', hosts: [] })
  setSystemTime()
})

const home = () => render(<MemoryRouter><Home /><ActionSheets /></MemoryRouter>)

describe('organising sessions on the overview', () => {
  test('the pinned come first; parked and snoozed wait folded until opened', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org(mark('%3', { pinned: 0 }), mark('%2', { parked: true })) })
    await home()
    expect(screen.getByText('Pinned')).toBeTruthy()
    expect(shown()).toEqual(['Write the docs', 'Fix the login test'])
    const fold = screen.getByRole('button', { name: /Parked and snoozed/ })
    expect(fold.getAttribute('aria-expanded')).toBe('false')
    await userEvent.setup().click(fold)
    expect(shown()).toEqual(['Write the docs', 'Fix the login test', 'Tidy the CSS'])
  })

  test('an older hub sends no organize: everything shows as before', async () => {
    emit({ t: 'overview', hosts: [box()] })
    await home()
    expect(screen.queryByText('Pinned')).toBeNull()
    expect(screen.queryByRole('button', { name: /Parked and snoozed/ })).toBeNull()
    expect(shown()).toEqual(['Fix the login test', 'Tidy the CSS', 'Write the docs'])
  })

  test('Pin and Park go to the hub, which tells every device', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Tidy the CSS' }))
    await user.click(screen.getByRole('menuitem', { name: 'Pin' }))
    expect(api.organize).toHaveBeenCalledWith('box', '%2', { pinned: true })

    // the hub's answer is the next overview; the menu then offers to undo it
    await hear({ t: 'overview', hosts: [box()], organize: org(mark('%2', { pinned: 0 })) })
    // it moves up into Pinned; its old card fades out where it was
    await act(() => new Promise((r) => setTimeout(r, LEAVE_MS + 50)))
    await user.click(screen.getByRole('button', { name: 'Actions for Tidy the CSS' }))
    expect(screen.getByRole('menuitem', { name: 'Unpin' })).toBeTruthy()
    api.organize.mockResolvedValueOnce({ session: mark('%2', { pinned: 0, parked: true }) })
    await user.click(screen.getByRole('menuitem', { name: 'Park' }))
    expect(api.organize).toHaveBeenLastCalledWith('box', '%2', { parked: true })
    const undo = useStore.getState().toasts.at(-1)!.action!
    expect(undo.label).toBe('Undo')
    await hear({ t: 'overview', hosts: [box()], organize: org(mark('%2', { pinned: 0, parked: true })) })
    await act(async () => undo.onSelect())
    expect(api.organize).toHaveBeenLastCalledWith('box', '%2', { parked: null })
  })

  test("Undo leaves alone another session that has come to the parked one's pane", async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    api.organize.mockResolvedValueOnce({ session: mark('%2', { parked: true }) })
    await user.click(screen.getByRole('button', { name: 'Actions for Tidy the CSS' }))
    await user.click(screen.getByRole('menuitem', { name: 'Park' }))
    const undo = useStore.getState().toasts.at(-1)!.action!
    // the pane closed and tmux gave its id to a session that was parked too
    await hear({ t: 'overview', hosts: [box()], organize: org({ ...mark('%2', { parked: true }), id: 's-other' }) })
    const calls = api.organize.mock.calls.length
    await act(async () => undo.onSelect())
    expect(api.organize.mock.calls.length).toBe(calls)
    expect(useStore.getState().toasts.at(-1)?.title).toBe('Could not undo')
  })

  test('a card shows the title given in Inyon, and says Parked when that is all it is', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org(mark('%1', { title: 'Login bug' }), mark('%2', { parked: true })) })
    await home()
    expect(screen.getByRole('link', { name: 'Login bug' })).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: /Parked and snoozed/ }))
    const card = screen.getByRole('link', { name: 'Tidy the CSS' }).parentElement!
    expect(within(card).getByText('Parked')).toBeTruthy()
  })

  test('Show opens the parked and snoozed to reach a session there that needs you', async () => {
    const waiting = host('box', { panes: [agentPane('%1', 'claude', { title: 'Fix the login test' }), agentPane('%2', 'codex', { title: 'Tidy the CSS', agent: { kind: 'codex', pid: 1, state: 'waiting' } })] })
    emit({ t: 'overview', hosts: [waiting], organize: org(mark('%2', { parked: true })) })
    await home()
    const fold = screen.getByRole('button', { name: /Parked and snoozed/ })
    expect(fold.getAttribute('aria-expanded')).toBe('false')
    await userEvent.setup().click(screen.getByRole('button', { name: 'Show' }))
    expect(fold.getAttribute('aria-expanded')).toBe('true')
  })

  test('tags and a note are saved from their sheets and shown on the card', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Fix the login test' }))
    await user.click(screen.getByRole('menuitem', { name: 'Tags…' }))
    const sheet = screen.getByRole('dialog')
    await user.type(within(sheet).getByRole('textbox'), 'Review, #auth')
    await user.click(within(sheet).getByRole('button', { name: 'Save' }))
    expect(api.organize).toHaveBeenCalledWith('box', '%1', { tags: ['review', 'auth'] })

    await hear({ t: 'overview', hosts: [box()], organize: org(mark('%1', { tags: ['review', 'auth'], note: 'waiting on the API' })) })
    expect(screen.getByText(/#review #auth/)).toBeTruthy()
    expect(screen.getByLabelText('Note: waiting on the API')).toBeTruthy()
  })

  test('a snooze is picked from the times on this device', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    await user.click(screen.getByRole('menuitem', { name: 'Snooze…' }))
    const sheet = screen.getByRole('dialog')
    await user.click(within(sheet).getByRole('switch', { name: "Tell me when it's back" }))
    const before = Date.now()
    await user.click(within(sheet).getByRole('button', { name: /For an hour/ }))
    const [, , patch] = api.organize.mock.calls.at(-1)!
    expect(patch.snoozeNotice).toBe(true)
    expect(patch.snoozedUntil! - before).toBeGreaterThanOrEqual(3600_000 - 5_000)
    expect(patch.snoozedUntil! - before).toBeLessThanOrEqual(3600_000 + 5_000)
  })

  test('the snooze time is worked out on the tap, not when the sheet opened', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    await user.click(screen.getByRole('menuitem', { name: 'Snooze…' }))
    // the sheet stays open for two hours before the tap
    const later = Date.now() + 2 * 3600_000
    setSystemTime(new Date(later))
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: /For an hour/ }))
    const [, , patch] = api.organize.mock.calls.at(-1)!
    expect(patch.snoozedUntil! - later).toBeGreaterThanOrEqual(3600_000 - 5_000)
  })

  test('asking for a notice while finished notices are off says how to get it', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    useStore.setState((s) => ({ settings: { ...s.settings, notifyDone: false } }))
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    await user.click(screen.getByRole('menuitem', { name: 'Snooze…' }))
    const sheet = screen.getByRole('dialog')
    expect(within(sheet).queryByText(/When an agent finishes/)).toBeNull()
    await user.click(within(sheet).getByRole('switch', { name: "Tell me when it's back" }))
    expect(within(sheet).getByText(/When an agent finishes/)).toBeTruthy()
  })

  test('the store keeps the same organize until it changes, so cards render again only then', () => {
    const o = org(mark('%1', { pinned: 0 }))
    emit({ t: 'overview', hosts: [box()], organize: o })
    const first = useStore.getState().organize
    emit({ t: 'overview', hosts: [box()], organize: structuredClone(o) })
    expect(useStore.getState().organize).toBe(first)
    emit({ t: 'overview', hosts: [box()], organize: org() })
    expect(useStore.getState().organize.sessions).toEqual([])
  })

  test('Close when done goes to the hub, and the menu then offers to keep it open', async () => {
    emit({ t: 'overview', hosts: [box()], organize: { ...org(), v: 2 } })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    await user.click(screen.getByRole('menuitem', { name: 'Close when done' }))
    expect(api.organize).toHaveBeenCalledWith('box', '%3', { closeWhenDone: true })
    await hear({ t: 'overview', hosts: [box()], organize: { ...org(mark('%3', { closeWhenDone: true })), v: 2 } })
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    expect(screen.getByRole('menuitem', { name: 'Keep open when done' })).toBeTruthy()
  })

  test('a hub that keeps only pins and parking offers no Close when done', async () => {
    emit({ t: 'overview', hosts: [box()], organize: org() })
    const user = userEvent.setup()
    await home()
    await user.click(screen.getByRole('button', { name: 'Actions for Write the docs' }))
    expect(screen.getByRole('menuitem', { name: 'Pin' })).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Close when done' })).toBeNull()
  })

  test('a message kept to send later is handed to the hub with its time, and the box empties', async () => {
    let sent = 0
    api.sendLater.mockImplementation(async () => ({ id: 'd-1', host: 'box', pane: '%1', text: 'run the tests', at: 0 }))
    const user = userEvent.setup()
    await render(<SendLaterSheet host="box" pane="%1" text="run the tests " open onClose={() => {}} onSent={() => sent++} />)
    const before = Date.now()
    await user.click(screen.getByRole('button', { name: /In an hour/ }))
    const [host, pane, text, at] = api.sendLater.mock.calls[0]!
    expect([host, pane, text]).toEqual(['box', '%1', 'run the tests'])
    expect(Math.abs(at - before - 3600_000)).toBeLessThan(5_000)
    expect(sent).toBe(1)
    expect(useStore.getState().toasts.at(-1)?.title).toMatch(/^Sends at /)
  })

  test('the time to send at is worked out on the tap, not when the sheet opened', async () => {
    api.sendLater.mockImplementation(async () => ({ id: 'd-1', host: 'box', pane: '%1', text: 'go on', at: 0 }))
    await render(<SendLaterSheet host="box" pane="%1" text="go on" open onClose={() => {}} onSent={() => {}} />)
    const later = Date.now() + 2 * 3600_000
    setSystemTime(new Date(later))
    await userEvent.setup().click(screen.getByRole('button', { name: /In an hour/ }))
    const [, , , at] = api.sendLater.mock.calls.at(-1)!
    expect(Math.abs(at - later - 3600_000)).toBeLessThan(5_000)
  })

  test('a scheduled message can be cancelled, and one that failed says why', async () => {
    const user = userEvent.setup()
    const s = { id: 'd-1', host: 'box', pane: '%1', text: 'run the tests', at: Date.now() + 3600_000 }
    const view = await render(<ScheduledBubble host="box" pane="%1" s={s} />)
    expect(screen.getByText(/^Sends at /)).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(api.cancelSend).toHaveBeenCalledWith('box', 'd-1')
    view.unmount()
    // Undo puts it back for the conversation it was for
    const withConv = await render(<ScheduledBubble host="box" pane="%1" s={{ ...s, conv: 'conv-a' }} />)
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    await act(async () => useStore.getState().toasts.at(-1)!.action!.onSelect())
    expect(api.sendLater).toHaveBeenLastCalledWith('box', '%1', 'run the tests', s.at, 'conv-a')
    withConv.unmount()
    await render(<ScheduledBubble host="box" pane="%1" s={{ ...s, error: "The session isn't there any more" }} />)
    expect(screen.getByText("The session isn't there any more")).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeTruthy()
  })
})

describe('a message waiting to go', () => {
  test('Send now is one request to the hub, not a hold and a cancel', async () => {
    const s = { id: 'd-1', host: 'box', pane: '%1', text: 'run the tests', at: Date.now() + 3600_000 }
    await render(<ScheduledBubble host="box" pane="%1" s={s} />)
    await userEvent.setup().click(screen.getByRole('button', { name: 'Send now' }))
    expect(api.sendNow).toHaveBeenCalledWith('box', 'd-1')
    expect(api.hold).not.toHaveBeenCalled()
    expect(api.cancelSend).not.toHaveBeenCalled()
  })
})
