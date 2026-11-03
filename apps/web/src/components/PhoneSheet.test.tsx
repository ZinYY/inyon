import { beforeEach, expect, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { PhoneSheet } from './PhoneSheet.tsx'
import { api } from '../test/fakes.ts'
import { flush, renderSettled as render } from '../test/render.tsx'
import { ApiError } from '../lib/api.ts'

const code = { id: 'K7Q4MX2T', code: '493812', link: 'inyon://hub?u=http%3A%2F%2Flocalhost%3A4777&c=K7Q4MX2T%3A493812&n=studio', expiresAt: Date.now() + 600_000 }

beforeEach(() => {
  api.phonePair.mockImplementation(async () => code)
})

test('shows the code as the app expects it, with the hub address', async () => {
  await render(<PhoneSheet open onClose={() => {}} />)
  await flush()
  expect(screen.getByText('K7Q4-MX2T')).toBeTruthy()
  expect(screen.getByText('493 812')).toBeTruthy()
  expect(screen.getByRole('img', { name: 'Pairing code' }).innerHTML).toContain('<svg')
  expect(api.phonePair).toHaveBeenCalledWith(location.origin)
})

test("shows the hub's ID and certificate the phone will show before it pairs", async () => {
  api.phonePair.mockImplementation(async () => ({ ...code, link: `${code.link}&h=3f9c1a7e2b6d4c80&f=${'cd'.repeat(32)}` }))
  await render(<PhoneSheet open onClose={() => {}} />)
  await flush()
  expect(screen.getByText('3f9c 1a7e 2b6d 4c80')).toBeTruthy()
  expect(screen.getByText('cdcd cdcd cdcd cdcd')).toBeTruthy()
})

test('an old sign-in asks for the password, then shows the code', async () => {
  api.phonePair.mockImplementationOnce(async () => {
    throw new ApiError(403, 'Sign in again to pair a phone.')
  })
  api.login.mockImplementation(async () => ({ ok: true }))
  await render(<PhoneSheet open onClose={() => {}} />)
  await flush()
  expect(screen.queryByText('K7Q4-MX2T')).toBeNull()
  await userEvent.type(screen.getByLabelText('Password'), 'hunter22')
  await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
  await flush()
  expect(api.login).toHaveBeenCalledWith('hunter22')
  expect(screen.getByText('K7Q4-MX2T')).toBeTruthy()
})
