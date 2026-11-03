import { beforeEach, expect, test } from 'bun:test'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { PairedCompanion } from '@inyon/protocol'
import { CompanionSheet, companionLink, mmss } from './CompanionSheet.tsx'
import { api } from '../test/fakes.ts'
import { flush, renderSettled as render } from '../test/render.tsx'
import { ApiError } from '../lib/api.ts'

const legato: PairedCompanion = { id: 'c1', name: 'Legato on test-laptop', app: 'Legato for Windows', scopes: ['overview', 'chat', 'answer', 'send'], contractVersion: 1, pairedAt: 1, lastSeen: 1 }

beforeEach(() => {
  api.companionPair.mockImplementation(async () => ({ code: 'K7M2QX', expiresAt: Date.now() + 300_000 }))
  api.companions.mockImplementation(async () => [])
})

test('shows the six characters in two halves, how long they work, and the hub address', async () => {
  await render(<CompanionSheet open onClose={() => {}} />)
  await flush()
  expect(screen.getByRole('img', { name: 'Pairing code K 7 M 2 Q X' })).toBeTruthy()
  expect(screen.getByText(/Works once · changes in [45]:\d\d/)).toBeTruthy()
  expect(screen.getByText(location.origin)).toBeTruthy()
  expect(screen.getByText('Waiting for the app to send the code…')).toBeTruthy()
})

test('says so once the app has claimed the code, with what it can and cannot do', async () => {
  await render(<CompanionSheet open onClose={() => {}} />)
  await flush()
  api.companions.mockImplementation(async () => [legato])
  await act(() => new Promise((r) => setTimeout(r, 2200)))
  await flush()
  expect(screen.getByText(/Legato on test-laptop is paired/)).toBeTruthy()
  expect(screen.getByText("Can't open terminals, files or settings")).toBeTruthy()
  expect(screen.queryByRole('button', { name: /New code/ })).toBeNull()
})

test('an app paired before the sheet opened is not taken for this one', async () => {
  api.companions.mockImplementation(async () => [legato])
  await render(<CompanionSheet open onClose={() => {}} />)
  await act(() => new Promise((r) => setTimeout(r, 2200)))
  await flush()
  expect(screen.queryByText(/is paired/)).toBeNull()
})

test('New code asks the hub for another', async () => {
  await render(<CompanionSheet open onClose={() => {}} />)
  await flush()
  api.companionPair.mockImplementation(async () => ({ code: 'R4TW8P', expiresAt: Date.now() + 300_000 }))
  await userEvent.click(screen.getByRole('button', { name: /New code/ }))
  await flush()
  expect(screen.getByRole('img', { name: 'Pairing code R 4 T W 8 P' })).toBeTruthy()
})

test('an old sign-in asks for the password, then shows the code', async () => {
  api.companionPair.mockImplementationOnce(async () => {
    throw new ApiError(403, 'Sign in again to pair a companion app.')
  })
  api.login.mockImplementation(async () => ({ ok: true }))
  await render(<CompanionSheet open onClose={() => {}} />)
  await flush()
  await userEvent.type(screen.getByLabelText('Password'), 'hunter22')
  await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
  await flush()
  expect(api.login).toHaveBeenCalledWith('hunter22')
  expect(screen.getByRole('img', { name: 'Pairing code K 7 M 2 Q X' })).toBeTruthy()
})

test('a renewal that fails stops asking and offers Try again', async () => {
  api.companionPair.mockImplementationOnce(async () => ({ code: 'K7M2QX', expiresAt: Date.now() + 500 }))
  api.companionPair.mockImplementation(async () => {
    throw new ApiError(500, 'The hub is restarting')
  })
  await render(<CompanionSheet open onClose={() => {}} />)
  await act(() => new Promise((r) => setTimeout(r, 3200)))
  await flush()
  expect(api.companionPair).toHaveBeenCalledTimes(2)
  expect(screen.getByText('The hub is restarting')).toBeTruthy()
  expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
})

test('the code is asked for only once the companions already paired are known', async () => {
  let listed!: (v: PairedCompanion[]) => void
  api.companions.mockImplementationOnce(() => new Promise((r) => (listed = r)))
  await render(<CompanionSheet open onClose={() => {}} />)
  expect(api.companionPair).not.toHaveBeenCalled()
  await act(async () => listed([legato]))
  await flush()
  expect(api.companionPair).toHaveBeenCalledTimes(1)
})

test('the pairing link holds the address, the code and the certificate', () => {
  expect(companionLink('https://100.97.188.112:4777', 'K7M2QX', 'ab'.repeat(32))).toBe(`inyon://companion?u=https%3A%2F%2F100.97.188.112%3A4777&c=K7M2QX&f=${'ab'.repeat(32)}`)
  // a hub without its own certificate (plain http on a VPN, or behind a proxy) leaves f out
  expect(companionLink('http://abhash-arm:4777', 'K7M2QX')).toBe('inyon://companion?u=http%3A%2F%2Fabhash-arm%3A4777&c=K7M2QX')
})

test('Copy pairing link copies the link for this code', async () => {
  api.companionPair.mockImplementation(async () => ({ code: 'K7M2QX', expiresAt: Date.now() + 300_000, tlsFingerprint: 'cd'.repeat(32) }))
  const copied: string[] = []
  Object.defineProperty(window, 'isSecureContext', { value: true, configurable: true })
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (t: string) => void copied.push(t) }, configurable: true })
  try {
    await render(<CompanionSheet open onClose={() => {}} />)
    await flush()
    await userEvent.click(screen.getByRole('button', { name: /Copy pairing link/ }))
    expect(copied).toEqual([companionLink(location.origin, 'K7M2QX', 'cd'.repeat(32))])
  } finally {
    delete (window as any).isSecureContext
    delete (navigator as any).clipboard
  }
})

test('the countdown reads as minutes and seconds', () => {
  expect(mmss(300_000)).toBe('5:00')
  expect(mmss(42_100)).toBe('0:43')
  expect(mmss(-5)).toBe('0:00')
})
