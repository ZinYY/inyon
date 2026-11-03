import { VERSION } from './version.ts'
import { platform, plugin } from './native.ts'

/**
 * The next Inyon app for Android is written in Rust and installs over this one. It can't read
 * this WebView's storage, so the app hands what's there to the native InyonMigrate plugin, which
 * adds the secrets (the phone's key, hub tokens, server sign-ins) from the native secret store and
 * seals it all into a file only an app with this app id and signing key can open
 * (MigrationExport.kt). It runs on every start and every time the app goes to the background, so
 * the file is never older than the last time the app was open.
 */
export const EXPORT_VERSION = 1

/** Records the app can make again, left out of the export: chat caches and snapshots for the first frame. */
const CACHED_RECORDS = ['chats/']
const CACHED_LOCAL = ['inyon.chat.', 'inyon.chats.', 'inyon.overview.']

export interface UpgradeExport {
  v: number
  /** ms since the epoch */
  exportedAt: number
  appVersion: string
  /** the core's records (IndexedDB "inyon" / kv): key to JSON text, as the store keeps them */
  records: Record<string, string>
  /** this app's own localStorage keys (inyon.*): paired hubs, settings */
  local: Record<string, string>
}

export async function collectExport(records: Map<string, string>, storage: Storage = localStorage, now = Date.now()): Promise<UpgradeExport> {
  const out: UpgradeExport = { v: EXPORT_VERSION, exportedAt: now, appVersion: VERSION, records: {}, local: {} }
  for (const [k, v] of records) if (!CACHED_RECORDS.some((p) => k.startsWith(p))) out.records[k] = v
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i)
    if (!k?.startsWith('inyon.') || CACHED_LOCAL.some((p) => k.startsWith(p))) continue
    const v = storage.getItem(k)
    if (v !== null) out.local[k] = v
  }
  return out
}

let last: Promise<void> = Promise.resolve()

/**
 * Writes the export now. One at a time, in order: each reads the records only once the one
 * before it is written, so an older picture never lands after a newer one.
 */
export function exportForUpgrade(read: () => Promise<Map<string, string>> = async () => (await import('./phone.ts')).kvRecords()): Promise<void> {
  const migrate = plugin('InyonMigrate')
  if (!migrate) return last
  last = last
    .then(async () => {
      await migrate.exportForUpgrade({ json: JSON.stringify(await collectExport(await read())) })
    })
    .catch((err) => console.error('upgrade export:', err))
  return last
}

/** On the Android app: export now and each time the app goes to the background. */
export function startUpgradeExport(read?: () => Promise<Map<string, string>>) {
  if (platform() !== 'android') return
  void exportForUpgrade(read)
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') void exportForUpgrade(read)
  })
}
