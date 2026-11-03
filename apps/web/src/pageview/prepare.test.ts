import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { inlineCss } from './css.ts'
import { LOCATION } from './jsloc.ts'
import { asDocument, prepare, srcsetParts, virtual } from './prepare.ts'
import { VBASE, base64, sitePath, textOf, type Got, type Load } from './protocol.ts'

// happy-dom would fetch a parsed document's stylesheets, scripts and frames for real
const settings = (window as any).happyDOM.settings
const OFF = { disableCSSFileLoading: true, disableJavaScriptFileLoading: true, disableIframePageLoading: true }
const before = Object.fromEntries(Object.keys(OFF).map((k) => [k, settings[k]]))
beforeAll(() => Object.assign(settings, OFF))
afterAll(() => Object.assign(settings, before))

const enc = new TextEncoder()
const file = (type: string, text: string, url = ''): Got => ({ status: 200, type, url, body: enc.encode(text).buffer as ArrayBuffer })

/** A site as the core would answer it: a missing file is null, and every request is counted. */
function site(files: Record<string, [string, string]>) {
  const asked: string[] = []
  const load: Load = async (url) => {
    asked.push(url)
    const f = files[sitePath(url)]
    return f ? file(f[0], f[1], url) : null
  }
  return { load, asked }
}

const fromData = (u: string | null) => {
  const m = /^data:([^;,]*)(?:;[^,]*)?;base64,(.*)$/.exec(u ?? '')
  return m ? { type: m[1], text: new TextDecoder().decode(Uint8Array.from(atob(m[2]!), (c) => c.charCodeAt(0))) } : null
}

describe('addresses', () => {
  test("only the page's own: under VBASE, against the document's address", () => {
    const at = `${VBASE}mockups/index.html?theme=dark`
    expect(virtual('screens/a.html', at)).toBe(`${VBASE}mockups/screens/a.html`)
    expect(virtual('../kit/kit.js', at)).toBe(`${VBASE}kit/kit.js`)
    expect(virtual('../../../../etc/passwd', at)).toBe(`${VBASE}etc/passwd`)
    expect(virtual('/abs.css', at)).toBe(`${VBASE}abs.css`)
    for (const outside of ['https://example.com/x.js', '//cdn.example.com/x.js', 'data:image/png;base64,AA', 'blob:null/1', 'javascript:alert(1)', 'mailto:a@b.c', '', null]) expect(virtual(outside, at)).toBeUndefined()
  })

  test('the path in the site keeps its query and drops the fragment', () => {
    expect(sitePath(`${VBASE}a/b.html?theme=dark#x`)).toBe('a/b.html?theme=dark')
  })

  test('srcset candidates, data: addresses with commas in them included', () => {
    expect(srcsetParts('a.png 1x, b.png 2x')).toEqual([{ url: 'a.png', rest: '1x' }, { url: 'b.png', rest: '2x' }])
    expect(srcsetParts('data:image/png;base64,AA,BB 1x,c.png')).toEqual([{ url: 'data:image/png;base64,AA,BB', rest: '1x' }, { url: 'c.png', rest: '' }])
  })

  test('base64 of more than one piece', () => {
    const big = new Uint8Array(70_000).map((_, i) => i % 251)
    expect(atob(base64(big)).length).toBe(70_000)
    expect(atob(base64(big)).charCodeAt(69_999)).toBe(69_999 % 251)
  })
})

describe('a stylesheet', () => {
  test('its imports and url()s are put in, against its own address; outside ones, fragments and missing files stay', async () => {
    const { load } = site({
      'kit/theme.css': ['text/css', 'body { background: url(img/bg.png) }'],
      'kit/img/bg.png': ['image/png', 'PNG'],
      'kit/fonts/g.woff2': ['font/woff2', 'WOFF'],
    })
    const css = `@import "theme.css";
@font-face { src: url('fonts/g.woff2') format('woff2') }
a { background: url(https://example.com/x.png), url(#grad), url(gone.png) }`
    const out = await inlineCss(css, `${VBASE}kit/site.css`, load)
    expect(out).toContain('body { background: url("data:image/png;base64,')
    expect(out).toContain('url("data:font/woff2;base64,')
    expect(out).toContain('url(https://example.com/x.png)')
    expect(out).toContain('url(#grad)')
    expect(out).toContain('url(gone.png)')
    expect(out).not.toContain('@import')
  })

  test('an import with a media list keeps it, and a loop of imports ends', async () => {
    const { load } = site({ 'a.css': ['text/css', '@import "b.css" print; .a{}'], 'b.css': ['text/css', '@import "a.css"; .b{}'] })
    const out = await inlineCss('@import url(a.css);', `${VBASE}index.html`, load)
    expect(out).toContain('@media print {')
    expect(out).toContain('.a{}')
    expect(out).toContain('.b{}')
  })
})

describe('a document', () => {
  const SITE = {
    'kit/site.css': ['text/css; charset=utf-8', 'h1 { background: url(dot.png) }'],
    'kit/dot.png': ['image/png', 'DOT'],
    'kit/kit.js': ['text/javascript', "window.K = { here: () => location.pathname }"],
    'pic.png': ['image/png', 'PIC'],
    'pic@2x.png': ['image/png', 'PIC2'],
    'icon.svg': ['image/svg+xml', '<svg/>'],
  } satisfies Record<string, [string, string]>

  test("its files go in as data: addresses, and its scripts' location points at the shim", async () => {
    const { load } = site(SITE)
    const html = `<!doctype html><html><head>
      <base href="https://elsewhere.example/">
      <meta http-equiv="Content-Security-Policy" content="script-src 'self'">
      <meta http-equiv="refresh" content="0; url=other.html">
      <link rel="stylesheet" href="kit/site.css" integrity="sha384-x">
      <link rel="icon" href="icon.svg">
      <link rel="preload" href="pic.png" as="image">
      <script src="kit/kit.js"></script>
      <script>document.title = location.search</script>
      <script type="application/json" id="data">{"location": 1}</script>
      <style>.x { background: url(pic.png) }</style>
    </head><body>
      <img id="a" src="pic.png" srcset="pic.png 1x, pic@2x.png 2x" onerror="location.reload()">
      <img id="b" src="https://example.com/x.png">
      <img id="c" src="gone.png">
      <div id="d" style="background: url(pic.png)"></div>
      <a id="e" href="other.html">other</a>
      <iframe id="f" src="screens/a.html?theme=dark"></iframe>
      <iframe id="g" src="https://example.com/"></iframe>
    </body></html>`
    const out = await prepare(html, `${VBASE}index.html?theme=dark`, load, 'SHIM()')
    const doc = new DOMParser().parseFromString(out, 'text/html')
    const $ = (q: string) => doc.querySelector(q)!

    expect(out.startsWith('<!DOCTYPE html>')).toBe(true)
    // the shim first of all, with the document's address; then our base, and no other
    expect(doc.head.firstElementChild!.localName).toBe('base')
    expect(doc.querySelectorAll('base').length).toBe(1)
    expect($('base').getAttribute('href')).toBe(`${VBASE}index.html?theme=dark`)
    const shim = doc.head.children[1]!
    expect(shim.localName).toBe('script')
    expect(shim.textContent).toBe('SHIM()')
    expect(JSON.parse(shim.getAttribute('data-inyon')!)).toEqual({ url: `${VBASE}index.html?theme=dark` })
    expect(doc.querySelector('meta[http-equiv]')).toBeNull()

    const css = fromData($('link[rel=stylesheet]').getAttribute('href'))!
    expect(css.type).toBe('text/css')
    expect(css.text).toContain('url("data:image/png;base64,')
    expect($('link[rel=stylesheet]').hasAttribute('integrity')).toBe(false)
    expect(fromData($('link[rel=icon]').getAttribute('href'))).toEqual({ type: 'image/svg+xml', text: '<svg/>' })
    expect(doc.querySelector('link[rel=preload]')).toBeNull()

    const scripts = [...doc.querySelectorAll('script')]
    expect(fromData(scripts[1]!.getAttribute('src'))!.text).toBe(`window.K = { here: () => ${LOCATION}.pathname }`)
    expect(scripts[2]!.textContent).toBe(`document.title = ${LOCATION}.search`)
    expect(scripts[3]!.textContent).toBe('{"location": 1}')
    expect($('style').textContent).toContain('url("data:image/png;base64,')

    expect(fromData($('#a').getAttribute('src'))).toEqual({ type: 'image/png', text: 'PIC' })
    expect($('#a').getAttribute('srcset')).toMatch(/^data:image\/png;base64,\S+ 1x, data:image\/png;base64,\S+ 2x$/)
    expect($('#a').getAttribute('onerror')).toBe(`${LOCATION}.reload()`)
    expect($('#b').getAttribute('src')).toBe('https://example.com/x.png')
    // a missing file keeps its address: the frame's CSP refuses it, so it fails as it would
    expect($('#c').getAttribute('src')).toBe('gone.png')
    expect($('#d').getAttribute('style')).toContain('url("data:image/png;base64,')
    expect($('#e').getAttribute('href')).toBe('other.html')
    // the shim fills a frame of the page's own once it runs
    expect($('#f').hasAttribute('src')).toBe(false)
    expect($('#f').getAttribute('data-inyon-src')).toBe(`${VBASE}screens/a.html?theme=dark`)
    expect($('#g').getAttribute('src')).toBe('https://example.com/')
  })

  test('each file is asked for once, however often it is named', async () => {
    const { load, asked } = site(SITE)
    await prepare('<img src="pic.png"><img src="pic.png"><div style="background:url(pic.png)"></div>', `${VBASE}`, load, '')
    expect(asked).toEqual([`${VBASE}pic.png`])
  })

  test('a folder page resolves against its own address', async () => {
    const { load, asked } = site(SITE)
    await prepare('<link rel="stylesheet" href="../kit/site.css"><img src="../pic.png">', `${VBASE}docs/`, load, '')
    expect(asked.sort()).toEqual([`${VBASE}kit/dot.png`, `${VBASE}kit/site.css`, `${VBASE}pic.png`])
  })

  test("what isn't a page: an image as itself, text as text, an error as what it said", async () => {
    const { load } = site({})
    const img = await asDocument({ status: 200, type: 'image/png', url: `${VBASE}a.png`, body: enc.encode('PNG').buffer as ArrayBuffer }, load, '')
    expect(img).toContain('<img alt="" src="data:image/png;base64,')
    const text = await asDocument(file('text/plain', '<b>hi</b>'), load, '')
    expect(text).toContain('<pre>&lt;b&gt;hi&lt;/b&gt;</pre>')
    const missing = await asDocument({ status: 404, type: 'text/plain', url: '', body: enc.encode('Not found').buffer as ArrayBuffer }, load, '')
    expect(missing).toContain('<pre>Not found</pre>')
    const html = await asDocument(file('text/html; charset=utf-8', '<p>hi</p>', `${VBASE}x.html`), load, 'S')
    expect(html).toContain(`<base href="${VBASE}x.html">`)
    expect(textOf(enc.encode('﻿hé').buffer as ArrayBuffer)).toBe('hé')
  })
})
