/**
 * Inyon was called Tether: settings, drafts and choices this browser kept under "tether." keys
 * are copied to their "inyon." names once. Imported before anything reads them.
 */
export function copyOldKeys(storage: Storage = localStorage) {
  try {
    for (const k of Object.keys(storage)) {
      if (!k.startsWith('tether.')) continue
      const key = `inyon.${k.slice('tether.'.length)}`
      if (storage.getItem(key) === null) storage.setItem(key, storage.getItem(k)!)
    }
  } catch {}
}

copyOldKeys()
