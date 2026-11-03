import { afterAll, afterEach, expect, mock, spyOn, test } from 'bun:test'
import { fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { LaptopSheet, SETUP_COMMANDS } from './LaptopSheet.tsx'
import { flush, renderSettled as render } from '../test/render.tsx'
import { api } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'

/** The Android app's scanner, answering with whatever the user did on it. */
function phone(scanQr: () => Promise<unknown>, clipboard = '') {
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: { InyonDevice: { scanQr, readClipboardText: async () => ({ text: clipboard }) } },
  }
}

// bun keeps a spy in place for every test file after this one
afterAll(() => mock.restore())

afterEach(() => {
  delete (window as any).Capacitor
  useStore.setState({ toasts: [] })
})

test('says how to get the helper onto a laptop or server, for Mac/Linux and for Windows', async () => {
  const user = userEvent.setup()
  await render(<LaptopSheet open onClose={() => {}} />)
  expect(screen.getByText(SETUP_COMMANDS.unix.command)).toBeTruthy()
  expect(screen.getByText(/on a server, your SSH session/)).toBeTruthy()
  expect(screen.getByRole('heading', { name: 'Add a computer' })).toBeTruthy()
  await user.click(screen.getByRole('tab', { name: 'Windows' }))
  expect(screen.getByText(SETUP_COMMANDS.windows.command)).toBeTruthy()
  expect(screen.getByText(/In PowerShell, run/)).toBeTruthy()
  expect(SETUP_COMMANDS.unix.command).toBe('curl -fsSL https://inyon.tech/get.sh | sh')
  expect(SETUP_COMMANDS.windows.command).toBe('irm https://inyon.tech/get.ps1 | iex')
})

test('typing the ID and code pairs as soon as the last digit is in', async () => {
  const user = userEvent.setup()
  const pairLaptop = spyOn(api, 'pairLaptop').mockResolvedValue({ id: 'box' } as any)
  let closed = false
  await render(<LaptopSheet open onClose={() => (closed = true)} />)
  // copied off a terminal: lower case, an O for a zero
  await user.type(screen.getByRole('textbox', { name: 'ID' }), 'k7q4mxot')
  expect((screen.getByRole('textbox', { name: 'ID' }) as HTMLInputElement).value).toBe('K7Q4-MX0T')
  // the code box takes over once the ID is complete
  expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Code' }))
  await user.keyboard('493 81')
  expect(pairLaptop).not.toHaveBeenCalled()
  await user.keyboard('2')
  expect(pairLaptop).toHaveBeenCalledWith('K7Q4MX0T 493812')
  await new Promise((r) => setTimeout(r, 0))
  expect(closed).toBe(true)
  pairLaptop.mockRestore()
})

test("after a typed code, the computer's ID shows, to check against the one inyon printed", async () => {
  const user = userEvent.setup()
  const pairLaptop = spyOn(api, 'pairLaptop').mockResolvedValue({ id: 'box', machineId: 'ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u' })
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.type(screen.getByRole('textbox', { name: 'ID' }), 'K7Q4MX2T')
  await user.keyboard('493812')
  await flush()
  const toast = useStore.getState().toasts.at(-1)!
  expect(toast.title).toBe('Paired')
  expect(toast.body).toContain('Its ID is ybnd rfg8 ejkm.')
  pairLaptop.mockRestore()
})

test('an ID and code pasted together fill both boxes', async () => {
  const user = userEvent.setup()
  const pairLaptop = spyOn(api, 'pairLaptop').mockResolvedValue({ id: 'box' } as any)
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('textbox', { name: 'ID' }))
  await user.paste('K7Q4-MX2T  493 812')
  expect(pairLaptop).toHaveBeenCalledWith('K7Q4MX2T 493812')
  pairLaptop.mockRestore()
})

test('"Type a code instead" on the scanner goes to the ID box', async () => {
  const user = userEvent.setup()
  phone(async () => ({ type: true }))
  await render(<LaptopSheet open onClose={() => {}} link="inyon://pair?m=x" />)
  await user.click(screen.getByRole('button', { name: 'Use a different code' }))
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'ID' }))
  expect(useStore.getState().toasts).toEqual([])
})

test('a scanned code asks to pair with that computer by name, in one tap', async () => {
  const pairLaptop = spyOn(api, 'pairLaptop').mockResolvedValue({ id: 'x' })
  const link = 'inyon://pair?m=abc&s=secret&n=studio'
  await render(<LaptopSheet open onClose={() => {}} link={link} />)
  expect(screen.getByText('studio')).toBeTruthy()
  // nothing to set up or type: the steps stay out of the way
  expect(screen.queryByText('1. On the computer')).toBeNull()
  fireEvent.submit(document.getElementById('laptop-form')!)
  await flush()
  expect(pairLaptop).toHaveBeenCalledWith(link)
  pairLaptop.mockRestore()
})

test("a scanned computer code shows the computer's ID and pairs only after a tap", async () => {
  const user = userEvent.setup()
  const pairLaptop = spyOn(api, 'pairLaptop').mockResolvedValue({ id: 'x' })
  const link = 'inyon://pair?m=ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u&s=secretsecretsecret&n=build-box'
  phone(async () => ({ text: link }))
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  await flush()
  expect(screen.getByText('build-box')).toBeTruthy()
  expect(screen.getByText('ybnd rfg8 ejkm')).toBeTruthy()
  expect(pairLaptop).not.toHaveBeenCalled()
  await user.click(screen.getByRole('button', { name: 'Pair' }))
  await flush()
  expect(pairLaptop).toHaveBeenCalledWith(link)
  pairLaptop.mockRestore()
})

test("the QR code's own form, with the link in its hash, shows the computer's ID too", async () => {
  const user = userEvent.setup()
  const link = 'https://app.example/#pair?m=ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u&s=secretsecretsecret&n=build-box'
  phone(async () => ({ text: link }))
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  await flush()
  expect(screen.getByText('build-box')).toBeTruthy()
  expect(screen.getByText('ybnd rfg8 ejkm')).toBeTruthy()
})

test('a paste request from the scanner goes to the link box, which reads the clipboard', async () => {
  const user = userEvent.setup()
  phone(async () => ({ paste: true }), ' inyon://pair?m=abc \n')
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  const box = screen.getByRole('textbox', { name: 'Pairing link' }) as HTMLInputElement
  expect(document.activeElement).toBe(box)
  await user.click(screen.getByRole('button', { name: 'Paste' }))
  // a pasted link is shown for a yes like a scanned one
  expect(screen.getByRole('button', { name: 'Use a different code' })).toBeTruthy()
})

test('no camera access points to typing the ID and code', async () => {
  const user = userEvent.setup()
  phone(async () => ({ denied: true }))
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  expect(useStore.getState().toasts.map((t) => t.title)).toEqual(['No camera access'])
  expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'ID' }))
})

test('closing the scanner does nothing', async () => {
  const user = userEvent.setup()
  phone(async () => ({ cancelled: true }))
  await render(<LaptopSheet open onClose={() => {}} />)
  await user.click(screen.getByRole('button', { name: /Scan the code/ }))
  expect(useStore.getState().toasts).toEqual([])
  expect(screen.getByRole('textbox', { name: 'ID' })).toBeTruthy()
})
