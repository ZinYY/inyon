import { useEffect, useRef } from 'react'

/**
 * The Android back button closes whatever is open on top (a picker over a sheet, a sheet over a
 * page) before it goes back a page. native.ts fires a cancelable `inyon:back`; the newest open
 * layer handles it.
 */
const layers: { close: () => void }[] = []
let installed = false

function install() {
  if (installed || typeof window === 'undefined') return
  installed = true
  window.addEventListener('inyon:back', (e) => {
    const top = layers.at(-1)
    if (!top) return
    e.preventDefault()
    top.close()
  })
}

/** While `open`, back closes this layer. */
export function useBackCloses(open: boolean, close: () => void) {
  const latest = useRef(close)
  latest.current = close
  useEffect(() => {
    if (!open) return
    install()
    const layer = { close: () => latest.current() }
    layers.push(layer)
    return () => {
      const i = layers.indexOf(layer)
      if (i >= 0) layers.splice(i, 1)
    }
  }, [open])
}

/** Asks the open layers to close; true when one did, so back shouldn't navigate. */
export const backHandled = () => !window.dispatchEvent(new CustomEvent('inyon:back', { cancelable: true }))
