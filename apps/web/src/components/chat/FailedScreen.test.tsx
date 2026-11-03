import { afterAll, expect, mock, spyOn, test } from 'bun:test'
import { screen } from '@testing-library/react'
import { FailedScreen } from './FailedScreen.tsx'
import { renderSettled as render } from '../../test/render.tsx'
import { api } from '../../lib/api.ts'

afterAll(() => mock.restore())

const failures = [
  { at: Date.now(), host: 'box', pane: '%4', agent: 'gemini', error: "The message didn't show in Gemini's input box, so Enter wasn't pressed.", chars: 27, screen: ['✦ Done.', '│ > Type your message │'] },
  { at: Date.now() - 60_000, host: 'box', pane: '%4', agent: 'gemini', error: 'an older one', chars: 3, screen: ['old'] },
]

test('a failed send shows the newest screen kept for its pane', async () => {
  spyOn(api, 'sendFailures').mockResolvedValue(failures)
  await render(<FailedScreen host="box" pane="%4" open onClose={() => {}} />)
  expect(await screen.findByText(/Enter wasn't pressed/)).toBeTruthy()
  expect(screen.getByText(/Type your message/)).toBeTruthy()
  expect(screen.queryByText('an older one')).toBeNull()
})

test("another pane's screens, or a hub that keeps none, show that nothing was kept", async () => {
  spyOn(api, 'sendFailures').mockResolvedValue(failures)
  const one = await render(<FailedScreen host="box" pane="%9" open onClose={() => {}} />)
  expect(await screen.findByText(/Nothing was kept/)).toBeTruthy()
  one.unmount()
  spyOn(api, 'sendFailures').mockRejectedValue(new Error('Not found'))
  await render(<FailedScreen host="box" pane="%4" open onClose={() => {}} />)
  expect(await screen.findByText(/Nothing was kept/)).toBeTruthy()
})
