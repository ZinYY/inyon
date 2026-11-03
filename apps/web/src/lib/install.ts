import { useSyncExternalStore } from 'react'

/**
 * Installing the web app (Add to Home Screen). Chrome and Edge offer an install prompt the page
 * can hold on to and show from its own button; Safari on iPhone and iPad only installs from the
 * Share menu, so there the app explains where to tap.
 */
interface InstallPrompt extends Event {
  prompt(): Promise<void>
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>
}

let held: InstallPrompt | undefined
const listeners = new Set<() => void>()
const changed = () => listeners.forEach((l) => l())

export function watchInstall() {
  window.addEventListener('beforeinstallprompt', (e) => {
    e.preventDefault()
    held = e as InstallPrompt
    changed()
  })
  window.addEventListener('appinstalled', () => {
    held = undefined
    changed()
  })
}

/** Opened from the Home Screen or as an installed app, not in a browser tab. */
export const standalone = () => matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true

/** Safari on an iPhone or iPad (iPadOS reports itself as a Mac with a touch screen). */
export const appleMobile = () => /iPhone|iPad|iPod/.test(navigator.userAgent) || (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1)

/** How this browser can install the app: its own prompt, the Share menu, or not at all. */
export const useInstallWay = () =>
  useSyncExternalStore(
    (fn) => (listeners.add(fn), () => void listeners.delete(fn)),
    (): 'prompt' | 'share' | undefined => (standalone() ? undefined : held ? 'prompt' : appleMobile() ? 'share' : undefined),
  )

/** Shows the browser's install prompt; true when it was accepted. */
export async function install() {
  const p = held
  if (!p) return false
  held = undefined
  changed()
  await p.prompt()
  return (await p.userChoice).outcome === 'accepted'
}

/**
 * Asks the browser not to clear this site's data when space runs low. Browsers decide for
 * themselves (an installed app, or one used often, usually gets it); asking costs nothing.
 */
export const keepData = () => void navigator.storage?.persist?.().catch(() => false)

const DISMISSED = 'inyon.install-dismissed'
export function installDismissed() {
  try {
    return localStorage.getItem(DISMISSED) === '1'
  } catch {
    return false
  }
}
export function dismissInstall() {
  try {
    localStorage.setItem(DISMISSED, '1')
  } catch {}
}
