import type { SshPlugin } from '@inyon/phone'
import { sealedStore } from './secrets.ts'

/**
 * The web app's stand-in for the Android app's SSH plugin. A browser can't open SSH connections,
 * so the core reaches computers only through `inyon pair` links, and a server goes through a
 * paired computer. What it does keep is the secret store: the phone's identity key lives here.
 *
 * Secrets sit in IndexedDB, sealed with AES-GCM under a key the browser made non-extractable
 * (secrets.ts).
 */
export const NO_SSH = 'A browser can’t connect over SSH. Add the server through a paired computer, or use the Android app.'

const refuse = () => Promise.reject(new Error(NO_SSH))

export function browserSsh(): SshPlugin {
  return {
    probe: refuse,
    connect: refuse,
    disconnect: async () => undefined,
    exec: refuse,
    stream: refuse,
    streamKill: async () => undefined,
    pty: refuse,
    ptyWrite: refuse,
    ptyResize: async () => undefined,
    ptyKill: async () => undefined,
    setSecret: ({ key, value }) => sealedStore.set(key, value),
    getSecret: async ({ key }) => ({ value: await sealedStore.get(key) }),
    deleteSecret: ({ key }) => sealedStore.delete(key),
    // nothing streams without SSH
    addListener: () => ({ remove() {} }),
  } as SshPlugin
}
