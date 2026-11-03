import { expect, test } from 'bun:test'
import { causeOf, ConnLog, contextAt, downFor, dozedAt, powerContexts, span, why, type Env } from './connlog.ts'

/** A storage of its own, so the log can be loaded again from what it saved. */
function memory() {
  const m = new Map<string, string>()
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), raw: m }
}
const env = (o: Partial<{ online: boolean; hidden: boolean; frozen: { at?: number; back?: number } }> = {}): Env => ({
  online: () => o.online ?? true,
  hidden: () => o.hidden ?? false,
  frozen: () => o.frozen ?? {},
})
const HUB = { kind: 'hub' as const, name: 'studio' }

test('a drop is kept with its close code and how long it was down', () => {
  const log = new ConnLog(memory(), env())
  log.up('hub:a', HUB, 1_000)
  log.down('hub:a', HUB, 'closed', { code: 1006, at: 5_000 })
  expect(log.get('hub:a')?.up).toBe(false)
  expect(downFor(log.lastDrop('hub:a')!, 65_000)).toBe('still down after 1m')
  log.up('hub:a', HUB, 125_000)
  const d = log.lastDrop('hub:a')!
  expect(d).toMatchObject({ at: 5_000, back: 125_000, cause: 'closed', code: 1006 })
  expect(downFor(d)).toBe('2m')
  expect(why(d)).toBe('the connection was cut without a goodbye')
})

test('failed tries while down count on the same drop, and a sign-out explains it', () => {
  const log = new ConnLog(memory(), env())
  log.up('hub:a', HUB, 0)
  log.down('hub:a', HUB, 'closed', { code: 1006, at: 10 })
  log.down('hub:a', HUB, 'unreachable', { detail: 'Failed to fetch', at: 20 })
  log.down('hub:a', HUB, 'unreachable', { detail: 'Load failed', at: 30 })
  expect(log.get('hub:a')!.drops).toHaveLength(1)
  expect(log.lastDrop('hub:a')).toMatchObject({ tries: 2, last: 'Load failed', cause: 'closed' })
  log.down('hub:a', HUB, 'signed-out', { detail: 'studio no longer knows this phone' })
  expect(why(log.lastDrop('hub:a')!)).toBe('the hub signed this device out')
  log.explain('hub:a', 'signed-out', 'studio no longer knows this phone')
  expect(why(log.lastDrop('hub:a')!)).toBe('studio no longer knows this phone')
})

test('the stored copy is bounded, redacted and comes back after a restart', () => {
  const store = memory()
  const log = new ConnLog(store, env())
  for (let i = 0; i < 20; i++) {
    log.up('hub:a', HUB, i * 100)
    log.down('hub:a', HUB, 'closed', { code: 1006, at: i * 100 + 50, detail: 'wss://box/ws?ticket=abc123secret' })
  }
  expect(log.get('hub:a')!.drops.length).toBe(12)
  for (let i = 0; i < 50; i++) log.down(`m${i}`, { kind: 'computer', name: `pc${i}` }, 'unreachable', { at: 10_000 + i })
  expect(log.all().length).toBe(40)
  const saved = JSON.parse(store.raw.get('inyon.connlog.v1')!)
  expect(saved.length).toBeLessThanOrEqual(40)
  expect(store.raw.get('inyon.connlog.v1')).not.toContain('abc123secret')
  // the app closed while the last one was down: it comes back as cut, not as still down
  const again = new ConnLog(store, env())
  const last = again.all().find((s) => s.id === 'm49')!.drops.at(-1)!
  expect(last.cut).toBe(true)
  expect(downFor(last)).toBe('until the app closed')
  expect(again.get('m49')?.up).toBeUndefined()
})

test('what was going on: offline, in the background, just thawed', () => {
  expect(contextAt(env({ online: false }), 0)).toContain('offline')
  expect(contextAt(env({ hidden: true }), 0)).toEqual(['background'])
  expect(contextAt(env({ frozen: { at: 100, back: 5_000 } }), 8_000)).toContain('frozen')
  expect(contextAt(env({ frozen: { at: 100, back: 5_000 } }), 60_000)).not.toContain('frozen')
  const log = new ConnLog(memory(), env({ hidden: true }))
  log.up('x', HUB, 0)
  const d = log.down('x', HUB, 'closed', { code: 1006, at: 10 })
  expect(why(d)).toBe('the connection was cut without a goodbye while Inyon was in the background')
  log.annotate(d, ['doze', 'restricted'])
  expect(why(d)).toBe("Android's Doze paused the phone")
})

test('Doze counts when it was on at the drop or ended just before the app noticed', () => {
  expect(dozedAt({ idle: true }, 0)).toBe(true)
  // dozing since 1000 and not ended yet
  expect(dozedAt({ idleSince: 1_000, idleEnded: 500 }, 2_000)).toBe(true)
  // ended 30 s before the drop was noticed
  expect(dozedAt({ idleSince: 1_000, idleEnded: 70_000 }, 100_000)).toBe(true)
  // ended long before
  expect(dozedAt({ idleSince: 1_000, idleEnded: 2_000 }, 500_000)).toBe(false)
  // began after the drop
  expect(dozedAt({ idleSince: 9_000, idleEnded: 0 }, 2_000)).toBe(false)
  expect(dozedAt({}, 2_000)).toBe(false)
  // dozing now, but it began after the drop
  expect(dozedAt({ idle: true, idleSince: 9_000 }, 2_000)).toBe(false)
  expect(dozedAt({ idle: true, idleSince: 1_000 }, 2_000)).toBe(true)
  expect(powerContexts({ ignoringOptimizations: false, keepAlive: false, powerSave: true }, 0)).toEqual(['restricted', 'saver', 'no-keepalive'])
  expect(powerContexts({ ignoringOptimizations: true, keepAlive: true, backgroundRestricted: false }, 0)).toEqual([])
})

test('a machine\'s own error says why', () => {
  expect(causeOf('Connecting to studio… (Couldn\'t find the computer online. Is `inyon link` running there?)')).toBe('unreachable')
  expect(causeOf('Reconnecting to homebox')).toBe('hub-down')
  expect(causeOf('Connecting to homebox…')).toBe('hub-down')
  expect(causeOf('This computer is no longer paired with this phone')).toBe('signed-out')
  expect(causeOf('Connecting to studio… (bad handshake)')).toBe('handshake')
  expect(causeOf('ssh: connect to host 10.0.0.2 port 22: Connection refused')).toBe('offline')
  expect(causeOf(undefined)).toBe('closed')
})

test('spans read like people say them', () => {
  expect(span(4_000)).toBe('4s')
  expect(span(250_000)).toBe('4m 10s')
  expect(span(3_600_000)).toBe('1h')
  expect(span(5_400_000)).toBe('1h 30m')
})
