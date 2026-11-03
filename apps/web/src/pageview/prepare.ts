import { inlineCss } from './css.ts'
import { rewriteLocation } from './jsloc.ts'
import { dataUrl, isVirtual, mime, textOf, textUrl, type Got, type Load } from './protocol.ts'

/**
 * A document of the page made ready for an iframe's srcdoc: every file it names when it loads (its
 * scripts, stylesheets, images, fonts, media) put in as data: addresses, its scripts' `location`
 * pointed at the shim, a `<base>` at its own address under VBASE, and the shim first of all, to
 * handle whatever its scripts ask for afterwards. Its frames are filled in by the shim once it
 * runs, each the same way.
 */

/** The attributes that make a browser fetch a file, and the elements they're on. Scripts, stylesheets and frames are handled on their own. */
export const FETCHES: Record<string, readonly string[]> = {
  img: ['src', 'srcset'],
  source: ['src', 'srcset'],
  video: ['src', 'poster'],
  audio: ['src'],
  track: ['src'],
  embed: ['src'],
  object: ['data'],
  input: ['src'],
  image: ['href', 'xlink:href'],
  feimage: ['href', 'xlink:href'],
}

/** A link element's rel values that fetch a file the viewer has to put in. */
const ICON_RELS = /(^|\s)(icon|apple-touch-icon|mask-icon)(\s|$)/i
/** ...and those that only fetch ahead, which go. */
const AHEAD_RELS = /(^|\s)(preload|modulepreload|prefetch|preconnect|dns-prefetch|manifest)(\s|$)/i

/** Script types a browser runs. Anything else (JSON, a template) is data and stays as it is. */
export const runs = (type: string | null) => !type || /^(module|(text|application)\/(x-)?(java|ecma)script|text\/jscript|text\/livescript)$/i.test(type.trim())

/** An address against a base, under VBASE; undefined for any other (an outside link, a data: address, nonsense). */
export function virtual(raw: string | null, base: string): string | undefined {
  if (raw === null) return
  const v = raw.trim()
  if (!v || /^(data|blob|javascript|about|mailto|tel):/i.test(v)) return
  try {
    const u = new URL(v, base).href
    return isVirtual(u) ? u : undefined
  } catch {
    return
  }
}

/** A srcset's candidates: each address, and what follows it (`2x`, `640w`), read as the HTML standard does. */
export function srcsetParts(v: string): { url: string; rest: string }[] {
  const out: { url: string; rest: string }[] = []
  let i = 0
  while (i < v.length) {
    while (i < v.length && /[\s,]/.test(v[i]!)) i++
    if (i >= v.length) break
    let j = i
    while (j < v.length && !/\s/.test(v[j]!)) j++
    let url = v.slice(i, j)
    i = j
    let rest = ''
    if (/,$/.test(url)) url = url.replace(/,+$/, '')
    else {
      let paren = false
      for (; i < v.length; i++) {
        const c = v[i]!
        if (c === '(') paren = true
        else if (c === ')') paren = false
        else if (c === ',' && !paren) break
        rest += c
      }
      i++
    }
    out.push({ url, rest: rest.trim() })
  }
  return out
}

/** The shim's tag for a document, with what it needs to know. */
export interface ShimConfig {
  /** the document's own address under VBASE, with its query and fragment */
  url: string
}

const escapeHtml = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!)

/** The page's files as data: addresses, each asked for once. */
function loader(load: Load) {
  const asked = new Map<string, Promise<Got | null>>()
  return (url: string) => asked.get(url) ?? asked.set(url, load(url).catch(() => null)).get(url)!
}

/**
 * The document `html` at `url` (under VBASE), ready to go in a srcdoc with the shim's source
 * `shim` running first. `load` asks for a file of the page.
 */
export async function prepare(html: string, url: string, load: Load, shim: string): Promise<string> {
  const doc = new DOMParser().parseFromString(html, 'text/html')
  const get = loader(load)
  const self = url.split('#')[0]!
  const jobs: Promise<unknown>[] = []
  const file = (el: Element, attr: string, as?: (got: Got) => Promise<string> | string) => {
    const u = virtual(el.getAttribute(attr), self)
    if (!u) return
    jobs.push(
      get(u).then(async (got) => {
        if (got) el.setAttribute(attr, as ? await as(got) : dataUrl(got.type, got.body))
      }),
    )
  }

  // the hub's CSP lets no page set its own base or policy, and a refresh would leave for nowhere
  for (const el of doc.querySelectorAll('base, meta[http-equiv]')) {
    const equiv = el.getAttribute('http-equiv')?.toLowerCase()
    if (el.localName === 'base' || equiv === 'refresh' || equiv === 'content-security-policy') el.remove()
  }

  for (const s of doc.querySelectorAll('script')) {
    if (!runs(s.getAttribute('type'))) continue
    s.removeAttribute('integrity')
    if (s.hasAttribute('src')) file(s, 'src', (got) => textUrl('text/javascript', rewriteLocation(textOf(got.body))))
    else if (s.textContent) s.textContent = rewriteLocation(s.textContent)
  }

  for (const l of doc.querySelectorAll('link[href]')) {
    const rel = l.getAttribute('rel') ?? ''
    if (AHEAD_RELS.test(rel)) l.remove()
    else if (/(^|\s)stylesheet(\s|$)/i.test(rel)) {
      l.removeAttribute('integrity')
      file(l, 'href', async (got) => textUrl('text/css', await inlineCss(textOf(got.body), got.url, get)))
    } else if (ICON_RELS.test(rel)) file(l, 'href')
  }

  for (const st of doc.querySelectorAll('style')) {
    const css = st.textContent ?? ''
    jobs.push(inlineCss(css, self, get).then((t) => t !== css && (st.textContent = t)))
  }

  for (const el of doc.querySelectorAll('*')) {
    const tag = el.localName.toLowerCase()
    for (const attr of FETCHES[tag] ?? []) {
      if (attr === 'srcset') {
        const v = el.getAttribute('srcset')
        if (!v) continue
        jobs.push(
          Promise.all(srcsetParts(v).map(async (c) => {
            const u = virtual(c.url, self)
            const got = u ? await get(u) : null
            return `${got ? dataUrl(got.type, got.body) : c.url}${c.rest ? ` ${c.rest}` : ''}`
          })).then((list) => el.setAttribute('srcset', list.join(', '))),
        )
      } else if (tag !== 'input' || el.getAttribute('type')?.toLowerCase() === 'image') file(el, attr)
    }
    const style = el.getAttribute('style')
    if (style && /url\(/i.test(style)) jobs.push(inlineCss(style, self, get).then((t) => el.setAttribute('style', t)))
    for (const a of [...el.attributes]) {
      if (a.name.startsWith('on')) a.value = rewriteLocation(a.value)
      else if (a.name === 'href' && /^\s*javascript:/i.test(a.value)) a.value = rewriteLocation(a.value)
    }
    // a frame is filled in by the shim once this document runs (the shim asks for it then)
    if (tag === 'iframe' || tag === 'frame') {
      const u = virtual(el.getAttribute('src'), self)
      if (u) {
        el.setAttribute('data-inyon-src', u)
        el.removeAttribute('src')
      }
    }
  }

  await Promise.all(jobs)

  const head = doc.head ?? doc.documentElement.insertBefore(doc.createElement('head'), doc.body)
  const base = doc.createElement('base')
  base.setAttribute('href', self)
  const script = doc.createElement('script')
  script.setAttribute('data-inyon', JSON.stringify({ url } satisfies ShimConfig))
  script.textContent = shim
  head.prepend(base, script)
  const doctype = doc.doctype ? `<!DOCTYPE ${doc.doctype.name}>` : ''
  return doctype + doc.documentElement.outerHTML
}

/**
 * What a frame shows for a file: a page (or the core's own error page, "Link expired", "Not
 * found", as the hub shows it) as itself, an image as itself, text as text, anything else as a
 * line saying so.
 */
export async function asDocument(got: Got, load: Load, shim: string): Promise<string> {
  const type = mime(got.type)
  if (type === 'text/html' || type === 'application/xhtml+xml') return prepare(textOf(got.body), got.url, load, shim)
  const wrap = (body: string) => `<!DOCTYPE html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><style>html,body{margin:0;min-height:100%}body{font:14px/1.5 ui-monospace,monospace;color:CanvasText;background:Canvas}pre{margin:0;padding:16px;white-space:pre-wrap;word-break:break-word}img{display:block;max-width:100%;margin:auto}</style></head><body>${body}</body></html>`
  if (got.status === 200 && type.startsWith('image/')) return wrap(`<img alt="" src="${dataUrl(got.type, got.body)}">`)
  if (got.status === 200 && (type.startsWith('text/') || /json|javascript|xml/.test(type))) return wrap(`<pre>${escapeHtml(textOf(got.body))}</pre>`)
  if (got.status === 200) return wrap(`<pre>This file can't be shown here.</pre>`)
  const said = textOf(got.body).trim()
  return wrap(`<pre>${escapeHtml(said && !/<[a-z]/i.test(said) ? said : got.status === 404 ? 'Not found' : `Error ${got.status}`)}</pre>`)
}
