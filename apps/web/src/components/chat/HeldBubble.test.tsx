import { expect, test } from 'bun:test'
import { act, fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { HeldBubble } from './HeldBubble.tsx'
import { Composer } from './Composer.tsx'
import { useStore } from '../../lib/store.ts'
import { api } from '../../test/fakes.ts'
import { flush, renderSettled as render } from '../../test/render.tsx'

const held = { id: 'held-1', text: 'Then run the tests', at: 0 }

test('a held message says when it goes, and sends now or drops with Undo', async () => {
  await render(<HeldBubble host="box" pane="%4" p={held} index={0} count={2} agentName="Gemini CLI" />)
  expect(screen.getByText('Held · goes when Gemini CLI is free')).toBeTruthy()
  fireEvent.click(screen.getByLabelText('Send now'))
  await flush()
  expect(api.sendHeld.mock.calls).toEqual([['box', '%4', 'held-1']])
  fireEvent.click(screen.getByLabelText('Drop'))
  await flush()
  expect(api.dropHeld.mock.calls).toEqual([['box', '%4', 'held-1']])
  const t = useStore.getState().toasts.at(-1)!
  expect(t.title).toBe('Message dropped')
  api.hold.mockImplementation(async () => ({ id: 'held-2', text: held.text, at: 1 }))
  act(() => t.action!.onSelect())
  await flush()
  expect(api.hold.mock.calls.at(-1)).toEqual(['box', '%4', 'Then run the tests'])
  expect(api.editHeld.mock.calls.at(-1)).toEqual(['box', '%4', 'held-2', { to: 0 }])
})

test('a held message that failed shows why, and the second in line its place', async () => {
  await render(<HeldBubble host="box" pane="%4" p={{ ...held, error: "The message didn't show in Gemini CLI's input box" }} index={0} count={1} agentName="Gemini CLI" />)
  expect(screen.getByText(/didn't show in Gemini CLI's input box/)).toBeTruthy()
  await render(<HeldBubble host="box" pane="%4" p={{ ...held, id: 'held-3' }} index={1} count={3} agentName="Gemini CLI" />)
  expect(screen.getByText('Held · 2 of 3')).toBeTruthy()
})

test('Alt+Enter holds what you typed and empties the box', async () => {
  const user = userEvent.setup()
  await render(<Composer host="box" pane="%77" agent="gemini" working />)
  const box = screen.getByRole('textbox') as HTMLTextAreaElement
  await user.type(box, 'Write the migration')
  await user.keyboard('{Alt>}{Enter}{/Alt}')
  await flush()
  expect(api.hold.mock.calls.at(-1)).toEqual(['box', '%77', 'Write the migration'])
  expect(api.send.mock.calls).toEqual([])
  expect(box.value).toBe('')
})
