import { expect, mock, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { AGENTS } from '@inyon/protocol'
import { ComposerControls } from './ComposerControls.tsx'
import { ControlsSheet } from './MobileSheets.tsx'
import type { SessionModel } from './useSession.ts'
import { chatInfo } from '../../test/fixtures.ts'
import { renderSettled as render } from '../../test/render.tsx'

const ultracode = mock((_on: boolean) => {})
const nothing = () => {}
/** a Claude session as useSession gives it to the controls, with the switch on or off */
const session = (on: boolean) =>
  ({
    agent: 'claude',
    def: AGENTS.claude,
    info: chatInfo(),
    status: { effort: 'high', updatedAt: 0 },
    efforts: AGENTS.claude.efforts,
    ultracode: on,
    actions: { ultracode, setModel: nothing, setEffort: nothing, setMode: nothing, setGoal: nothing, clearGoal: nothing, rc: nothing, compact: nothing, rewind: nothing, clear: nothing, exit: nothing, end: nothing, copyId: nothing, fork: nothing, canFork: false },
  }) as unknown as SessionModel

test('the effort menu ends with the ultracode switch, and the chip marks it while on', async () => {
  const user = userEvent.setup()
  const { rerender } = await render(<ComposerControls s={session(false)} />)
  await user.click(screen.getByTitle('Reasoning effort: High'))
  await user.click(screen.getByText('Ultracode: off'))
  expect(ultracode).toHaveBeenLastCalledWith(true)
  rerender(<ComposerControls s={session(true)} />)
  expect(screen.getByTitle('Reasoning effort: High · Ultracode on')).toBeTruthy()
  expect(screen.getByLabelText('Ultracode on')).toBeTruthy()
})

test("the phone's controls sheet has the switch, and says what it does while on", async () => {
  const user = userEvent.setup()
  await render(
    <MemoryRouter>
      <ControlsSheet s={session(true)} open onClose={nothing} />
    </MemoryRouter>,
  )
  expect(screen.getByText('On · a dynamic workflow for every task')).toBeTruthy()
  await user.click(screen.getByRole('switch', { name: 'Ultracode' }))
  expect(ultracode).toHaveBeenLastCalledWith(false)
})
