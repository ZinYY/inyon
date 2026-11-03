import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'
import { fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { HostConfig } from '@inyon/protocol'
import { ServerSheet } from './ServerSheet.tsx'
import { api } from '../test/fakes.ts'
import { fresh, host } from '../test/fixtures.ts'
import { useStore } from '../lib/store.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

// bun keeps a spy in place for every test file after this one
afterAll(() => mock.restore())

// these run as the Android app, which can SSH itself; the web app's sheet is tested at the end
const asAndroid = () => ((window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} })
beforeEach(asAndroid)
afterEach(() => delete (window as any).Capacitor)

const work: HostConfig = { id: 's1', name: 'Work', kind: 'ssh', target: 'me@10.0.0.1', port: 2222, auth: 'phone-key', hostKey: 'SHA256:abc' }
const home: HostConfig = { id: 's2', name: 'Home', kind: 'ssh', target: 'pi@192.168.1.2', auth: 'password', hostKey: 'SHA256:def' }
const noop = () => {}
const field = (placeholder: string) => screen.getByPlaceholderText(placeholder) as HTMLInputElement

describe('ServerSheet keeps what you type while live data flows', () => {
  test('renaming survives the store rebuilding the server on every refresh', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ServerSheet open onClose={noop} edit={work} />)
    const name = field('Work server')
    expect(name.value).toBe('Work')
    await user.clear(name)
    await user.type(name, 'Pro')
    // a status poll: an equal server, every object new
    rerender(<ServerSheet open onClose={noop} edit={fresh(work)} />)
    rerender(<ServerSheet open onClose={noop} edit={{ ...fresh(work), name: 'Work (renamed elsewhere)' }} />)
    await user.type(name, 'd box')
    expect(name.value).toBe('Prod box')
  })

  test('the address and port fields keep their edits too', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ServerSheet open onClose={noop} edit={work} />)
    const target = field('you@100.64.0.5')
    const port = field('22')
    await user.clear(target)
    await user.type(target, 'me@')
    await user.clear(port)
    await user.type(port, '2')
    rerender(<ServerSheet open onClose={noop} edit={fresh(work)} />)
    await user.type(target, '10.0.0.9')
    await user.type(port, '2x2')
    expect(target.value).toBe('me@10.0.0.9')
    expect(port.value).toBe('222')
  })

  test('adding a server: a refresh does not clear the form', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ServerSheet open onClose={noop} />)
    await user.type(field('Work server'), 'New')
    rerender(<ServerSheet open onClose={noop} edit={undefined} />)
    await user.type(field('Work server'), ' one')
    expect(field('Work server').value).toBe('New one')
  })

  test("reopening shows the server's current values", async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ServerSheet open onClose={noop} edit={work} />)
    await user.type(field('Work server'), ' typed then cancelled')
    rerender(<ServerSheet open={false} onClose={noop} edit={work} />)
    rerender(<ServerSheet open onClose={noop} edit={{ ...work, name: 'Work 2', port: 2200 }} />)
    await flush()
    expect(field('Work server').value).toBe('Work 2')
    expect(field('22').value).toBe('2200')
  })

  test('switching to another server shows its values', async () => {
    const user = userEvent.setup()
    const { rerender } = await render(<ServerSheet open onClose={noop} edit={work} />)
    await user.type(field('Work server'), 'xx')
    rerender(<ServerSheet open onClose={noop} edit={home} />)
    await flush()
    expect(field('Work server').value).toBe('Home')
    expect(field('you@100.64.0.5').value).toBe('pi@192.168.1.2')
    expect(field('22').value).toBe('')
  })

  test('saving sends what was typed, keeping the trusted key', async () => {
    const user = userEvent.setup()
    const onClose = () => {}
    const { rerender } = await render(<ServerSheet open onClose={onClose} edit={work} />)
    await user.clear(field('Work server'))
    await user.type(field('Work server'), 'Prod')
    rerender(<ServerSheet open onClose={onClose} edit={fresh(work)} />)
    fireEvent.submit(document.getElementById('server-form')!)
    await flush()
    expect(api.editServer.mock.calls[0]).toEqual(['s1', { name: 'Prod', target: 'me@10.0.0.1', port: 2222, via: '', hostKey: 'SHA256:abc', auth: 'phone-key', password: undefined, privateKey: undefined, passphrase: undefined }])
  })
})

test("says why the phone-key command is missing instead of just hiding it", async () => {
  const { spyOn } = await import('bun:test')
  const { api: real } = await import('../lib/api.ts')
  const key = spyOn(real, 'phoneKey').mockRejectedValue(new Error('keystore locked'))
  await render(<ServerSheet open onClose={noop} />)
  await flush()
  expect(screen.getByText(/Couldn't read this phone's key \(keystore locked\)/)).toBeTruthy()
  key.mockRestore()
})

describe('servers Inyon cannot run on', () => {
  test('a paired computer can connect instead: an alias, no fingerprint, no sign-in', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [host('jump', { config: { id: 'jump', kind: 'link', name: 'Jump box', target: undefined } })] })
    api.addServer.mockClear()
    api.probe.mockClear()
    await render(<ServerSheet open onClose={noop} />)
    await user.click(screen.getByRole('combobox', { name: 'Connect from', hidden: true }))
    await user.click(await screen.findByText('Jump box'))
    expect(screen.queryByText('Sign in with')).toBeNull()
    await user.type(field('Work server'), 'DB')
    await user.type(field('work-db or you@10.0.0.5'), 'work-db')
    expect(screen.getByText(/ssh work-db/)).toBeTruthy()
    fireEvent.submit(document.getElementById('server-form')!)
    await flush()
    expect(api.probe).not.toHaveBeenCalled()
    expect(api.addServer.mock.calls[0]).toEqual([{ name: 'DB', target: 'work-db', port: undefined, via: 'jump' }])
    useStore.setState({ hosts: [] })
  })

  test('"None" signs in with no key or password once the fingerprint is trusted', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [] })
    api.addServer.mockClear()
    api.probe.mockResolvedValue({ fingerprint: 'SHA256:xyz', keyType: 'ssh-ed25519' } as any)
    await render(<ServerSheet open onClose={noop} />)
    // without a paired computer there's nothing to choose between
    expect(screen.queryByText('Connect from')).toBeNull()
    await user.type(field('Work server'), 'Open box')
    await user.type(field('you@100.64.0.5'), 'me@10.0.0.7')
    await user.click(screen.getAllByRole('radio', { hidden: true }).find((r) => r.textContent === 'None')!)
    fireEvent.submit(document.getElementById('server-form')!)
    await flush()
    expect(screen.getByText('SHA256:xyz')).toBeTruthy()
    fireEvent.submit(document.getElementById('server-form')!)
    await flush()
    expect(api.addServer.mock.calls[0]![0]).toMatchObject({ target: 'me@10.0.0.7', auth: 'none', hostKey: 'SHA256:xyz', via: '', password: undefined })
  })
})

describe('in a browser, which has no SSH', () => {
  beforeEach(() => delete (window as any).Capacitor)

  test('with no paired computer it asks for one, and offers no form to fill', async () => {
    useStore.setState({ hosts: [] })
    await render(<ServerSheet open onClose={noop} />)
    expect(screen.getByText(/Pair a computer first/)).toBeTruthy()
    expect(screen.queryByPlaceholderText('Work server')).toBeNull()
    expect((screen.getByRole('button', { name: 'Continue', hidden: true }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('it connects through the paired computer, with no "This phone" choice', async () => {
    const user = userEvent.setup()
    useStore.setState({ hosts: [host('jump', { config: { id: 'jump', kind: 'link', name: 'Jump box', target: undefined } })] })
    api.addServer.mockClear()
    await render(<ServerSheet open onClose={noop} />)
    // the one computer is the only way in, so there's nothing to pick
    expect(screen.queryByText('Connect from')).toBeNull()
    expect(screen.queryByText('Sign in with')).toBeNull()
    await user.type(field('Work server'), 'DB')
    await user.type(field('work-db or you@10.0.0.5'), 'work-db')
    fireEvent.submit(document.getElementById('server-form')!)
    await flush()
    expect(api.addServer.mock.calls[0]).toEqual([{ name: 'DB', target: 'work-db', port: undefined, via: 'jump' }])
    useStore.setState({ hosts: [] })
  })
})
