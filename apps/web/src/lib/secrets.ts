import { plugin } from './native.ts'

/**
 * Secrets this device keeps: a paired hub's token, the phone's identity key, a server's password.
 * The Android app keeps them in its own store, encrypted under a key in the phone's Keystore, so
 * they never sit in the WebView's storage. A browser keeps them in IndexedDB, sealed with AES-GCM
 * under a key it made non-extractable: the page can use it, but nothing (this page included) can
 * read the key out.
 */
export interface SecretStore {
  get(key: string): Promise<string | null>
  set(key: string, value: string): Promise<void>
  delete(key: string): Promise<void>
}

type NativeSecrets = {
  setSecret(o: { key: string; value: string }): Promise<unknown>
  getSecret(o: { key: string }): Promise<{ value: string | null }>
  deleteSecret(o: { key: string }): Promise<unknown>
}

const DB = 'inyon-secrets'

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1)
    req.onupgradeneeded = () => {
      req.result.createObjectStore('keys')
      req.result.createObjectStore('secrets')
    }
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
}

function run<T>(db: IDBDatabase, store: string, mode: IDBTransactionMode, fn: (s: IDBObjectStore) => IDBRequest): Promise<T> {
  return new Promise((resolve, reject) => {
    const req = fn(db.transaction(store, mode).objectStore(store))
    req.onsuccess = () => resolve(req.result as T)
    req.onerror = () => reject(req.error)
  })
}

let sealing: Promise<{ db: IDBDatabase; key: CryptoKey }> | undefined

/** The database and the sealing key, made on first use. */
const ready = () =>
  (sealing ??= (async () => {
    const db = await open()
    let key = await run<CryptoKey | undefined>(db, 'keys', 'readonly', (s) => s.get('seal'))
    if (!key) {
      key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
      await run(db, 'keys', 'readwrite', (s) => s.put(key, 'seal'))
    }
    return { db, key }
  })())

const enc = new TextEncoder()
const dec = new TextDecoder()

/** The sealed IndexedDB store, for a browser. */
export const sealedStore: SecretStore = {
  async set(key, value) {
    const { db, key: seal } = await ready()
    const iv = crypto.getRandomValues(new Uint8Array(12))
    const data = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, seal, enc.encode(value)))
    await run(db, 'secrets', 'readwrite', (s) => s.put({ iv, data }, key))
  },
  async get(key) {
    const { db, key: seal } = await ready()
    const row = await run<{ iv: Uint8Array<ArrayBuffer>; data: Uint8Array<ArrayBuffer> } | undefined>(db, 'secrets', 'readonly', (s) => s.get(key))
    if (!row) return null
    const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: row.iv }, seal, row.data)
    return dec.decode(plain)
  },
  async delete(key) {
    const { db } = await ready()
    await run(db, 'secrets', 'readwrite', (s) => s.delete(key))
  },
}

/**
 * Where this device keeps secrets: the Android app's store, else the sealed one where the browser
 * has IndexedDB and WebCrypto (a page on plain http has no WebCrypto). Undefined when neither is
 * there.
 */
export function secretStore(): SecretStore | undefined {
  const native = plugin('InyonSsh') as unknown as NativeSecrets | undefined
  if (typeof native?.getSecret === 'function') {
    return {
      get: async (key) => (await native.getSecret({ key })).value ?? null,
      set: async (key, value) => void (await native.setSecret({ key, value })),
      delete: async (key) => void (await native.deleteSecret({ key })),
    }
  }
  if (typeof indexedDB !== 'undefined' && typeof crypto !== 'undefined' && crypto.subtle && (typeof isSecureContext === 'undefined' || isSecureContext)) return sealedStore
  return undefined
}
