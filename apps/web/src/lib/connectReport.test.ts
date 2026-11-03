import { expect, test } from 'bun:test'
import { buildReport, type ReportInput } from './connectReport.ts'
import type { Subject } from './connlog.ts'

const T = Date.UTC(2026, 9, 31, 14, 2, 0)
const hub: Subject = { id: 'hub:h1', kind: 'hub', name: 'studio', drops: [] }
const laptop: Subject = { id: 'pc1', kind: 'computer', name: 'laptop', drops: [] }

const input = (o: Partial<ReportInput> = {}): ReportInput => ({
  now: T,
  version: '0.24.0',
  commit: 'abc1234',
  app: { version: '0.24.0', build: '77' },
  platform: 'android app',
  online: true,
  hidden: false,
  power: { ignoringOptimizations: false, powerSave: false, idle: false, backgroundRestricted: false, standbyBucket: 'rare' },
  keepAlive: true,
  target: { name: 'studio', kind: 'hub', address: 'https://studio.example' },
  steps: [
    { id: 'network', title: 'Network', doing: '', state: 'pass', say: 'This device is online.' },
    { id: 'signed-in', title: 'Signed in', doing: '', state: 'fail', say: 'The hub no longer knows this phone.', fix: 'Pair again.', detail: 'POST /api/phones/ticket answered 401' },
    { id: 'socket', title: 'Live connection', doing: '', state: 'skip', say: 'Skipped until the step above passes.' },
  ],
  drops: [
    { subject: hub, drop: { at: T - 120_000, back: T - 60_000, cause: 'closed', code: 1006, while: ['background', 'doze'], tries: 2, last: 'Failed to fetch' } },
    { subject: laptop, drop: { at: T - 600_000, cause: 'unreachable', while: [] } },
  ],
  ...o,
})

test('the report says what is installed, what the checks found and the last drops', () => {
  const r = buildReport(input())
  expect(r).toContain('Inyon connection report, 2026-10-31 14:02 UTC')
  expect(r).toContain('App: Inyon 0.24.0 (abc1234), Android app 0.24.0 build 77')
  expect(r).toContain('Platform: android app')
  expect(r).toContain('Android: Stay connected on · battery limits on · background restricted no · battery saver off · Doze no · standby bucket rare')
  expect(r).toContain('Checks for studio (hub at https://studio.example)')
  expect(r).toContain('  [ok] Network: This device is online.')
  expect(r).toContain('  [FAIL] Signed in: The hub no longer knows this phone.')
  expect(r).toContain('         Do: Pair again.')
  expect(r).toContain('         POST /api/phones/ticket answered 401')
  expect(r).toContain('  [skip] Live connection')
  expect(r).toContain("2026-10-31 14:00 UTC  studio (hub): Android's Doze paused the phone; down 1m; close code 1006; while in the background, Doze; 2 failed tries, last: Failed to fetch")
  expect(r).toContain('2026-10-31 13:52 UTC  laptop (computer): nothing answered; still down after 10m')
})

test('a browser report has no Android line and says when nothing dropped', () => {
  const r = buildReport(input({ app: undefined, power: undefined, keepAlive: undefined, platform: 'browser', target: undefined, steps: undefined, drops: [] }))
  expect(r).not.toContain('Android')
  expect(r).toContain('Last drops\n  None recorded.')
})

test('no token, ticket, cookie, code or key gets into it', () => {
  const r = buildReport(
    input({
      target: { name: 'studio', kind: 'hub', address: 'https://studio.example/#hub?u=https://studio.example&c=K7Q4MX2T:493812' },
      steps: [{ id: 'socket', title: 'Live connection', doing: '', state: 'fail', say: 'refused', detail: `wss://studio.example/ws?ticket=9f8e7d6c5b4a&catalog=2\nauthorization: ${'Bear' + 'er'} ${['tk', '9f8e7d6c5b4a39281706f5e4d3c2b1a0'].join('_')}` }],
      drops: [{ subject: laptop, drop: { at: T, cause: 'handshake', detail: 'inyon://pair?m=yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o&s=secret', while: [] } }],
    }),
  )
  for (const secret of ['K7Q4MX2T', '493812', '9f8e7d6c5b4a', 'tk_9f8e', 'yj4bqhvahk8', 's=secret']) expect(r).not.toContain(secret)
  expect(r).toContain('wss://studio.example/ws?[hidden]')
  expect(r).toContain('studio.example')
})
