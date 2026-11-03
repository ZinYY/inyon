import type { PhoneRuntime, PhoneStore, SshPlugin } from '@inyon/phone'
import { isNative, plugin } from './native.ts'
import { isHosted } from './base.ts'
import { deviceName } from './deviceName.ts'

/**
 * The phone app has no hub: it runs Inyon's core here, in the app, and SSHes into your servers
 * through the native InyonSsh plugin. The web app on GitHub Pages runs the same core in the page,
 * without SSH. The core is a separate chunk a hub's browser never fetches, and `api.ts` and
 * `socket.ts` call it instead of the network.
 */
export const isPhone = () => isNative() || isHosted()

/** Whether this device can SSH into a server itself (the Android app can; a browser can't). */
export const canSsh = isNative

/**
 * Adds the phone's key to authorized_keys once, with the permissions sshd insists on. A file whose
 * last line has no newline after it gets one first, or the key would be glued onto that line.
 */
export const authorizeCommand = (key: string) =>
  `mkdir -p ~/.ssh && chmod 700 ~/.ssh && { grep -qxF '${key}' ~/.ssh/authorized_keys 2>/dev/null || { [ -s ~/.ssh/authorized_keys ] && [ -n "$(tail -c1 ~/.ssh/authorized_keys)" ] && echo >> ~/.ssh/authorized_keys; echo '${key}' >> ~/.ssh/authorized_keys; }; } && chmod 600 ~/.ssh/authorized_keys`

let runtime: Promise<PhoneRuntime> | undefined

export function phone(): Promise<PhoneRuntime> {
  return (runtime ??= (async () => {
    const ssh = (plugin('InyonSsh') as unknown as SshPlugin | undefined) ?? (isHosted() ? (await import('./browserSsh.ts')).browserSsh() : undefined)
    if (!ssh) throw new Error('This build of the app is missing its SSH support')
    const [store, { startPhone }, name] = await Promise.all([idbStore(), import('@inyon/phone'), deviceName()])
    // the computers this phone pairs with list it by this name
    const rt = await startPhone({ store, ssh, name })
    // a new network (Wi-Fi to mobile data, a VPN switching) breaks the links: reconnect right away
    let soon: ReturnType<typeof setTimeout> | undefined
    window.addEventListener('online', () => {
      clearTimeout(soon)
      soon = setTimeout(() => rt.resume(), 300)
    })
    document.addEventListener('visibilitychange', () => rt.background(document.visibilityState === 'hidden'))
    return rt
  })())
}

const openKv = () =>
  new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open('inyon', 1)
    req.onupgradeneeded = () => req.result.createObjectStore('kv')
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })

/** Every record as the store keeps it: key to JSON text. */
const readKv = (db: IDBDatabase) =>
  new Promise<Map<string, string>>((resolve, reject) => {
    const data = new Map<string, string>()
    const req = db.transaction('kv').objectStore('kv').openCursor()
    req.onsuccess = () => {
      const cur = req.result
      if (!cur) return resolve(data)
      data.set(String(cur.key), cur.value as string)
      cur.continue()
    }
    req.onerror = () => reject(req.error)
  })

/** The core's records as they are on disk now, for the upgrade export (upgradeExport.ts). */
export async function kvRecords() {
  const db = await openKv()
  try {
    return await readKv(db)
  } finally {
    db.close()
  }
}

/**
 * The core's records (server list, settings, chat caches) in IndexedDB. The core reads them
 * synchronously, so they're all loaded into memory first and written back as they change.
 */
async function idbStore(): Promise<PhoneStore> {
  const db = await openKv()
  const data = await readKv(db)
  // writes queue in order on their own transactions; a failed one only loses that record
  const write = (fn: (s: IDBObjectStore) => void) => {
    try {
      fn(db.transaction('kv', 'readwrite').objectStore('kv'))
    } catch (err) {
      console.error('phone store:', err)
    }
  }
  return {
    get: (key) => {
      const raw = data.get(key)
      return raw === undefined ? undefined : JSON.parse(raw)
    },
    set: (key, value) => {
      const raw = JSON.stringify(value)
      data.set(key, raw)
      write((s) => s.put(raw, key))
    },
    del: (key) => {
      data.delete(key)
      write((s) => s.delete(key))
    },
    keys: (prefix) => [...data.keys()].filter((k) => k.startsWith(prefix)),
  }
}

/**
 * The phone's stand-in for the hub's WebSocket: same shape as far as `socket.ts` cares, but the
 * messages go straight into the core.
 */
export class PhoneSocket {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 3
  readyState = PhoneSocket.CONNECTING
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  private conn?: { message(raw: string): void; close(): void }

  constructor() {
    phone().then(
      (rt) => {
        if (this.readyState === PhoneSocket.CLOSED) return
        // deliver on a later task, as a socket would: a reply must not run inside the send that caused it
        this.conn = rt.connect((msg) => {
          const data = JSON.stringify(msg)
          setTimeout(() => this.readyState === PhoneSocket.OPEN && this.onmessage?.({ data }))
        })
        this.readyState = PhoneSocket.OPEN
        this.onopen?.()
      },
      (err) => {
        console.error('phone:', err)
        this.close()
      },
    )
  }

  send(raw: string) {
    if (this.readyState === PhoneSocket.OPEN) this.conn?.message(raw)
  }

  close() {
    if (this.readyState === PhoneSocket.CLOSED) return
    this.readyState = PhoneSocket.CLOSED
    this.conn?.close()
    this.onclose?.()
  }
}
