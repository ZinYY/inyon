import { describe, expect, mock, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { AGENTS, type ChatInfo } from '@inyon/protocol'
import { ComposerControls } from './ComposerControls.tsx'
import type { SessionModel } from './useSession.ts'
import { chatInfo } from '../../test/fixtures.ts'
import { flush, renderSettled as render } from '../../test/render.tsx'

const actions = { setGoal: mock((_t: string) => {}), clearGoal: mock(() => {}), setModel() {}, setEffort() {}, setMode() {} }

/** What useSession gives the controls; rebuilt on every refresh, as useSession does. */
const session = (goal?: ChatInfo['goal']) =>
  ({ agent: 'claude', def: AGENTS.claude, info: chatInfo({ goal }), efforts: [], actions }) as unknown as SessionModel

const active = { condition: 'tests pass', met: false, ts: '' }
const box = () => screen.getByPlaceholderText('All tests pass and the PR is open') as HTMLTextAreaElement

describe('GoalSheet keeps what you type while live data flows', () => {
  test('typing survives refreshes, even one that changes the goal under it', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ComposerControls s={session(active)} />)
    await user.click(screen.getByTitle('Goal: tests pass'))
    expect(box().value).toBe('tests pass')
    await user.clear(box())
    await user.type(box(), 'lint')
    rerender(<ComposerControls s={session(active)} />)
    rerender(<ComposerControls s={session({ ...active, met: true, reason: 'all green' })} />)
    await user.type(box(), ' is clean')
    expect(box().value).toBe('lint is clean')
  })

  test('reopening shows the current goal', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ComposerControls s={session(active)} />)
    await user.click(screen.getByTitle('Goal: tests pass'))
    await user.type(box(), ' and more')
    await user.click(screen.getByText('Cancel'))
    rerender(<ComposerControls s={session({ ...active, condition: 'docs updated' })} />)
    await user.click(screen.getByTitle('Goal: docs updated'))
    await flush()
    expect(box().value).toBe('docs updated')
  })

  test('set and turn off', async () => {
    const user = userEvent.setup()
    actions.setGoal.mockClear()
    const { rerender } = await render(<ComposerControls s={session()} />)
    await user.click(screen.getByTitle('Set a goal'))
    expect(box().value).toBe('')
    await user.type(box(), '  ship it  ')
    await user.click(screen.getByText('Set goal'))
    expect(actions.setGoal.mock.calls).toEqual([['ship it']])

    rerender(<ComposerControls s={session(active)} />)
    await user.click(screen.getByTitle('Goal: tests pass'))
    await user.click(screen.getByText('Turn off'))
    expect(actions.clearGoal).toHaveBeenCalledTimes(1)
  })
})

describe('a pick made while the agent works waits, and says so', () => {
  const picked = (chosen: ChatInfo['chosen']) =>
    ({ agent: 'claude', def: AGENTS.claude, info: chatInfo({ chosen }), status: { effort: 'high' }, efforts: AGENTS.claude.efforts, actions: { ...actions, dropChosen } }) as unknown as SessionModel
  const dropChosen = mock(() => {})
  const model = AGENTS.claude.models[0]!

  test('the chip shows what was picked, and the menu can keep the current one', async () => {
    const user = userEvent.setup()
    dropChosen.mockClear()
    await render(<ComposerControls s={picked({ model: model.id, effort: 'low' })} />)
    const chip = screen.getByTitle(`Switches to ${model.label.split(' · ').pop()} when Claude Code is free`)
    expect(chip.textContent).toContain(model.label.split(' · ').pop()!)
    expect(screen.getByTitle('Switches to Low when Claude Code is free').textContent).toContain('Low')
    await user.click(chip)
    await user.click(screen.getByText("Don't switch"))
    expect(dropChosen).toHaveBeenCalledTimes(1)
  })

  test("one that didn't take shows why, and picking that row dismisses it", async () => {
    const user = userEvent.setup()
    dropChosen.mockClear()
    await render(<ComposerControls s={picked({ mode: 'plan', error: 'Answer the open question first' })} />)
    const chip = screen.getByTitle("Couldn't switch to Plan: Answer the open question first")
    await user.click(chip)
    await user.click(screen.getByText("Couldn't switch to Plan"))
    expect(dropChosen).toHaveBeenCalledTimes(1)
  })
})
