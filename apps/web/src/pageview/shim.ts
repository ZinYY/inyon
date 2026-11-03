import { inlineCss } from './css.ts'
import { ask, listenForAnswers, listenForGo, load, show } from './frames.ts'
import { LOCATION, rewriteLocation } from './jsloc.ts'
import { FETCHES, srcsetParts, virtual, type ShimConfig } from './prepare.ts'
import { TAG, isVirtual, mime, textOf, type Got } from './protocol.ts'

/**
 * The shim: the first script in every document of the page. prepare.ts put in what the document
 * names as it loads; this handles what its scripts do afterwards, so a page behaves as it does on
 * the hub:
 *
 * - `location` (rewritten to `__inyonLocation`, see jsloc.ts) reads the page's own address, and
 *   setting it, a link clicked, `window.open` or `history.pushState` go to another of the page's
 *   documents;
 * - `fetch` and XMLHttpRequest for the page's files are answered by the app;
 * - an image, script, stylesheet or frame given one of the page's files (by property,
 *   setAttribute, innerHTML or anything else) gets it, as a blob: address made here.
 *
 * The frame's CSP lets nothing load from anywhere but data: and blob:, so whatever this misses
 * fails to load; it never reaches anything else.
 */
export function startShim(config: ShimConfig, source: string) {
  listenForAnswers()
  listenForGo(source)

  // ---------- the address ----------

  const real = window.location
  const first = new URL(config.url)
  let here = new URL(first.href)
  here.hash = ''
  const baseEl = document.querySelector('base')
  const base = () => here.href
  const history = window.history
  const replaceState = history.replaceState
  const pushState = history.pushState
  if (first.hash) replaceState.call(history, history.state, '', `about:srcdoc${first.hash}`)

  /** Goes to an address the way following a link to it would. */
  const navigate = (to: string) => {
    let u: URL
    try {
      u = new URL(to, here.href)
    } catch {
      return
    }
    if (!isVirtual(u.href)) {
      real.href = u.href
      return
    }
    const fragment = u.href.includes('#')
    const hash = u.hash
    u.hash = ''
    if (fragment && u.href === here.href) {
      real.hash = hash
      return
    }
    window.parent.postMessage({ tag: TAG, t: 'go', url: u.href + hash }, '*')
  }

  const at = (change: (u: URL) => void) => {
    const u = new URL(here.href)
    change(u)
    navigate(u.href)
  }
  const stand = {
    get href() {
      return here.href + real.hash
    },
    set href(v: string) {
      navigate(String(v))
    },
    get origin() {
      return here.origin
    },
    get protocol() {
      return here.protocol
    },
    get host() {
      return here.host
    },
    get hostname() {
      return here.hostname
    },
    get port() {
      return here.port
    },
    get pathname() {
      return here.pathname
    },
    set pathname(v: string) {
      at((u) => (u.pathname = v))
    },
    get search() {
      return here.search
    },
    set search(v: string) {
      at((u) => (u.search = v))
    },
    get hash() {
      return real.hash
    },
    set hash(v: string) {
      real.hash = v
    },
    get ancestorOrigins() {
      return real.ancestorOrigins
    },
    assign: (v: string) => navigate(String(v)),
    replace: (v: string) => navigate(String(v)),
    reload: () => navigate(here.href),
    toString: () => here.href + real.hash,
  }
  for (const owner of [window, document]) Object.defineProperty(owner, LOCATION, { get: () => stand, set: (v) => navigate(String(v)), configurable: false })

  for (const [name, fn] of [['pushState', pushState], ['replaceState', replaceState]] as const) {
    history[name] = function (state: unknown, title: string, url?: string | URL | null) {
      if (url === undefined || url === null) return fn.call(this, state, title)
      const u = new URL(String(url), here.href)
      if (u.origin !== here.origin) throw new DOMException(`A history state with the URL '${u.href}' can't be made in a document with the URL '${here.href}'.`, 'SecurityError')
      const hash = u.hash
      u.hash = ''
      if (u.href !== here.href) {
        here = u
        baseEl?.setAttribute('href', u.href)
      }
      return fn.call(this, state, title, `about:srcdoc${hash}`)
    }
  }

  // links, window.open
  const download = async (url: string, name: string) => {
    const got = await ask(url)
    if (got.status !== 200) return
    const a = document.createElement('a')
    a.href = URL.createObjectURL(new Blob([got.body], { type: got.type }))
    a.download = name || decodeURIComponent(new URL(url).pathname.split('/').pop() ?? '') || 'download'
    a.click()
    setTimeout(() => URL.revokeObjectURL(a.href), 60_000)
  }
  addEventListener('click', (e: MouseEvent) => {
    if (e.defaultPrevented || e.button !== 0) return
    const a = (e.target as Element | null)?.closest?.('a[href], area[href]')
    if (!a) return
    const u = virtual(a.getAttribute('href'), base())
    if (!u) return
    e.preventDefault()
    if (a.hasAttribute('download')) void download(u, a.getAttribute('download') ?? '')
    else navigate(u)
  })
  const open = window.open
  window.open = function (url?: string | URL, target?: string, features?: string) {
    const u = url === undefined || url === '' ? undefined : virtual(String(url), base())
    if (!u) return open.call(window, url, target, features)
    navigate(u)
    return null
  }

  // ---------- fetch and XMLHttpRequest ----------

  const realFetch = window.fetch
  const noBody = (status: number) => status === 204 || status === 205 || status === 304
  window.fetch = function (input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const req = input instanceof Request ? input : undefined
    const u = virtual(req ? req.url : input instanceof URL ? input.href : String(input), base())
    if (!u) return realFetch.call(window, input, init)
    const method = (init?.method ?? req?.method ?? 'GET').toUpperCase()
    if (method !== 'GET' && method !== 'HEAD') return Promise.resolve(new Response(null, { status: 405, headers: { allow: 'GET, HEAD' } }))
    return ask(u, method).then((g) => new Response(method === 'HEAD' || noBody(g.status) ? null : g.body, { status: g.status, headers: { 'content-type': g.type } }), () => Promise.reject(new TypeError('Failed to fetch')))
  } as typeof fetch

  hookXhr(base)

  // ---------- files given to elements ----------

  /** What the page set, by element and attribute, for the getters to give back. */
  const given = new WeakMap<Element, Record<string, string>>()
  const proto = Element.prototype
  const setAttr = proto.setAttribute
  const removeAttr = proto.removeAttribute

  const wanted = (el: Element, attr: string) => {
    const tag = el.localName.toLowerCase()
    if (tag === 'script' || tag === 'iframe' || tag === 'frame') return attr === 'src'
    if (tag === 'link') return attr === 'href'
    if (tag === 'input' && (el.getAttribute('type') ?? '').toLowerCase() !== 'image') return false
    return !!FETCHES[tag]?.includes(attr)
  }

  const blob = (body: BlobPart, type: string) => URL.createObjectURL(new Blob([body], { type }))

  /** The address an element gets for one of the page's files. */
  const fileFor = async (el: Element, got: Got): Promise<string> => {
    const tag = el.localName.toLowerCase()
    if (tag === 'script') return blob(rewriteLocation(textOf(got.body)), 'text/javascript')
    if (tag === 'link' && mime(got.type) === 'text/css') return blob(await inlineCss(textOf(got.body), got.url, load), 'text/css')
    return blob(got.body, got.type)
  }

  const fill = async (el: Element, attr: string, value: string) => {
    if (el.localName.toLowerCase() === 'iframe') return show(el as HTMLIFrameElement, value, source)
    let out: string | undefined
    if (attr === 'srcset') {
      const parts = await Promise.all(
        srcsetParts(value).map(async (c) => {
          const u = virtual(c.url, base())
          const got = u ? await load(u) : null
          return `${got ? blob(got.body, got.type) : c.url}${c.rest ? ` ${c.rest}` : ''}`
        }),
      )
      out = parts.join(', ')
    } else {
      const got = await load(value).catch(() => null)
      if (got) out = await fileFor(el, got)
    }
    if (given.get(el)?.[attr] !== value) return
    if (out !== undefined) setAttr.call(el, attr, out)
    else el.dispatchEvent(new Event('error'))
  }

  /** Takes over an attribute set to one of the page's files; false for anything else, which goes through as it is. */
  const claim = (el: Element, attr: string, raw: string): boolean => {
    if (!wanted(el, attr)) return false
    const value = attr === 'srcset' ? (srcsetParts(raw).some((c) => virtual(c.url, base())) ? raw : undefined) : virtual(raw, base())
    const mine = given.get(el)
    if (value === undefined) {
      if (mine) delete mine[attr]
      return false
    }
    ;(mine ?? given.set(el, {}).get(el)!)[attr] = value
    removeAttr.call(el, attr)
    void fill(el, attr, value)
    return true
  }

  const hook = (target: { prototype: object } | undefined, prop: string, attr = prop) => {
    const p = target?.prototype
    const d = p && Object.getOwnPropertyDescriptor(p, prop)
    if (!d?.set || !d.get) return
    const { get, set } = d
    Object.defineProperty(p, prop, {
      ...d,
      get() {
        return given.get(this)?.[attr] ?? get.call(this)
      },
      set(v: unknown) {
        if (!claim(this, attr, String(v))) set.call(this, v)
      },
    })
  }
  hook(window.HTMLImageElement, 'src')
  hook(window.HTMLImageElement, 'srcset')
  hook(window.HTMLSourceElement, 'src')
  hook(window.HTMLSourceElement, 'srcset')
  hook(window.HTMLMediaElement, 'src')
  hook(window.HTMLVideoElement, 'poster')
  hook(window.HTMLTrackElement, 'src')
  hook(window.HTMLEmbedElement, 'src')
  hook(window.HTMLObjectElement, 'data')
  hook(window.HTMLInputElement, 'src')
  hook(window.HTMLScriptElement, 'src')
  hook(window.HTMLLinkElement, 'href')
  hook(window.HTMLIFrameElement, 'src')

  proto.setAttribute = function (name: string, value: string) {
    if (!claim(this, name.toLowerCase(), String(value))) setAttr.call(this, name, value)
  }
  const setAttrNS = proto.setAttributeNS
  proto.setAttributeNS = function (ns: string | null, name: string, value: string) {
    if (!claim(this, name.toLowerCase(), String(value))) setAttrNS.call(this, ns, name, value)
  }
  proto.removeAttribute = function (name: string) {
    const mine = given.get(this)
    if (mine) delete mine[name.toLowerCase()]
    removeAttr.call(this, name)
  }

  // a script's own text is rewritten as the page's scripts are
  for (const [owner, prop] of [[window.HTMLScriptElement, 'text'], [Node, 'textContent'], [HTMLElement, 'innerText']] as const) {
    const d = Object.getOwnPropertyDescriptor(owner.prototype, prop)
    if (!d?.set) continue
    const set = d.set
    Object.defineProperty(owner.prototype, prop, { ...d, set(v: unknown) { set.call(this, this instanceof HTMLScriptElement ? rewriteLocation(String(v ?? '')) : v) } })
  }

  // markup: its files are marked before the browser sees it (it would start loading them at once),
  // and filled in once it is in the document
  const inner = Object.getOwnPropertyDescriptor(proto, 'innerHTML')!
  const MARK = 'data-inyon-'
  const markAttr = (attr: string) => MARK + attr.replace(':', '-')
  const markTree = (root: ParentNode) => {
    for (const el of root.querySelectorAll('*')) {
      const tag = el.localName.toLowerCase()
      const attrs = tag === 'script' || tag === 'iframe' || tag === 'frame' ? ['src'] : tag === 'link' ? ['href'] : (FETCHES[tag] ?? [])
      for (const attr of attrs) {
        const v = el.getAttribute(attr)
        if (v === null || !wanted(el, attr)) continue
        if (attr === 'srcset' ? srcsetParts(v).some((c) => virtual(c.url, base())) : virtual(v, base())) {
          removeAttr.call(el, attr)
          setAttr.call(el, markAttr(attr), v)
        }
      }
      for (const a of [...el.attributes]) if (a.name.startsWith('on') || (a.name === 'href' && /^\s*javascript:/i.test(a.value))) a.value = rewriteLocation(a.value)
      if (tag === 'script' && el.textContent) el.textContent = el.textContent
    }
  }
  const markup = (html: string) => {
    if (!/\b(src|srcset|href|poster|data|on\w+)\s*=|<script/i.test(html)) return html
    const t = document.createElement('template')
    inner.set!.call(t, html)
    markTree(t.content)
    return inner.get!.call(t) as string
  }
  for (const owner of [Element, window.ShadowRoot]) {
    const d = owner && Object.getOwnPropertyDescriptor(owner.prototype, 'innerHTML')
    if (!d?.set) continue
    const set = d.set
    Object.defineProperty(owner.prototype, 'innerHTML', { ...d, set(v: unknown) { set.call(this, this instanceof HTMLScriptElement ? rewriteLocation(String(v ?? '')) : markup(String(v ?? ''))) } })
  }
  const outer = Object.getOwnPropertyDescriptor(proto, 'outerHTML')
  if (outer?.set) Object.defineProperty(proto, 'outerHTML', { ...outer, set(v: unknown) { outer.set!.call(this, markup(String(v ?? ''))) } })
  const adjacent = proto.insertAdjacentHTML
  proto.insertAdjacentHTML = function (where: InsertPosition, html: string) {
    adjacent.call(this, where, markup(String(html)))
  }
  const fragment = Range.prototype.createContextualFragment
  Range.prototype.createContextualFragment = function (html: string) {
    return fragment.call(this, markup(String(html)))
  }

  // anything in the document that still names one of the page's files: marks, and whatever got
  // past the hooks above
  const styled = (el: Element) => {
    const style = el.getAttribute('style')
    if (style && /url\(/i.test(style)) void inlineCss(style, base(), load).then((t) => t !== style && el.getAttribute('style') === style && setAttr.call(el, 'style', t))
  }
  const check = (el: Element, name: string) => {
    if (name === 'style') return styled(el)
    const v = el.getAttribute(name)
    if (v === null) return
    if (name.startsWith(MARK)) {
      removeAttr.call(el, name)
      const attr = name === `${MARK}xlink-href` ? 'xlink:href' : name.slice(MARK.length)
      if (!claim(el, attr, v)) setAttr.call(el, attr, v)
    } else if (wanted(el, name) && !/^\s*(blob|data):/i.test(v)) claim(el, name, v)
  }
  const ATTRS = ['src', 'srcset', 'href', 'xlink:href', 'poster', 'data', 'style']
  const MARKS = ATTRS.filter((a) => a !== 'style').map(markAttr)
  const scan = (root: Element) => {
    for (const el of [root, ...root.querySelectorAll('*')]) {
      for (const name of [...MARKS, ...ATTRS]) if (el.hasAttribute(name)) check(el, name)
      if (el.localName === 'style' && /url\(|@import/i.test(el.textContent ?? '')) {
        const css = el.textContent ?? ''
        void inlineCss(css, base(), load).then((t) => t !== css && (el.textContent = t))
      }
    }
  }
  new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === 'childList') {
        for (const n of r.addedNodes) if (n.nodeType === 1) scan(n as Element)
      } else if (r.attributeName && r.target.nodeType === 1) check(r.target as Element, r.attributeName)
    }
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: [...ATTRS, ...MARKS] })
}

/** XMLHttpRequest for the page's files, answered by the app; any other address goes through as it is. */
function hookXhr(base: () => string) {
  const X = XMLHttpRequest.prototype
  interface Mine {
    method: string
    url: string
    ready: number
    got?: Got
    aborted?: boolean
  }
  const mine = new WeakMap<XMLHttpRequest, Mine>()
  const fire = (x: XMLHttpRequest, type: string) => x.dispatchEvent(type.startsWith('load') || type === 'error' || type === 'abort' ? new ProgressEvent(type) : new Event(type))
  const open = X.open as (this: XMLHttpRequest, ...a: unknown[]) => void
  X.open = function (this: XMLHttpRequest, method: string, url: string | URL, ...rest: unknown[]) {
    const u = virtual(String(url), base())
    if (!u) {
      mine.delete(this)
      return open.call(this, method, url, ...rest)
    }
    mine.set(this, { method: method.toUpperCase(), url: u, ready: 1 })
    fire(this, 'readystatechange')
  } as typeof X.open
  const send = X.send
  X.send = function (body?: Document | XMLHttpRequestBodyInit | null) {
    const m = mine.get(this)
    if (!m) return send.call(this, body)
    const method = m.method === 'HEAD' ? 'HEAD' : 'GET'
    fire(this, 'loadstart')
    ask(m.url, method).then(
      (got) => {
        if (m.aborted) return
        m.got = m.method === 'GET' || m.method === 'HEAD' ? got : { ...got, status: 405, body: new ArrayBuffer(0) }
        m.ready = 4
        fire(this, 'readystatechange')
        fire(this, 'load')
        fire(this, 'loadend')
      },
      () => {
        if (m.aborted) return
        m.ready = 4
        fire(this, 'readystatechange')
        fire(this, 'error')
        fire(this, 'loadend')
      },
    )
  }
  const pass = <K extends keyof XMLHttpRequest>(name: K, mineValue: (m: Mine, x: XMLHttpRequest) => unknown) => {
    const d = Object.getOwnPropertyDescriptor(X, name)
    if (!d?.get) return
    const get = d.get
    Object.defineProperty(X, name, {
      ...d,
      get() {
        const m = mine.get(this)
        return m ? mineValue(m, this) : get.call(this)
      },
    })
  }
  const text = (m: Mine) => (m.got && m.method !== 'HEAD' ? textOf(m.got.body) : '')
  pass('readyState', (m) => m.ready)
  pass('status', (m) => m.got?.status ?? 0)
  pass('statusText', (m) => (m.got ? (m.got.status === 200 ? 'OK' : '') : ''))
  pass('responseURL', (m) => m.got?.url ?? '')
  pass('responseText', (m) => text(m))
  pass('responseXML', (m) => (m.got ? new DOMParser().parseFromString(text(m), mime(m.got.type) === 'text/html' ? 'text/html' : 'application/xml') : null))
  pass('response', (m, x) => {
    if (!m.got) return x.responseType === '' || x.responseType === 'text' ? '' : null
    switch (x.responseType) {
      case 'arraybuffer':
        return m.got.body
      case 'blob':
        return new Blob([m.got.body], { type: m.got.type })
      case 'json':
        try {
          return JSON.parse(text(m))
        } catch {
          return null
        }
      case 'document':
        return new DOMParser().parseFromString(text(m), 'text/html')
      default:
        return text(m)
    }
  })
  const header = X.getResponseHeader
  X.getResponseHeader = function (name: string) {
    const m = mine.get(this)
    return m ? (m.got && name.toLowerCase() === 'content-type' ? m.got.type : null) : header.call(this, name)
  }
  const headers = X.getAllResponseHeaders
  X.getAllResponseHeaders = function () {
    const m = mine.get(this)
    return m ? (m.got ? `content-type: ${m.got.type}\r\n` : '') : headers.call(this)
  }
  const setHeader = X.setRequestHeader
  X.setRequestHeader = function (name: string, value: string) {
    if (!mine.has(this)) setHeader.call(this, name, value)
  }
  const abort = X.abort
  X.abort = function () {
    const m = mine.get(this)
    if (!m) return abort.call(this)
    m.aborted = true
    m.ready = 0
    fire(this, 'abort')
    fire(this, 'loadend')
  }
}
