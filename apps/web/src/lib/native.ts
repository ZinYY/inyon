/**
 * The Inyon phone apps run this same web app inside a native shell (Capacitor). The shell injects
 * `window.Capacitor`, whose `Plugins` proxy calls straight into native code, so the web app needs
 * no extra packages and does nothing different in a normal browser.
 */
import { backHandled } from './back.ts'
import { appHref } from './base.ts'
import { useSyncExternalStore } from 'react'
import type { Power } from './connlog.ts'

type Plugin = Record<string, (...args: any[]) => Promise<any>>

const cap = () => (window as any).Capacitor as { isNativePlatform?: () => boolean; getPlatform?: () => string; Plugins?: Record<string, Plugin> } | undefined

export const isNative = () => !!cap()?.isNativePlatform?.()
export const platform = () => (isNative() ? cap()!.getPlatform!() : 'web') as 'ios' | 'android' | 'web'
export const plugin = (name: string): Plugin | undefined => (isNative() ? cap()?.Plugins?.[name] : undefined)

const go = (href: string) => {
  if (location.pathname + location.search === appHref(href)) return
  window.history.pushState({}, '', appHref(href))
  window.dispatchEvent(new PopStateEvent('popstate'))
}

// an inyon://pair link (a laptop's code scanned with the camera) opens the pairing sheet; tether:// is
// how a laptop from before the rename to Inyon prints it
const openUrl = (url: string | undefined) => {
  if (url && /^(inyon|tether):\/\/pair\?/.test(url)) go(`/machines?pair=${encodeURIComponent(url)}`)
  // a hub's code, scanned with the camera: pair this phone with the hub
  else if (url && /^inyon:\/\/hub\?/.test(url)) go(`/machines?hub=${encodeURIComponent(url)}`)
}

// whether the Android WebView keeps Capacitor's bridge to the app's own top frame; unknown until
// the app answers, and treated as no until then
let isolated: boolean | undefined
const isolationHeard = new Set<() => void>()
const setIsolated = (v: boolean) => {
  isolated = v
  for (const fn of isolationHeard) fn()
}

/**
 * Whether a frame may show another site's page (an agent's page under /v/, a dev server preview).
 * A browser has no native bridge to reach. The Android app's WebView keeps the bridge to the app's
 * own top frame when it supports WebMessageListener; an older one exposes it to every frame, so
 * there a page opens in the browser instead.
 */
export const framesIsolated = () => !isNative() || isolated === true
export const useFramesIsolated = () =>
  useSyncExternalStore(
    (fn) => (isolationHeard.add(fn), () => void isolationHeard.delete(fn)),
    framesIsolated,
    framesIsolated,
  )

let fragmentLink: string | undefined

/**
 * The web app opened from a computer's QR code: `https://…/#pair?m=…`. The code rides after `#`,
 * which browsers never send to the host; it comes out of the address bar before the app starts, so
 * it isn't left in history or sent anywhere on a reload. The pairing sheet then offers it.
 */
export function takePairFragment() {
  const kind = location.hash.startsWith('#pair?') ? 'pair' : location.hash.startsWith('#hub?') ? 'hub' : undefined
  if (!kind) return
  fragmentLink = `inyon://${kind}?${location.hash.slice(kind.length + 2)}`
  window.history.replaceState(null, '', appHref('/machines'))
}
export const pairFragment = () => fragmentLink
export const forgetPairFragment = () => void (fragmentLink = undefined)

/**
 * The Android app tells Chromium its page is always on screen, so the page isn't frozen a minute
 * into the background (InyonWebView). The page's own visibility then always says visible, so it
 * follows the app's instead: hidden while Inyon is off screen, with a visibilitychange each time.
 */
function followAppVisibility(device: ReturnType<typeof plugin>) {
  if (typeof device?.addListener !== 'function') return
  let visible = true
  let heard = false
  const set = (v: unknown) => {
    if (typeof v !== 'boolean' || v === visible) return
    visible = v
    document.dispatchEvent(new Event('visibilitychange'))
  }
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (visible ? 'visible' : 'hidden') })
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => !visible })
  device.addListener('appVisibility', (e: { visible?: boolean }) => {
    heard = true
    set(e?.visible)
  })
  // started behind the lock screen, say, the app went on and off screen before this page listened;
  // an event that comes before the answer is newer than it
  Promise.resolve(device.appVisibility?.())
    .then((r: { visible?: boolean } | undefined) => heard || set(r?.visible))
    .catch(() => {})
}

/** Native behaviour for the phone apps: back button, notification taps, deep links, status bar. */
export function installNative() {
  if (!isNative()) return
  document.documentElement.dataset.native = platform()
  if (platform() === 'android') followAppVisibility(plugin('InyonDevice'))
  // Android's back button closes whatever is open on top first, then walks back through the app,
  // and leaves it from the first screen
  plugin('App')?.addListener('backButton', ({ canGoBack }: { canGoBack: boolean }) => {
    if (backHandled()) return
    if (canGoBack && window.history.length > 1) window.history.back()
    else plugin('App')?.exitApp()
  })
  // tapping a notification opens the session it's about: an event while the app runs, a stored
  // link when the tap started the app
  const device = plugin('InyonDevice')
  void Promise.resolve(device?.framesIsolated?.())
    .then((r: { isolated?: boolean } | undefined) => setIsolated(r?.isolated === true))
    .catch(() => setIsolated(false))
  device?.addListener('pushOpened', ({ href }: { href?: string }) => href && go(href))
  // an answer tapped on a notification, sent from here
  device?.addListener('notificationAnswer', (a: NotificationAnswer) => void import('./notifyAnswer.ts').then(({ answerFromNotification }) => answerFromNotification(a)))
  device
    ?.takeLaunchHref?.()
    .then((r: { href?: string | null } | undefined) => r?.href && go(r.href))
    .catch(() => {})
  plugin('StatusBar')?.setOverlaysWebView({ overlay: true }).catch(() => {})
  plugin('App')?.addListener('appUrlOpen', ({ url }: { url: string }) => openUrl(url))
  plugin('App')
    ?.getLaunchUrl?.()
    .then((r: { url?: string } | undefined) => openUrl(r?.url))
    .catch(() => {})
  // back from the background: sockets may have died while the phone slept, so check them now
  const resume = () => {
    if (document.visibilityState === 'visible') import('./phone.ts').then(({ phone }) => phone()).then((r) => r.resume(), () => {})
  }
  plugin('App')?.addListener('resume', resume)
  document.addEventListener('visibilitychange', resume)
}

/** Keeps the status bar text readable on the current theme. */
export function nativeTheme(theme: 'light' | 'dark') {
  plugin('StatusBar')?.setStyle({ style: theme === 'dark' ? 'DARK' : 'LIGHT' }).catch(() => {})
}

/** An answer tapped on a notification's button. */
export interface NotificationAnswer {
  host: string
  pane: string
  option: number
  label: string
  tag: string
  /** when it was tapped (ms); the app may have been frozen since */
  at?: number
}

/** A question's answers, offered as the notification's buttons (one to three). */
export interface NotifyAnswers {
  host: string
  pane: string
  options: { n: number; label: string }[]
}

/**
 * A notification from the Android app, with Inyon's logo and buttons that fit `kind` (waiting,
 * done, offline, online). `tag` names the session or machine: its next notification replaces this
 * one, and opening it clears it. False where the app can't post notifications itself.
 */
export async function nativeNotify(title: string, body: string, href: string, tag?: string, kind?: string, answers?: NotifyAnswers) {
  const device = plugin('InyonDevice')
  if (typeof device?.postNotification !== 'function') return false
  try {
    const r = await device.postNotification({ title, body, href, tag, kind, ...answers })
    return r?.posted !== false
  } catch {
    return false
  }
}

/** You're looking at it now: take its notification away. */
export function clearNativeNotification(tag: string) {
  void Promise.resolve(plugin('InyonDevice')?.clearNotification?.({ tag })).catch(() => {})
}

/** Asks for permission to notify; undefined where the app can't post notifications itself. */
export async function askNativeNotifications() {
  const device = plugin('InyonDevice')
  if (typeof device?.requestNotificationPermission !== 'function') return undefined
  try {
    return !!(await device.requestNotificationPermission())?.granted
  } catch {
    return false
  }
}

/** Whether the phone app may post notifications (permission given and not turned off for Inyon). */
export async function nativeNotificationsOn(): Promise<boolean | undefined> {
  const device = plugin('InyonDevice')
  if (typeof device?.notificationsEnabled !== 'function') return undefined
  try {
    return !!(await device.notificationsEnabled())?.enabled
  } catch {
    return undefined
  }
}

/**
 * Clipboard text. Android's WebView refuses navigator.clipboard.readText(), so the Android app
 * reads it natively; elsewhere it's the browser's clipboard API, which may ask first or refuse.
 */
export async function readClipboard(): Promise<string> {
  const device = plugin('InyonDevice')
  if (typeof device?.readClipboardText === 'function') {
    try {
      return ((await device.readClipboardText()) as { text?: string } | undefined)?.text ?? ''
    } catch {}
  }
  if (!navigator.clipboard?.readText) throw new Error('Reading the clipboard is not supported here')
  return navigator.clipboard.readText()
}

/** A file the phone app handed over (keyboard image, clipboard), as base64. */
export interface NativeFile {
  name: string
  type: string
  data: string
}

export const nativeFile = (f: NativeFile) => new File([Uint8Array.from(atob(f.data), (c) => c.charCodeAt(0))], f.name || 'pasted', { type: f.type || 'application/octet-stream' })

/** Listens to a native plugin event; returns a function that stops listening. */
export function onNative<T>(name: string, event: string, fn: (e: T) => void) {
  const handle = plugin(name)?.addListener(event, fn) as Promise<{ remove(): void }> | { remove(): void } | undefined
  return () => void Promise.resolve(handle).then((h) => h?.remove())
}

/**
 * Android's Doze, battery limits and Stay connected (InyonDevice.powerState), for the connection
 * log and "Can't connect?". Undefined outside the Android app or on an app without it.
 */
export async function nativePower(): Promise<Power | undefined> {
  const device = plugin('InyonDevice')
  if (typeof device?.powerState !== 'function') return undefined
  try {
    return ((await device.powerState()) as Power | undefined) ?? undefined
  } catch {
    return undefined
  }
}
