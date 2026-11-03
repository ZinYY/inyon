import { beforeEach, expect, test } from 'bun:test'
import { act, fireEvent, screen, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { ConnectSheet, LastDrop, targetFor } from './ConnectSheet.tsx'
import type { CheckDeps, Sock } from '../lib/connectCheck.ts'
import { connLog } from '../lib/connlog.ts'
import { useStore } from '../lib/store.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const HUB = { id: 'h1', name: 'studio', url: 'https://studio.example', pairedAt: 1 }
const pairHub = () => localStorage.setItem('inyon.hubs.v1', JSON.stringify([HUB]))
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status })

/** A network the test answers by hand, one request at a time. */
function held() {
  const waiting: { url: string; resolve: (r: Response) => void; reject: (e: Error) => void }[] = []
  const fetch = (url: string) => new Promise<Response>((resolve, reject) => waiting.push({ url, resolve, reject }))
  return { fetch, waiting }
}

function deps(o: Partial<CheckDeps> = {}): CheckDeps {
  return {
    fetch: async (url) => (url.includes('/ticket') ? json(200, { ticket: 't' }) : json(200, { authed: true })),
    socket: () => {
      const s: Sock = { onopen: null, onclose: null, onerror: null, close() {} }
      setTimeout(() => s.onopen?.({}), 0)
      return s
    },
    online: () => true,
    lookUp: async () => ['100.64.0.7'],
    token: async () => 'tok',
    hubWsUrl: () => 'wss://studio.example/ws',
    host: () => undefined,
    timeoutMs: 1000,
    ...o,
  }
}

let at = '/'
function Where() {
  at = useLocation().pathname + useLocation().search
  return null
}
const sheet = (d: CheckDeps, id = 'hub:h1') => (
  <MemoryRouter>
    <Routes>
      <Route path="*" element={<><ConnectSheet id={id} open onClose={() => {}} checkDeps={d} /><Where /></>} />
    </Routes>
  </MemoryRouter>
)
const settle = async () => {
  for (let i = 0; i < 10; i++) await act(() => new Promise((r) => setTimeout(r, 5)))
}

beforeEach(() => {
  for (const s of connLog.all()) connLog.forget(s.id)
  at = '/'
})

test('the checks run in order, each with a plain sentence while it runs', async () => {
  pairHub()
  const net = held()
  await render(sheet(deps({ fetch: net.fetch })))
  expect(screen.getByRole('heading', { name: "Can't connect?" })).toBeTruthy()
  const list = screen.getByRole('list', { name: 'Checks' })
  // the first request is out: its step says what it's doing, the later ones wait
  await settle()
  expect(within(list).getByRole('status').textContent).toBe('Checking the hub answers…')
  expect(list.querySelectorAll('[data-state="pending"]').length).toBe(3)
  expect(list.querySelectorAll('[data-state="pass"]').length).toBe(2)
  net.waiting.shift()!.resolve(new Response(null))
  await settle()
  expect(within(list).getByRole('status').textContent).toBe('Checking the hub answers to this address…')
})

test('a failure says what to do, offers the fix, and the raw details stay folded', async () => {
  pairHub()
  await render(sheet(deps({ fetch: async (url) => (url.includes('/ticket') ? json(401, { error: 'Not signed in', reason: 'revoked' }) : new Response(null)) })))
  await settle()
  const failed = document.querySelector('[data-state="fail"]')!
  expect(failed.textContent).toContain('The hub no longer knows this phone.')
  expect(failed.textContent).toContain('Pair again: on the hub choose Open on your phone')
  expect(document.querySelector('[data-state="skip"]')!.textContent).toContain('Skipped until the step above passes.')
  // no raw output on screen until Details is opened
  const toggle = screen.getByRole('button', { name: 'Details' })
  expect(toggle.getAttribute('aria-expanded')).toBe('false')
  expect(screen.queryByText(/POST \/api\/phones\/ticket answered 401/)).toBeNull()
  fireEvent.click(toggle)
  await flush()
  expect(toggle.getAttribute('aria-expanded')).toBe('true')
  expect(document.querySelector('pre')!.textContent).toContain('POST /api/phones/ticket answered 401')
  fireEvent.click(within(failed as HTMLElement).getByRole('button', { name: 'Pair again' }))
  await flush()
  expect(at).toBe('/machines?add=1')
})

test('Copy report puts the redacted report on the clipboard', async () => {
  pairHub()
  connLog.down('hub:h1', { kind: 'hub', name: 'studio' }, 'closed', { code: 1006, detail: 'wss://studio.example/ws?ticket=s3cr3t-t1cket', at: Date.now() - 60_000 })
  let copied = ''
  Object.defineProperty(window, 'isSecureContext', { configurable: true, value: true })
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (t: string) => void (copied = t) } })
  await render(sheet(deps()))
  await settle()
  expect(screen.getByText(/last drop/).textContent).toContain('the connection was cut without a goodbye')
  fireEvent.click(screen.getByRole('button', { name: /Copy report/ }))
  await settle()
  expect(copied).toContain('Checks for studio (hub at https://studio.example)')
  expect(copied).toContain('[ok] Live connection: The live connection opens.')
  expect(copied).toContain('studio (hub): the connection was cut without a goodbye')
  expect(copied).not.toContain('s3cr3t')
  expect(useStore.getState().toasts.at(-1)?.title).toBe('Report copied')
  expect(screen.getByText(/Everything here works/)).toBeTruthy()
})

test("a new run drops the last run's report", async () => {
  pairHub()
  let copied = ''
  Object.defineProperty(window, 'isSecureContext', { configurable: true, value: true })
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (t: string) => void (copied = t) } })
  let hold = false
  const net = held()
  await render(sheet(deps({ fetch: (url, init) => (hold ? net.fetch(url) : url.includes('/ticket') ? Promise.resolve(json(200, { ticket: 't' })) : Promise.resolve(json(200, { authed: true }))) })))
  await settle()
  hold = true
  fireEvent.click(screen.getByRole('button', { name: /Check again/ }))
  await settle()
  fireEvent.click(screen.getByRole('button', { name: /Copy report/ }))
  await settle()
  // what's on screen now: the hub check still out, the live connection not run yet
  expect(copied).toContain('[...] Hub answers')
  expect(copied).not.toContain('[ok] Live connection')
})

test('Check again runs everything afresh', async () => {
  pairHub()
  let online = false
  await render(sheet(deps({ online: () => online })))
  await settle()
  expect(document.querySelector('[data-state="fail"]')!.textContent).toContain('This device is offline.')
  online = true
  fireEvent.click(screen.getByRole('button', { name: /Check again/ }))
  await settle()
  expect(document.querySelector('[data-state="fail"]')).toBeNull()
})

test('what gets checked for each kind of id', () => {
  pairHub()
  const link = { config: { id: 'pc1', name: 'laptop', kind: 'link' }, status: 'connecting' } as never
  const viaHub = { config: { id: 'hub:h1/n1', name: 'server', kind: 'node' }, status: 'error' } as never
  const node = { config: { id: 'n2', name: 'box', kind: 'node' }, status: 'error' } as never
  expect(targetFor('hub:h1', [])?.kind).toBe('hub')
  expect(targetFor('hub:gone', [])).toBeUndefined()
  expect(targetFor('pc1', [link])?.kind).toBe('computer')
  const m = targetFor('hub:h1/n1', [viaHub])
  expect(m?.kind === 'machine' && m.via.kind).toBe('hub')
  // in a hub's own page: its machines go through this hub
  const own = targetFor('n2', [node])
  expect(own?.kind === 'machine' && own.via.kind).toBe('self')
  expect(targetFor('self', [])?.kind).toBe('self')
})

test('Settings shows the last drop in words', async () => {
  connLog.up('pc1', { kind: 'computer', name: 'laptop' }, Date.now() - 300_000)
  connLog.down('pc1', { kind: 'computer', name: 'laptop' }, 'closed', { code: 4408, at: Date.now() - 240_000 })
  connLog.up('pc1', { kind: 'computer', name: 'laptop' }, Date.now() - 180_000)
  await render(<LastDrop id="pc1" />)
  expect(document.body.textContent).toBe('Last drop 4m ago, it stopped answering pings (down 1m)')
  await render(<LastDrop id="nothing" />)
})
