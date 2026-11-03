import { asDocument } from './prepare.ts'
import { TAG, asMsg, isVirtual, type Got, type Load } from './protocol.ts'

/**
 * What every document in the viewer does, the loader and each of the page's own: ask the app for a
 * file, and show a document of the page in one of its frames.
 */

let next = 1
const waiting = new Map<number, { ok: (g: Got) => void; fail: (e: Error) => void }>()

/** Asks the app (the top window) for a file of the page, by its address under VBASE. */
export function ask(url: string, method: 'GET' | 'HEAD' = 'GET'): Promise<Got> {
  return new Promise((ok, fail) => {
    const id = next++
    waiting.set(id, { ok, fail })
    window.top!.postMessage({ tag: TAG, t: 'get', id, url, method }, '*')
  })
}

/** A file when it was there, null for anything else. */
export const load: Load = (url) => ask(url).then((g) => (g.status === 200 ? g : null))

/**
 * Takes the app's answers. The listener goes on first, before any of the page's, and keeps the
 * answers to itself.
 */
export function listenForAnswers() {
  addEventListener('message', (e) => {
    if (e.source !== window.top) return
    const m = asMsg(e.data)
    if (!m || (m.t !== 'got' && m.t !== 'failed')) return
    e.stopImmediatePropagation()
    const w = waiting.get(m.id)
    if (!w) return
    waiting.delete(m.id)
    if (m.t === 'got') w.ok({ status: m.status, type: m.type, url: m.url, body: m.body })
    else w.fail(new Error(m.error))
  })
}

const escapeHtml = (s: string) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]!)

/** What a frame shows when its document couldn't be had at all. */
export const failure = (e: unknown) =>
  `<!DOCTYPE html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><style>body{margin:0;display:grid;place-items:center;min-height:100vh;font:14px/1.5 system-ui,sans-serif;color:CanvasText;background:Canvas;color-scheme:light dark}p{max-width:32ch;padding:16px;text-align:center;opacity:.75}</style></head><body><p>${escapeHtml(e instanceof Error ? e.message : String(e))}</p></body></html>`

const srcdoc = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'srcdoc')!.set!
const turns = new WeakMap<Element, number>()

/** Shows the document at `url` in a frame, with the shim (`shim`, its source) running first. A later call for the same frame wins. */
export async function show(frame: HTMLIFrameElement, url: string, shim: string) {
  const turn = (turns.get(frame) ?? 0) + 1
  turns.set(frame, turn)
  let html: string
  try {
    html = await asDocument(await ask(url), load, shim)
  } catch (e) {
    html = failure(e)
  }
  if (turns.get(frame) === turn) srcdoc.call(frame, html)
}

/** A document in one of this document's frames asks to be replaced by another of the page's (it followed a link). */
export function listenForGo(shim: string) {
  addEventListener('message', (e) => {
    const m = asMsg(e.data)
    if (m?.t !== 'go' || typeof m.url !== 'string' || !isVirtual(m.url)) return
    for (const f of document.querySelectorAll('iframe')) {
      if (f.contentWindow && f.contentWindow === e.source) {
        e.stopImmediatePropagation()
        void show(f, m.url, shim)
        return
      }
    }
  })
}
