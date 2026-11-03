import { describe, expect, mock, test } from 'bun:test'
import { fireEvent, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { AGENTS, CONFIRM_OPTION, promptKey, type ScreenPrompt } from '@inyon/protocol'
import { ComposerControls } from './ComposerControls.tsx'
import { ControlsSheet } from './MobileSheets.tsx'
import { PromptCard } from '../chat/PromptCard.tsx'
import type { SessionModel } from './useSession.ts'
import { api } from '../../test/fakes.ts'
import { agentPane, chatInfo, host } from '../../test/fixtures.ts'
import { flush, renderSettled as render } from '../../test/render.tsx'

const actions = { setModel: mock((_id: string) => {}), setEffort: mock((_l: string) => {}), setMode: mock((_m: string) => {}), redo: mock(() => {}), share: mock((_on: boolean) => {}), rewind() {}, compact() {}, clear() {}, copyId() {}, exit() {}, end() {}, fork() {}, canFork: false }
const box = host('box', { panes: [agentPane('%1', 'opencode')] })
const ref = { host: box, session: box.sessions[0], window: box.sessions[0].windows[0], pane: box.sessions[0].windows[0].panes[0] }

/** An OpenCode session as useSession hands it over, with the machine's own model and agent lists. */
const session = (more: Partial<SessionModel> = {}) =>
  ({
    agent: 'opencode',
    def: { ...AGENTS.opencode, models: [{ id: 'opencode/big-pickle', label: 'OpenCode · Big Pickle', efforts: [] }, { id: 'opencode/space-bunny-free', label: 'OpenCode · Space Bunny Free', efforts: ['low', 'high'] }], modes: [{ id: 'build', label: 'Build' }, { id: 'plan', label: 'Plan' }, { id: 'reviewer', label: 'Reviewer' }] },
    info: chatInfo(),
    status: { model: 'Space Bunny Free', modelId: 'opencode/space-bunny-free', updatedAt: 1 },
    efforts: ['low', 'high'],
    ref,
    actions,
    ...more,
  }) as unknown as SessionModel

describe('OpenCode in the composer controls', () => {
  test('the model menu ends with Connect a provider…, which opens the providers for the session\'s machine', async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue({ updatedAt: 1, providers: [] })
    await render(<ComposerControls s={session()} />)
    await user.click(screen.getByRole('button', { name: 'Model' }))
    const menu = screen.getByRole('menu')
    expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['OpenCode · Big Pickle', 'OpenCode · Space Bunny Free', 'Connect a provider…'])
    await user.click(within(menu).getByRole('menuitem', { name: 'Connect a provider…' }))
    await flush()
    expect(api.providers.mock.calls).toEqual([['box', '%1']])
    expect(screen.getByRole('dialog', { name: 'OpenCode providers' })).toBeTruthy()
  })

  test('with no model listed yet the menu still offers to connect a provider, and "/connect" opens it', async () => {
    api.providers.mockResolvedValue({ updatedAt: 1, providers: [] })
    await render(<ComposerControls s={session({ def: { ...AGENTS.opencode, models: [] } as any, efforts: [] })} />)
    expect(screen.getByRole('button', { name: 'Model' })).toBeTruthy()
    const handled = !window.dispatchEvent(new CustomEvent('inyon:open-menu', { detail: 'providers', cancelable: true }))
    expect(handled).toBe(true)
    // it opens on the next tick, after the key that asked for it
    expect(await screen.findByRole('dialog', { name: 'OpenCode providers' })).toBeTruthy()
  })

  test("the agent menu lists the machine's own agents, custom ones too", async () => {
    const user = userEvent.setup()
    await render(<ComposerControls s={session()} />)
    await user.click(screen.getByRole('button', { name: /^Agent:/ }))
    const items = within(screen.getByRole('menu')).getAllByRole('menuitem').map((m) => m.textContent)
    expect(items).toEqual(['Build', 'Plan', 'Reviewer'])
    await user.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: 'Reviewer' }))
    expect(actions.setMode.mock.calls.at(-1)).toEqual(['reviewer'])
  })

  test('the effort menu starts with Default, which is what no variant means', async () => {
    const user = userEvent.setup()
    await render(<ComposerControls s={session()} />)
    await user.click(screen.getByRole('button', { name: 'Reasoning effort' }))
    const items = within(screen.getByRole('menu')).getAllByRole('menuitem')
    expect(items.map((m) => m.textContent)).toEqual(['Default', 'Low', 'High'])
    await user.click(items[0])
    expect(actions.setEffort.mock.calls.at(-1)).toEqual(['default'])
  })
})

describe('OpenCode in the phone controls sheet', () => {
  test('Default is a choice next to the variants, and Redo and Share are there', async () => {
    const user = userEvent.setup()
    await render(<MemoryRouter><ControlsSheet s={session()} open onClose={() => {}} /></MemoryRouter>)
    const group = screen.getByRole('radiogroup')
    expect(within(group).getAllByRole('radio').map((r) => [r.textContent, r.getAttribute('aria-checked')])).toEqual([['Default', 'true'], ['Low', 'false'], ['High', 'false']])
    await user.click(within(group).getByRole('radio', { name: 'High' }))
    expect(actions.setEffort.mock.calls.at(-1)).toEqual(['high'])
    await user.click(screen.getByRole('button', { name: 'Redo' }))
    expect(actions.redo).toHaveBeenCalledTimes(1)
    await user.click(screen.getByRole('button', { name: 'Share' }))
    expect(actions.share.mock.calls.at(-1)).toEqual([true])
    expect(screen.getByRole('button', { name: 'Connect a provider…' })).toBeTruthy()
  })

  test('a shared conversation offers Unshare', async () => {
    await render(<MemoryRouter><ControlsSheet s={session({ info: chatInfo({ share: 'https://opencode.ai/s/abc' }) })} open onClose={() => {}} /></MemoryRouter>)
    expect(screen.getByRole('button', { name: 'Unshare' })).toBeTruthy()
  })
})

describe("OpenCode's questions with several answers", () => {
  const prompt: ScreenPrompt = {
    body: ['Which colours do you like?'],
    options: [
      { n: 1, label: 'Red', selected: true, checked: false },
      { n: 2, label: 'Green', selected: false, checked: true },
      { n: 3, label: 'Type your own answer', selected: false, checked: false },
    ],
    multi: true,
    confirm: 'Confirm',
    freeTextOption: 3,
    tabs: [{ label: 'Colours', done: false, current: true }, { label: 'Confirm', done: false, current: false }],
  }

  test('a tap toggles a choice, Confirm sends them all, and no raw arrow keys are offered', async () => {
    const { rerender } = await render(<PromptCard prompt={prompt} host="box" pane="%1" />)
    expect(screen.queryByRole('button', { name: 'Up' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Next' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /Red/ }))
    expect(api.answer.mock.calls.at(-1)).toEqual(['box', '%1', 1, undefined, promptKey(prompt)])
    // the hub shows the tick, which lets the card take the next tap
    const ticked = { ...prompt, options: prompt.options.map((o) => (o.n === 1 ? { ...o, checked: true } : o)) }
    rerender(<PromptCard prompt={ticked} host="box" pane="%1" />)
    await flush()
    fireEvent.click(screen.getByRole('button', { name: /^Confirm$/ }))
    expect(api.answer.mock.calls.at(-1)).toEqual(['box', '%1', CONFIRM_OPTION, undefined, promptKey(ticked)])
  })

  test('the review step shows what was answered and a Submit button', async () => {
    const review: ScreenPrompt = { body: ['Review', 'Colours: Green, Blue'], options: [], multi: false, confirm: 'Submit', tabs: [{ label: 'Colours', done: true, current: false }, { label: 'Confirm', done: false, current: true }] }
    await render(<PromptCard prompt={review} host="box" pane="%1" />)
    expect(screen.getByText('Colours: Green, Blue')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /^Submit$/ }))
    expect(api.answer.mock.calls.at(-1)).toEqual(['box', '%1', CONFIRM_OPTION, undefined, promptKey(review)])
  })
})
