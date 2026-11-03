import type { Terminal } from '@xterm/xterm'
import { setRenderer } from '../../lib/renderer.ts'

type WebglModule = Pick<typeof import('@xterm/addon-webgl'), 'WebglAddon'>

/**
 * Draws an opened terminal with WebGL, which is far cheaper than the DOM renderer on a phone. The
 * addon comes in its own chunk once the terminal is up. Without WebGL (activating throws), when
 * the browser drops the context later, or when it draws at the wrong scale, xterm's default
 * renderer takes over again. `changed` runs after a switch, since the cells may change size.
 */
export function attachWebgl(term: Terminal, load: () => Promise<WebglModule> = () => import('@xterm/addon-webgl'), changed: () => void = () => {}) {
  let closed = false
  let addon: { dispose(): void } | undefined
  const done = load().then(
    async ({ WebglAddon }) => {
      if (closed) return
      const a = new WebglAddon()
      const fallBack = () => {
        a.dispose()
        addon = undefined
        setRenderer('dom')
        changed()
      }
      a.onContextLoss(fallBack)
      try {
        term.loadAddon(a)
        addon = a
        setRenderer('webgl')
      } catch {
        setRenderer('dom')
        return
      }
      changed()
      // Some browsers (software WebGL at a 2x screen, at least) size the canvas in CSS pixels once
      // the terminal refits, and draw every cell twice as big: a few giant lines
      await frames(2)
      if (!closed && addon === a && !atScreenScale(term)) fallBack()
    },
    () => setRenderer('dom'),
  )
  return {
    /** settled once the renderer is decided (tests) */
    done,
    dispose() {
      closed = true
      addon?.dispose()
      addon = undefined
    },
  }
}

const frames = (n: number): Promise<void> =>
  n <= 0 ? Promise.resolve() : new Promise((r) => (globalThis.requestAnimationFrame ?? ((f: () => void) => setTimeout(f, 16)))(() => r(frames(n - 1))))

/** Whether the WebGL canvas has a pixel for each screen pixel, as a sharp drawing needs. */
export function atScreenScale(term: Pick<Terminal, 'element'>, ratio = globalThis.devicePixelRatio ?? 1) {
  const canvas = [...(term.element?.querySelectorAll('canvas') ?? [])].find((c) => !c.classList.contains('xterm-link-layer') && c.clientWidth > 0)
  if (!canvas || ratio < 1.5) return true
  return canvas.width / canvas.clientWidth > ratio * 0.75
}
