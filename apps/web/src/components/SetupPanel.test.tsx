import { describe, expect, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { SetupPanel } from './SetupPanel.tsx'
import type { SetupAgent, SetupReport, SetupResult } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'
import { api } from '../test/fakes.ts'
import { host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const report = (agents: Partial<SetupAgent>[]): SetupReport => ({
  os: 'Linux x86_64',
  pkg: 'apt',
  sudo: false,
  checks: [{ id: 'tmux', label: 'tmux', ok: true, detail: 'tmux 3.4' }],
  keepSession: false,
  agents: agents.map((a) => ({ kind: 'codex', installed: true, signedIn: true, ...a })),
})

const panel = () => render(<MemoryRouter><SetupPanel host={host('laptop')} /></MemoryRouter>)
const toasts = () => useStore.getState().toasts.map((t) => t.title)

describe('the agents in the Setup tab', () => {
  test('an agent behind the newest version gets an Update button naming it', async () => {
    api.setup.mockResolvedValue(report([{ version: 'codex-cli 0.49.0', latest: '0.50.0', outdated: true, via: 'npm' }]))
    await panel()
    expect(screen.getByText('0.49.0 · signed in')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Update to 0.50.0' })).toBeTruthy()
  })

  test('a current one says so, with nothing to press', async () => {
    api.setup.mockResolvedValue(report([{ version: 'codex-cli 0.50.0', latest: '0.50.0', outdated: false, via: 'npm' }]))
    await panel()
    expect(screen.getByText('0.50.0 · up to date · signed in')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /Update/ })).toBeNull()
  })

  test('with no answer from the registry, a quiet Update still runs the updater', async () => {
    api.setup.mockResolvedValue(report([{ kind: 'antigravity', version: '1.3.3' }]))
    await panel()
    expect(screen.getByText('1.3.3 · signed in')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Update' })).toBeTruthy()
  })

  test('a missing agent gets Install', async () => {
    api.setup.mockResolvedValue(report([{ installed: false, version: undefined, signedIn: false, latest: '0.50.0' }]))
    await panel()
    expect(screen.getByText('Not installed')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Install' })).toBeTruthy()
  })

  test('updating says so in the row, then toasts the new version and re-checks', async () => {
    const user = userEvent.setup()
    api.setup.mockResolvedValue(report([{ version: 'codex-cli 0.49.0', latest: '0.50.0', outdated: true, via: 'npm' }]))
    let finish!: (r: SetupResult) => void
    api.runSetup.mockReturnValue(new Promise<SetupResult>((res) => (finish = res)))
    await panel()
    await user.click(screen.getByRole('button', { name: 'Update to 0.50.0' }))
    expect(screen.getByText('Updating…')).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Update to 0.50.0' }) as HTMLButtonElement).disabled).toBe(true)
    api.setup.mockResolvedValue(report([{ version: 'codex-cli 0.50.0', latest: '0.50.0', outdated: false, via: 'npm' }]))
    finish({ ok: true, output: 'added 1 package', version: 'codex-cli 0.50.0', latest: '0.50.0' })
    await flush()
    expect(api.runSetup.mock.calls).toEqual([['laptop', 'update:codex']])
    expect(toasts()).toEqual(['Codex updated to 0.50.0'])
    expect(api.setup).toHaveBeenCalledTimes(2)
    expect(screen.getByText('0.50.0 · up to date · signed in')).toBeTruthy()
  })

  test('a failed update opens the cause, the output, the command to run and the install guide under the row', async () => {
    const user = userEvent.setup()
    api.setup.mockResolvedValue(report([{ version: 'codex-cli 0.49.0', latest: '0.50.0', outdated: true, via: 'npm' }]))
    api.runSetup.mockResolvedValue({
      ok: false,
      output: 'npm ERR! code EACCES\nnpm ERR! syscall mkdir',
      version: 'codex-cli 0.49.0',
      manual: 'npm install -g @openai/codex@latest',
      hint: 'npm can\'t write there; set a user prefix: npm config set prefix "$HOME/.npm-global" && npm install -g @openai/codex@latest',
    })
    await panel()
    await user.click(screen.getByRole('button', { name: 'Update to 0.50.0' }))
    await flush()
    expect(screen.getByText(/npm can't write there/)).toBeTruthy()
    expect(screen.getByText(/npm ERR! syscall mkdir/).tagName).toBe('PRE')
    expect(screen.getByText('npm install -g @openai/codex@latest')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Copy command' })).toBeTruthy()
    expect((screen.getByText('Install guide').closest('a') as HTMLAnchorElement).href).toBe('https://developers.openai.com/codex/cli')
    expect(toasts()).toEqual([])
    // the button stays, so it can be tried again
    expect(screen.getByRole('button', { name: 'Update to 0.50.0' })).toBeTruthy()
  })

  test("OpenCode's row names its connected providers and opens them, instead of a sign-in", async () => {
    const user = userEvent.setup()
    api.setup.mockResolvedValue(report([{ kind: 'opencode', version: '1.19.3', latest: '1.19.3', outdated: false, signedIn: false, providers: ['Anthropic', 'Groq'] }]))
    api.providers.mockResolvedValue({ updatedAt: 1, providers: [{ id: 'anthropic', name: 'Anthropic', connected: true, via: 'credential', type: 'api' }] })
    await panel()
    expect(screen.getByText('1.19.3 · up to date · Anthropic, Groq')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Sign in' })).toBeNull()
    await user.click(screen.getByRole('button', { name: 'Providers' }))
    await flush()
    expect(api.providers.mock.calls).toEqual([['laptop', undefined]])
    expect(screen.getByRole('dialog', { name: 'OpenCode providers' })).toBeTruthy()
  })

  test('an OpenCode with no provider connected says it runs the free models', async () => {
    api.setup.mockResolvedValue(report([{ kind: 'opencode', version: '1.19.3', signedIn: true, providers: [] }]))
    await panel()
    expect(screen.getByText('1.19.3 · free models only')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Providers' })).toBeTruthy()
  })

  test("the hub's refusal shows in place too", async () => {
    const user = userEvent.setup()
    api.setup.mockResolvedValue(report([{ kind: 'gemini', installed: false, version: undefined, signedIn: false }]))
    api.runSetup.mockRejectedValue(new Error('Install Node.js first'))
    await panel()
    await user.click(screen.getByRole('button', { name: 'Install' }))
    await flush()
    expect(screen.getByText('Install Node.js first')).toBeTruthy()
  })
})
