/**
 * The page viewer for a page on a paired computer, in the phone app with no hub. There's no server
 * to give the page an address of its own, so the page is rendered from its files, which the app
 * fetches over the link (the phone core answers `/v/<token>/<path>` as a hub would):
 *
 *   app ── iframe sandbox, no allow-same-origin ──▶ pageview.html (the loader, an opaque origin)
 *                                                    └─ iframe srcdoc ─▶ the page (opaque too)
 *                                                                        └─ its own frames, the same way
 *
 * Every document of the page lives at a made-up address under VBASE: `<base href>` points there,
 * so relative links and `a.href` read as they would on the hub, and the shim (shim.ts) turns
 * whatever the page asks of that address (a script, a stylesheet, fetch, a link followed,
 * `location.href = …`) into a file asked for from the app. The address itself never loads: the
 * loader's CSP allows no network at all.
 *
 * Why not a service worker: Chromium (and so the Android WebView) doesn't let one serve a frame
 * with an opaque origin. A sandboxed iframe skips it entirely, and a page sandboxed by a CSP
 * header from the worker has its own requests go to the network, never to the worker. Why a
 * loader page and not srcdoc straight from the app: a srcdoc (or blob:, data:) document takes the
 * app's own CSP, whose `script-src 'self'` stops every script on the page. pageview.html is a
 * real file with a CSP of its own, which its srcdoc documents take instead.
 */

/** Where every document of a page seems to live. `.invalid` never resolves, and the CSP allows none of it anyway. */
export const VBASE = 'https://page.invalid/'

/** Marks every message of this protocol, so a page's own messages are never taken for one. */
export const TAG = 'inyon-pageview-1'

/** A file the app fetched for a document: what the phone core answered, after its redirects. */
export interface Got {
  status: number
  type: string
  /** the address it was found at, after a folder's redirect to its slash */
  url: string
  body: ArrayBuffer
}

export type Msg =
  /** loader → app: ready for a page */
  | { tag: typeof TAG; t: 'ready' }
  /** app → loader: show this address (VBASE…) in this theme */
  | { tag: typeof TAG; t: 'open'; url: string; theme?: string }
  /** any document of the page → app: a file */
  | { tag: typeof TAG; t: 'get'; id: number; url: string; method?: 'GET' | 'HEAD' }
  /** app → that document: the file, or why not */
  | ({ tag: typeof TAG; t: 'got'; id: number } & Got)
  | { tag: typeof TAG; t: 'failed'; id: number; error: string }
  /** a document → the one it's framed in: show this address in my place */
  | { tag: typeof TAG; t: 'go'; url: string }

/** A message of this protocol, or null. */
export function asMsg(data: unknown): Msg | null {
  const d = data as { tag?: unknown; t?: unknown } | null
  return d && typeof d === 'object' && d.tag === TAG && typeof d.t === 'string' ? (d as Msg) : null
}

/** Whether an address is one of the page's own, under VBASE. */
export const isVirtual = (url: string) => url.startsWith(VBASE)

/** The part after VBASE: the path in the site with its query, never the fragment. */
export function sitePath(url: string): string {
  const rest = url.slice(VBASE.length)
  const hash = rest.indexOf('#')
  return hash < 0 ? rest : rest.slice(0, hash)
}

/** Asks for a file of the page: what came back when it was there (200), null for anything else. */
export type Load = (url: string) => Promise<Got | null>

/** Bytes as base64, a piece at a time (one call to fromCharCode with 4 MiB of arguments throws). */
export function base64(bytes: ArrayBuffer | Uint8Array): string {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)
  let s = ''
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000))
  return btoa(s)
}

/** A file as a data: address. A content type's spaces would end the address early, so they go. */
export const dataUrl = (type: string, body: ArrayBuffer | Uint8Array) => `data:${(type || 'application/octet-stream').replace(/\s+/g, '')};base64,${base64(body)}`

/** Text as a data: address, as UTF-8. */
export const textUrl = (type: string, text: string) => dataUrl(`${type};charset=utf-8`, new TextEncoder().encode(text))

/** A file's bytes as text (UTF-8, a byte order mark dropped). */
export const textOf = (body: ArrayBuffer) => new TextDecoder().decode(body)

/** The content type without its parameters, in lower case. */
export const mime = (type: string) => type.split(';')[0]!.trim().toLowerCase()
