import { useSyncExternalStore } from 'react'
import { api } from './api.ts'
import { socket } from './socket.ts'
import { COMMIT, VERSION } from './version.ts'
import { isHosted, publicFile } from './base.ts'

/**
 * What the hub runs, next to what this page runs. A browser keeps the old app until it reloads,
 * so after the hub is updated or restarted with new code, the app offers a reload. The web app on
 * GitHub Pages has no hub: there, the newest build is the one published next to it.
 */
export interface Build {
  version: string
  commit?: string
}

/** This page's own code. */
export const bundle: Build = { version: VERSION, commit: COMMIT || undefined }

/** The same version, and the same commit when both know theirs (short hashes may differ in length). */
export function sameBuild(a: Build, b: Build) {
  if (a.version !== b.version) return false
  if (!a.commit || !b.commit) return true
  return a.commit.startsWith(b.commit) || b.commit.startsWith(a.commit)
}

let hub: Build | undefined
const listeners = new Set<() => void>()
const subscribe = (fn: () => void) => (listeners.add(fn), () => void listeners.delete(fn))

/** The build the web app's host serves now (`version.json`, written by the build). */
async function published(): Promise<{ version?: string; commit?: string }> {
  const res = await fetch(publicFile('version.json'), { cache: 'no-store' })
  return res.ok ? res.json() : {}
}

/** Asks the hub what it runs now. */
export async function checkHub() {
  try {
    const s = isHosted() ? await published() : await api.session()
    if (!s.version) return
    const next = { version: s.version, commit: s.commit || undefined }
    if (hub?.version === next.version && hub.commit === next.commit) return
    hub = next
    listeners.forEach((l) => l())
  } catch {}
}

let watching = 0
let stopWatching: (() => void) | undefined
/** Checks each time the socket connects: a hub that restarted comes back on a new connection. */
export function watchHub() {
  if (watching++ === 0) {
    const stop = socket.onStatus((up) => up && void checkHub())
    // the web app's socket never drops, so it looks again each time it comes back into view
    const back = () => document.visibilityState === 'visible' && void checkHub()
    if (isHosted()) document.addEventListener('visibilitychange', back)
    stopWatching = () => (stop(), document.removeEventListener('visibilitychange', back))
    if (socket.up || isHosted()) void checkHub()
  }
  return () => {
    if (--watching === 0) stopWatching?.()
  }
}

/** The hub's build, once known. */
export const hubBuild = () => hub
export const useHubBuild = () => useSyncExternalStore(subscribe, () => hub)

/** For tests: forget what the hub said. */
export const forgetHub = () => void (hub = undefined)

const SEEN = 'inyon.reload-seen'
const key = (b: Build) => `${b.version}+${b.commit ?? ''}`

/** Whether to offer a reload: the hub runs other code, and this build wasn't offered before. */
export function reloadDue(h: Build | undefined): h is Build {
  if (!h || sameBuild(h, bundle)) return false
  try {
    return localStorage.getItem(SEEN) !== key(h)
  } catch {
    return true
  }
}

export function markReloadSeen(h: Build) {
  try {
    localStorage.setItem(SEEN, key(h))
  } catch {}
}
