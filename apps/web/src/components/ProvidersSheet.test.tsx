import { describe, expect, test } from 'bun:test'
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ProvidersReport } from '@inyon/protocol'
import { ProvidersSheet } from './ProvidersSheet.tsx'
import { ConfirmHost } from './Confirm.tsx'
import { ApiError } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'
import { api } from '../test/fakes.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const report: ProvidersReport = {
  updatedAt: 1,
  providers: [
    { id: 'anthropic', name: 'Anthropic', connected: true, via: 'credential', type: 'api' },
    { id: 'groq', name: 'Groq', connected: true, via: 'env', env: ['GROQ_API_KEY'] },
    { id: 'opencode', name: 'OpenCode', connected: true },
    { id: 'openai', name: 'OpenAI', connected: false, env: ['OPENAI_API_KEY'] },
    { id: 'zai', name: 'Z.AI', connected: false },
  ],
}
const sheet = () => render(<><ProvidersSheet hostId="laptop" hostName="Laptop" open onClose={() => {}} /><ConfirmHost /></>)
const toasts = () => useStore.getState().toasts.map((t) => t.title)

describe("OpenCode's providers on a machine", () => {
  test('connected ones say how, and only a saved credential can be disconnected', async () => {
    api.providers.mockResolvedValue(report)
    await sheet()
    expect(api.providers.mock.calls).toEqual([['laptop', undefined]])
    expect(screen.getByText('Anthropic')).toBeTruthy()
    expect(screen.getByText('API key')).toBeTruthy()
    expect(screen.getByText('From GROQ_API_KEY')).toBeTruthy()
    expect(screen.getByText('Free models, no sign-in')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Disconnect' })).toHaveLength(1)
  })

  test('disconnecting asks first, then runs on the machine and reads the list again', async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue(report)
    await sheet()
    await user.click(screen.getByRole('button', { name: 'Disconnect' }))
    expect(screen.getByText('Disconnect Anthropic?')).toBeTruthy()
    api.providers.mockResolvedValue({ ...report, providers: report.providers.filter((p) => p.id !== 'anthropic') })
    await user.click(within(screen.getByRole('dialog', { name: 'Disconnect Anthropic?' })).getByRole('button', { name: 'Disconnect' }))
    await flush()
    expect(api.disconnectProvider.mock.calls).toEqual([['laptop', 'anthropic']])
    expect(toasts()).toContain('Anthropic disconnected')
    expect(api.providers).toHaveBeenCalledTimes(2)
    // (the confirmation sheet is still fading out, with a button of the same name)
    expect(within(screen.getByRole('dialog', { name: 'OpenCode providers' })).queryByRole('button', { name: 'Disconnect' })).toBeNull()
  })

  test("a refusal from the hub shows as a toast, and the list stays", async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue(report)
    api.disconnectProvider.mockRejectedValue(new ApiError(409, 'Groq is connected through GROQ_API_KEY in the machine\'s environment. Unset it there to disconnect.'))
    await sheet()
    await user.click(screen.getByRole('button', { name: 'Disconnect' }))
    await user.click(within(screen.getByRole('dialog', { name: 'Disconnect Anthropic?' })).getByRole('button', { name: 'Disconnect' }))
    await flush()
    expect(useStore.getState().toasts.some((t) => t.title === 'Could not disconnect' && /GROQ_API_KEY/.test(t.body ?? ''))).toBe(true)
  })

  test("the search narrows the rest; picking one starts OpenCode's login for it and shows its key prompt", async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue(report)
    api.connectProvider.mockResolvedValue({ pane: '%5' })
    api.signInScreen.mockResolvedValue({ lines: ['┌  Add credential', '│', '◆  Enter your API key', '│  _', '└'], links: [], input: { label: 'Enter your API key', secret: true } })
    await sheet()
    const list = screen.getByRole('listbox', { name: 'Providers to connect' })
    expect(within(list).getAllByRole('option').map((o) => o.textContent)).toEqual(['OpenAIor set OPENAI_API_KEY', 'Z.AI'])
    await user.type(screen.getByRole('textbox', { name: 'Search providers' }), 'z.a')
    expect(within(list).getAllByRole('option')).toHaveLength(1)
    await user.click(within(list).getByRole('option', { name: /Z\.AI/ }))
    await flush()
    expect(api.connectProvider.mock.calls).toEqual([['laptop', 'zai']])
    expect(screen.getByRole('dialog', { name: 'Connect Z.AI' })).toBeTruthy()
    const key = (await screen.findByLabelText('Enter your API key')) as HTMLInputElement
    expect(key.type).toBe('password')
    await user.type(key, 'sk-test')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    expect(api.signInSend.mock.calls.at(-1)).toEqual(['laptop', '%5', { text: 'sk-test' }])
  })

  test('Enter in the search connects the first match; no match says so', async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue(report)
    api.connectProvider.mockResolvedValue({ pane: '%5' })
    api.signInScreen.mockResolvedValue({ lines: [], links: [] })
    await sheet()
    const box = screen.getByRole('textbox', { name: 'Search providers' })
    await user.type(box, 'nothing like this')
    expect(screen.getByText('No provider matches “nothing like this”.')).toBeTruthy()
    await user.clear(box)
    await user.type(box, 'open{Enter}')
    await flush()
    expect(api.connectProvider.mock.calls).toEqual([['laptop', 'openai']])
  })

  test("sign out disconnects every saved credential, not the environment's", async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue(report)
    await sheet()
    await user.click(screen.getByRole('button', { name: 'Sign out of OpenCode' }))
    await user.click(within(screen.getByRole('dialog', { name: 'Sign out of every provider?' })).getByRole('button', { name: 'Sign out' }))
    await flush()
    expect(api.disconnectProvider.mock.calls).toEqual([['laptop', 'anthropic']])
    expect(toasts()).toContain('Signed out of OpenCode')
  })

  test("when the machine can't list them, the reason shows with a way to try again", async () => {
    const user = userEvent.setup()
    api.providers.mockResolvedValue({ updatedAt: 1, providers: [], error: "OpenCode's provider list couldn't be read on that machine. Is OpenCode installed there?" })
    await sheet()
    expect(screen.getByText(/couldn't be read/)).toBeTruthy()
    api.providers.mockResolvedValue(report)
    await user.click(screen.getByRole('button', { name: 'Try again' }))
    await flush()
    expect(screen.getByText('Anthropic')).toBeTruthy()
  })
})
