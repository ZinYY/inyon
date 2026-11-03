import { beforeEach, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { VERSION } from '@inyon/protocol'
import { UpdatePrompt } from './UpdatePrompt.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'
import { forgetHub } from '../lib/updates.ts'
import { api, setSocketUp } from '../test/fakes.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

/** The hub restarts: the socket drops, and comes back to a hub running `version`. */
async function hubRestarts(version: string, commit = 'abc1234') {
  api.session.mockResolvedValue({ authed: true, version, commit })
  act(() => setSocketUp(false))
  act(() => setSocketUp(true))
  await flush()
}
const prompt = () => screen.queryByText('Inyon was updated')

beforeEach(() => {
  forgetHub()
  api.session.mockResolvedValue({ authed: true, version: VERSION, commit: 'abc1234' })
})

test('no prompt while the hub runs the same version as this page', async () => {
  await render(<UpdatePrompt />)
  await hubRestarts(VERSION)
  expect(api.session).toHaveBeenCalled()
  expect(prompt()).toBeNull()
})

test('a hub that comes back with a new version offers a reload, once', async () => {
  const user = userEvent.setup()
  await render(<UpdatePrompt />)
  await hubRestarts('99.0.0')
  expect(prompt()).toBeTruthy()
  expect(screen.getByRole('button', { name: 'Reload' })).toBeTruthy()
  expect(screen.getByRole('status').className).toContain('animate-toast-in')
  await user.click(screen.getByRole('button', { name: 'Not now' }))
  // it fades out first, then goes
  expect(screen.getByRole('status').className).toContain('animate-fade-out')
  await act(() => new Promise((r) => setTimeout(r, LEAVE_MS + 20)))
  expect(prompt()).toBeNull()
  // the same new version again, even after a page load: no nagging
  await hubRestarts('99.0.0')
  expect(prompt()).toBeNull()
  forgetHub()
  await hubRestarts('99.0.0')
  expect(prompt()).toBeNull()
  // the next one is offered again
  await hubRestarts('99.0.1')
  expect(prompt()).toBeTruthy()
})

test('the socket being up already is enough to check', async () => {
  api.session.mockResolvedValue({ authed: true, version: '99.0.0', commit: 'abc1234' })
  act(() => setSocketUp(true))
  await render(<UpdatePrompt />)
  await flush()
  expect(prompt()).toBeTruthy()
})

test('builds match on version, and on commit when both know it', async () => {
  const { sameBuild } = await import('../lib/updates.ts')
  expect(sameBuild({ version: '0.6.3', commit: 'abc1234' }, { version: '0.6.3', commit: 'abc1234ef' })).toBe(true)
  expect(sameBuild({ version: '0.6.3', commit: 'abc1234' }, { version: '0.6.3', commit: 'def5678' })).toBe(false)
  expect(sameBuild({ version: '0.6.3' }, { version: '0.6.3', commit: 'def5678' })).toBe(true)
  expect(sameBuild({ version: '0.6.3' }, { version: '0.6.4' })).toBe(false)
})
