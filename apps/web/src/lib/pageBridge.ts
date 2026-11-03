import { TAG, VBASE, asMsg, sitePath, type Got } from '../pageview/protocol.ts'

/**
 * The app's side of the page viewer (src/pageview): a page on a paired computer, in the phone app
 * with no hub, asks the app for each of its files, and the app asks the phone core, which answers
 * `/v/<token>/<path>` as a hub would (crates/inyon-phone/src/pages.rs). Only a document inside a
 * frame the app put up for a page is answered, and only with that page's files: the address under
 * VBASE is turned into a path under the frame's own token, so nothing else the core answers is in
 * reach.
 */

/** Asks for a path (`/v/<token>/…`) as the core answers it. */
export type PageFetch = (path: string, method: 'GET' | 'HEAD') => Promise<Response>

export interface BridgedFrame {
  /** the loader's window (the frame's contentWindow) */
  win: Window
  /** the link's path, `/v/<token>/` */
  base: string
  /** what the loader shows first, under VBASE */
  url: string
  theme?: string
  fetch: PageFetch
}

/** How many redirects (a folder to its slash) are followed. */
const MAX_HOPS = 4

/**
 * The file at a page's address under VBASE, asked for under the link `base`. A folder's redirect
 * to its slash is followed and the address it ends at given back under VBASE.
 */
export async function fetchPageFile(base: string, url: string, method: 'GET' | 'HEAD', fetch: PageFetch): Promise<Got> {
  let u = new URL(url)
  for (let hop = 0; ; hop++) {
    if (`${u.origin}/` !== VBASE) return { status: 404, type: 'text/plain', url, body: new ArrayBuffer(0) }
    u.hash = ''
    // URL parsing has already resolved any dot segments, so the path can't climb out of the token
    const res = await fetch(base + sitePath(u.href), method)
    const to = res.headers.get('location')
    if (res.status >= 300 && res.status < 400 && to?.startsWith(base) && hop < MAX_HOPS) {
      u = new URL(VBASE + to.slice(base.length))
      continue
    }
    // a fetch that followed the redirect itself (a browser's, in the browser check)
    if (res.redirected) {
      const at = new URL(res.url)
      if (at.pathname.startsWith(base)) u = new URL(VBASE + at.pathname.slice(base.length) + at.search)
    }
    return { status: res.status, type: res.headers.get('content-type') ?? '', url: u.href, body: method === 'HEAD' ? new ArrayBuffer(0) : await res.arrayBuffer() }
  }
}

const frames = new Set<BridgedFrame>()

/** The frame a window is in (the loader itself, or any frame inside it, however deep). */
function frameOf(source: MessageEventSource | null): BridgedFrame | undefined {
  let w = source as Window | null
  for (let i = 0; w && i < 16; i++) {
    for (const f of frames) if (f.win === w) return f
    let up: Window
    try {
      up = w.parent
    } catch {
      return
    }
    if (up === w) return
    w = up
  }
}

function onMessage(e: MessageEvent) {
  const m = asMsg(e.data)
  if (!m || (m.t !== 'get' && m.t !== 'ready')) return
  const f = frameOf(e.source)
  const source = e.source as Window | null
  if (!f || !source) return
  if (m.t === 'ready') {
    if (source === f.win) source.postMessage({ tag: TAG, t: 'open', url: f.url, theme: f.theme }, '*')
    return
  }
  if (typeof m.id !== 'number' || typeof m.url !== 'string') return
  fetchPageFile(f.base, m.url, m.method === 'HEAD' ? 'HEAD' : 'GET', f.fetch).then(
    (got) => source.postMessage({ tag: TAG, t: 'got', id: m.id, ...got }, '*', [got.body]),
    (err) => source.postMessage({ tag: TAG, t: 'failed', id: m.id, error: (err as Error).message || 'The file could not be fetched' }, '*'),
  )
}

/** Answers a frame's documents until the returned function is called. */
export function bridgeFrame(f: BridgedFrame): () => void {
  if (!frames.size) window.addEventListener('message', onMessage)
  frames.add(f)
  return () => {
    frames.delete(f)
    if (!frames.size) window.removeEventListener('message', onMessage)
  }
}

/** A frame's address (`/v/<token>/part?theme=…`) as the viewer shows it, under VBASE. */
export const virtualSrc = (src: string, base: string) => VBASE + (src.startsWith(base) ? src.slice(base.length) : '')
