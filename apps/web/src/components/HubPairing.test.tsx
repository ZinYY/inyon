import { afterEach, beforeEach, expect, mock, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { LaptopSheet } from './LaptopSheet.tsx'
import { forgetHub, hubToken, listHubs } from '../lib/hubs.ts'
import { useStore } from '../lib/store.ts'
import { emit } from '../test/fakes.ts'
import { host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const realFetch = globalThis.fetch
const claims: { url: string; body: any }[] = []
beforeEach(() => {
  localStorage.removeItem('inyon.hubs.v1')
  claims.length = 0
  // the hub's claim endpoint, answered here
  globalThis.fetch = mock(async (input: any, init?: RequestInit) => {
    const url = String(input instanceof Request ? input.url : input)
    claims.push({ url, body: JSON.parse(String(init?.body ?? '{}')) })
    if (claims[claims.length - 1]!.body.code !== 'K7Q4MX2T:493812') return new Response(JSON.stringify({ error: 'That code is not right.' }), { status: 401 })
    return new Response(JSON.stringify({ token: 'tok', hub: { id: 'h1', name: 'studio', version: '0.12.0' } }), { status: 200 })
  }) as any
})
afterEach(() => {
  globalThis.fetch = realFetch
  for (const h of listHubs()) forgetHub(h.id)
  delete (window as any).Capacitor
})

test('a scanned hub code pairs with one tap and remembers the hub', async () => {
  const onClose = mock(() => {})
  await render(<LaptopSheet open onClose={onClose} link="inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio" />)
  expect(screen.getByText(/Pair with the hub/)).toBeTruthy()
  await userEvent.click(screen.getByRole('button', { name: 'Pair' }))
  await flush()
  expect(claims[0]!.url).toBe('https://hub.example/api/phones/claim')
  expect(claims[0]!.body.code).toBe('K7Q4MX2T:493812')
  expect(listHubs()).toMatchObject([{ id: 'h1', name: 'studio', url: 'https://hub.example' }])
  expect(await hubToken('h1')).toBe('tok')
  expect(onClose).toHaveBeenCalled()
})

test("a hub code shows the hub's address, not just the name the code gives it, and its ID", async () => {
  await render(<LaptopSheet open onClose={() => {}} link="inyon://hub?u=https%3A%2F%2Fevil.example&c=K7Q4MX2T%3A493812&n=Home%20hub&h=3f9c1a7e2b6d4c80" />)
  expect(screen.getByText('evil.example')).toBeTruthy()
  expect(screen.getByText(/It calls itself “Home hub”/)).toBeTruthy()
  expect(screen.getByText('3f9c 1a7e 2b6d 4c80')).toBeTruthy()
  expect(claims).toEqual([])
})

test('a scanned hub code waits for a tap before anything is sent', async () => {
  const scanQr = mock(async () => ({ text: 'inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio&h=h1h1h1h1h1h1h1h1' }))
  ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: { InyonDevice: { scanQr } } }
  await render(<LaptopSheet open onClose={() => {}} />)
  await userEvent.click(screen.getByRole('button', { name: /Connect to a hub/ }))
  await userEvent.click(screen.getByRole('button', { name: /Scan the hub's code/ }))
  await flush()
  expect(screen.getByText('hub.example')).toBeTruthy()
  expect(claims).toEqual([])
  await userEvent.click(screen.getByRole('button', { name: 'Pair' }))
  await flush()
  expect(claims.map((c) => c.url)).toEqual(['https://hub.example/api/phones/claim'])
})

test('a code for a plain-http hub on a public address says why it won\'t pair, and sends nothing', async () => {
  await render(<LaptopSheet open onClose={() => {}} link="inyon://hub?u=http%3A%2F%2F203.0.113.5%3A4777&c=K7Q4MX2T%3A493812&n=studio" />)
  expect(screen.getByText(/Inyon won't use plain http with 203.0.113.5/)).toBeTruthy()
  expect((screen.getByRole('button', { name: 'Pair' }) as HTMLButtonElement).disabled).toBe(true)
  expect(claims).toEqual([])
})

test('a typed address and code pair too, and a wrong code says so', async () => {
  await render(<LaptopSheet open onClose={() => {}} />)
  await userEvent.click(screen.getByRole('button', { name: /Connect to a hub/ }))
  await userEvent.type(screen.getByLabelText('Hub address'), 'box:4777')
  await userEvent.type(screen.getByLabelText('Hub code'), 'k7q4-mx2t 000000')
  await userEvent.click(screen.getByRole('button', { name: 'Pair' }))
  await flush()
  expect(claims[0]!.url).toBe('http://box:4777/api/phones/claim')
  expect(listHubs()).toEqual([])
  expect(useStore.getState().toasts.map((t) => t.title)).toContain("Couldn't pair with the hub")
})

test("a hub's machines join the list with the hub's id in front", () => {
  useStore.setState({ hosts: [] })
  emit({ t: 'overview', hosts: [host('mine')] } as any)
  emit({ t: 'overview', hosts: [host('hub:h1/laptop')], source: 'hub:h1' } as any)
  expect(useStore.getState().hosts.map((h) => h.config.id)).toEqual(['mine', 'hub:h1/laptop'])
  // the hub going quiet changes only its own
  emit({ t: 'overview', hosts: [], source: 'hub:h1' } as any)
  expect(useStore.getState().hosts.map((h) => h.config.id)).toEqual(['mine'])
})
