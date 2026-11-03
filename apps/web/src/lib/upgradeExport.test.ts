import { afterEach, expect, test } from 'bun:test'
import { collectExport, exportForUpgrade, startUpgradeExport } from './upgradeExport.ts'
import { VERSION } from './version.ts'

afterEach(() => {
  delete (window as any).Capacitor
  localStorage.clear()
})

/** The Android app's bridge, with InyonMigrate answering after `delay` ms and noting what it got. */
function android(delays: number[] = []) {
  const got: { json: string }[] = []
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: {
      InyonMigrate: {
        exportForUpgrade: (o: { json: string }) => new Promise((r) => setTimeout(() => r(got.push(o)), delays.shift() ?? 0)),
      },
    },
  }
  return got
}

test("the export holds the core's records and the app's own settings, not what it can make again", async () => {
  localStorage.setItem('inyon.hubs.v1', '[{"id":"h1","name":"home"}]')
  localStorage.setItem('inyon.settings.v2', '{"theme":"dark"}')
  localStorage.setItem('inyon.overview.v1', '{"hosts":[]}')
  localStorage.setItem('inyon.chat.v1.box %3', '{}')
  localStorage.setItem('inyon.chats.v1', '[]')
  localStorage.setItem('someone-else', 'x')
  const records = new Map([
    ['hosts', '[{"id":"laptop","kind":"link"}]'],
    ['names', '{"laptop $1":"api"}'],
    ['chats/laptop %3', '{"items":[]}'],
  ])
  const e = await collectExport(records, localStorage, 1_800_000_000_000)
  expect(e).toEqual({
    v: 1,
    exportedAt: 1_800_000_000_000,
    appVersion: VERSION,
    records: { hosts: '[{"id":"laptop","kind":"link"}]', names: '{"laptop $1":"api"}' },
    local: { 'inyon.hubs.v1': '[{"id":"h1","name":"home"}]', 'inyon.settings.v2': '{"theme":"dark"}' },
  })
  // the secrets are the native side's to add; none come from here
  expect(JSON.stringify(e)).not.toContain('secrets')
})

test('exports run one at a time, and a later one reads the records after the earlier one is written', async () => {
  // the first write is slow; the second must still land after it, with what was there by then
  const got = android([40, 0])
  let n = 0
  const read = async () => new Map([['hosts', JSON.stringify(++n)]])
  await Promise.all([exportForUpgrade(read), exportForUpgrade(read)])
  expect(got.map((o) => JSON.parse(o.json).records.hosts)).toEqual(['1', '2'])
})

test('a failed export is logged and the next one still runs', async () => {
  const got = android()
  const errors: unknown[] = []
  const was = console.error
  console.error = (...a: unknown[]) => void errors.push(a)
  try {
    await exportForUpgrade(async () => {
      throw new Error('IndexedDB is closed')
    })
    await exportForUpgrade(async () => new Map())
  } finally {
    console.error = was
  }
  expect(errors).toHaveLength(1)
  expect(got).toHaveLength(1)
})

test('only the Android app exports, at start and each time it goes to the background', async () => {
  ;(window as any).Capacitor = { isNativePlatform: () => false }
  let reads = 0
  const read = async () => (reads++, new Map<string, string>())
  startUpgradeExport(read)
  await exportForUpgrade(read)
  expect(reads).toBe(0)

  const got = android()
  startUpgradeExport(read)
  await exportForUpgrade(async () => new Map())
  expect(got).toHaveLength(2)
  Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true })
  document.dispatchEvent(new Event('visibilitychange'))
  await exportForUpgrade(async () => new Map())
  Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true })
  expect(got).toHaveLength(4)
})

test('the export is what the app from 1.0 reads back (fixtures/upgrade/export-v1.json)', async () => {
  const fixture = await Bun.file(new URL('../../../../fixtures/upgrade/export-v1.json', import.meta.url)).json()
  for (const [k, v] of Object.entries(fixture.local as Record<string, string>)) localStorage.setItem(k, v)
  localStorage.setItem('inyon.overview.v1', '{"hosts":[]}')
  const records = new Map(Object.entries(fixture.records as Record<string, string>))
  records.set('chats/box %3', '{"items":[]}')
  const { secrets: _added, ...collected } = fixture
  // the Kotlin side adds the secrets; the version is whichever build wrote it
  expect(await collectExport(records, localStorage, fixture.exportedAt)).toEqual({ ...collected, appVersion: VERSION })
})
