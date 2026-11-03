import { useStore } from './store.ts'
import { plugin } from './native.ts'

export type HapticKind = 'tap' | 'select' | 'long' | 'success' | 'warning' | 'error' | 'event'

// one light tick per touch; two pulses only for a confirmed outcome, three for an error
const PATTERNS: Record<HapticKind, number | number[]> = {
  tap: 8,
  select: 8,
  long: 40,
  success: [10, 40, 14],
  warning: [18, 60, 18],
  error: [28, 50, 28, 50, 28],
  event: [60, 100, 90],
}

let iosSwitch: HTMLLabelElement | null = null
const isIOS = () => typeof navigator !== 'undefined' && /iP(hone|ad|od)/.test(navigator.userAgent)

/**
 * Haptic feedback. The Android app plays it natively (InyonDevice); app builds from before that
 * use Capacitor's Haptics plugin, as does the iPhone app (Taptic Engine). In a browser, Android
 * implements the Vibration API; iOS Safari doesn't, but toggling a native
 * `<input type="checkbox" switch>` produces the system haptic (iOS 17.4+) inside a user gesture.
 */
export function haptic(kind: HapticKind = 'tap') {
  if (!useStore.getState().settings.haptics) return
  const device = plugin('InyonDevice')
  if (typeof device?.haptic === 'function') {
    let run: Promise<unknown>
    try {
      run = Promise.resolve(device.haptic({ kind }))
    } catch (e) {
      run = Promise.reject(e)
    }
    run.catch(() => capacitorHaptic(kind) || webHaptic(kind))
    return
  }
  if (!capacitorHaptic(kind)) webHaptic(kind)
}

/** Capacitor's Haptics plugin; false when the app doesn't have it. */
function capacitorHaptic(kind: HapticKind) {
  const native = plugin('Haptics')
  if (!native) return false
  try {
    const run =
      kind === 'tap' ? native.impact({ style: 'LIGHT' })
      : kind === 'select' ? native.selectionChanged()
      : kind === 'long' ? native.impact({ style: 'HEAVY' })
      : native.notification({ type: kind === 'success' ? 'SUCCESS' : kind === 'error' ? 'ERROR' : 'WARNING' })
    run?.catch(() => {})
  } catch {}
  return true
}

function webHaptic(kind: HapticKind) {
  try {
    if ('vibrate' in navigator && !isIOS()) {
      navigator.vibrate(PATTERNS[kind])
      return
    }
    if (isIOS()) {
      if (!iosSwitch) {
        iosSwitch = document.createElement('label')
        iosSwitch.ariaHidden = 'true'
        iosSwitch.style.cssText = 'position:fixed;left:-100px;top:0;width:1px;height:1px;opacity:0;pointer-events:none'
        const input = document.createElement('input')
        input.type = 'checkbox'
        input.setAttribute('switch', '')
        iosSwitch.appendChild(input)
        document.body.appendChild(iosSwitch)
      }
      iosSwitch.click()
      if (kind !== 'tap' && kind !== 'select' && kind !== 'long') setTimeout(() => iosSwitch?.click(), 90)
    }
  } catch {}
}

/**
 * Haptics on every button press in the app (light tap), installed once. It fires on the click, not
 * on touch-down: a finger that lands on a tool row or link to scroll the chat isn't a press.
 */
export function installTapHaptics() {
  let touch = 0
  document.addEventListener('pointerdown', (e) => void (touch = e.pointerType === 'touch' ? Date.now() : 0), { passive: true, capture: true })
  document.addEventListener(
    'click',
    (e) => {
      // only taps (a click soon after a touch), never mouse clicks or keyboard presses
      if (!touch || Date.now() - touch > 800) return
      touch = 0
      const el = (e.target as HTMLElement)?.closest?.('button, a, [role="switch"], [role="radio"], [role="tab"], [role="option"], [role="menuitem"]')
      if (el && !(el as HTMLButtonElement).disabled) haptic('tap')
    },
    { passive: true, capture: true },
  )
}
