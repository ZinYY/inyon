import { listenForAnswers, listenForGo, show } from './frames.ts'
import { startShim } from './shim.ts'
import { TAG, asMsg, isVirtual } from './protocol.ts'
import type { ShimConfig } from './prepare.ts'

/**
 * The one script of the viewer, built into pageview.html (see build.ts). In the loader it waits
 * for the app to say which page to show; in a document of the page (where prepare.ts put it, with
 * the document's address in `data-inyon`) it is the shim. Either way it takes its own source along,
 * to put first in every document it shows in turn.
 */

const me = document.currentScript as HTMLScriptElement | null
const source = me?.textContent ?? ''
const tag = me?.getAttribute('data-inyon')

if (tag) {
  me!.remove()
  startShim(JSON.parse(tag) as ShimConfig, source)
} else startLoader()

/** The loader: an opaque origin (a sandboxed frame without allow-same-origin) inside the app, or nothing at all. */
function startLoader() {
  const frame = document.getElementById('page') as HTMLIFrameElement | null
  if (!frame || self.origin !== 'null' || window.parent === window) {
    document.body.textContent = 'Pages open inside Inyon.'
    return
  }
  listenForAnswers()
  listenForGo(source)
  let theme: string | undefined
  const tell = () => theme && frame.contentWindow?.postMessage({ type: 'theme', value: theme }, '*')
  addEventListener('message', (e) => {
    if (e.source === window.parent) {
      const m = asMsg(e.data)
      if (m?.t === 'open' && typeof m.url === 'string' && isVirtual(m.url)) {
        theme = m.theme
        void show(frame, m.url, source)
        return
      }
      // the app's theme changed: the page hears it from its own parent, as on the hub
      const d = e.data as { type?: unknown; value?: unknown } | null
      if (d?.type === 'theme' && typeof d.value === 'string') {
        theme = d.value
        tell()
      }
    } else if (e.source === frame.contentWindow && (e.data as { type?: unknown } | null)?.type === 'theme?') tell()
  })
  window.parent.postMessage({ tag: TAG, t: 'ready' }, '*')
}
