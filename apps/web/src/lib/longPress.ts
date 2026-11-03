import { useRef } from 'react'
import { haptic } from './haptics.ts'

/**
 * Long-press on a touch screen: half a second without moving runs `fn` with a firm buzz, and the tap
 * that ends the press doesn't also open what's under it. The browser's own long-press menu (copy
 * link and the like) stays out of the way.
 */
export function useLongPress(fn: () => void, ms = 500) {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const start = useRef<{ x: number; y: number } | undefined>(undefined)
  const fired = useRef(false)
  const cancel = () => {
    clearTimeout(timer.current)
    start.current = undefined
  }
  return {
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType !== 'touch') return
      fired.current = false
      start.current = { x: e.clientX, y: e.clientY }
      timer.current = setTimeout(() => {
        fired.current = true
        haptic('long')
        fn()
      }, ms)
    },
    onPointerMove: (e: React.PointerEvent) => {
      // a scroll, not a press
      if (start.current && Math.hypot(e.clientX - start.current.x, e.clientY - start.current.y) > 8) cancel()
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    onContextMenu: (e: React.MouseEvent) => {
      if (fired.current || start.current) e.preventDefault()
    },
    onClickCapture: (e: React.MouseEvent) => {
      if (fired.current) {
        e.preventDefault()
        e.stopPropagation()
        fired.current = false
      }
    },
  }
}
