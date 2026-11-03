// bun e2e/pageview.ts: a page on a paired computer, in the phone app with no hub, as the Pages
// viewer shows it (src/pageview). Serves an app page with the app's own CSP, the real loader
// (pageview.html, built as the app's build makes it) and the real bridge, with this server standing
// in for the phone core at /v/<token>/ (folders get their slash, dot names and missing files 404).
// Then checks, in Chromium, that a site made of many files renders as on a hub: its stylesheet and
// imports, images from HTML, CSS and innerHTML, a script, fetch, the theme from ?theme= and from the
// app, a frame of its own whose screens go to each other with location.href, links to a page and
// to a folder, an in-page jump. And that it stays shut in: an opaque origin, nothing outside its own
// files, no request but the app's own.
//
// PAGEVIEW_SITE=/path/to/site shows another site instead (a Blueprint plan site, say), takes a
// screenshot of each part and fails on any error the page logs.
// CHROME=/path/to/chrome uses a browser other than Playwright's own.
import path from 'node:path'
import { existsSync, statSync } from 'node:fs'
import { chromium, type FrameLocator, type Locator, type Page } from 'playwright'
import { pageviewHtml } from '../src/pageview/build.ts'

const site = path.resolve(process.env.PAGEVIEW_SITE ?? path.join(import.meta.dir, 'pageview-site'))
const custom = !!process.env.PAGEVIEW_SITE
const TOKEN = 'tok'
const BASE = `/v/${TOKEN}/`
const APP_CSP = "script-src 'self'; object-src 'none'; base-uri 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self'"

const loader = await pageviewHtml()
const built = await Bun.build({ entrypoints: [path.join(import.meta.dir, 'pageview-app.ts')], format: 'esm', target: 'browser' })
if (!built.success) throw new AggregateError(built.logs, 'Could not bundle the app half')
const appJs = await built.outputs[0]!.text()

const TYPES: Record<string, string> = { html: 'text/html; charset=utf-8', css: 'text/css; charset=utf-8', js: 'text/javascript; charset=utf-8', json: 'application/json', png: 'image/png', webp: 'image/webp', svg: 'image/svg+xml', woff2: 'font/woff2', md: 'text/markdown; charset=utf-8' }

/** What the phone core answers for a path under the token, roughly as pages_core decides it. */
function core(rest: string, query: string, head: boolean): Response {
  const segs = rest.split('/')
  if (segs.some((s) => s.startsWith('.') || s.includes('%2f') || s.includes('%2F') || s.includes('\\'))) return new Response('Not found', { status: 404 })
  let file = path.join(site, ...segs.map(decodeURIComponent))
  if (!file.startsWith(site)) return new Response('Not found', { status: 404 })
  if (existsSync(file) && statSync(file).isDirectory()) {
    if (rest && !rest.endsWith('/')) return new Response(null, { status: 301, headers: { location: `${BASE}${rest}/${query}` } })
    file = path.join(file, 'index.html')
  }
  if (!existsSync(file) || !statSync(file).isFile()) return new Response('Not found', { status: 404, headers: { 'content-type': 'text/plain; charset=utf-8' } })
  const type = TYPES[path.extname(file).slice(1)] ?? 'application/octet-stream'
  return new Response(head ? null : Bun.file(file), { headers: { 'content-type': type, 'cache-control': 'no-store' } })
}

const requests: string[] = []
const server = Bun.serve({
  port: 0,
  hostname: '127.0.0.1',
  fetch(req) {
    const u = new URL(req.url)
    requests.push(u.pathname)
    if (u.pathname === '/') return new Response(`<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${APP_CSP}"><style>html,body{margin:0;height:100%}iframe{border:0;width:100%;height:100%}</style></head><body><script type="module" src="/app.js"></script></body></html>`, { headers: { 'content-type': 'text/html' } })
    if (u.pathname === '/app.js') return new Response(appJs, { headers: { 'content-type': 'text/javascript' } })
    if (u.pathname === '/pageview.html') return new Response(loader, { headers: { 'content-type': 'text/html' } })
    if (u.pathname.startsWith(BASE)) return core(u.pathname.slice(BASE.length), u.search, req.method === 'HEAD')
    return new Response('Not found', { status: 404 })
  },
})
const origin = `http://127.0.0.1:${server.port}`

const problems: string[] = []
let checks = 0
const check = (ok: boolean, what: string) => {
  checks++
  if (ok) return
  problems.push(what)
  console.error(`  failed: ${what}`)
}

/** Waits for a locator's text to match, and records it. */
async function textIs(l: Locator, want: RegExp | string, what: string) {
  let got = ''
  const until = Date.now() + 15_000
  while (Date.now() < until) {
    got = (await l.textContent({ timeout: 1_000 }).catch(() => null)) ?? ''
    if (typeof want === 'string' ? got === want : want.test(got)) return check(true, what)
    await Bun.sleep(100)
  }
  check(false, `${what}: got "${got}"`)
}

// a check that hangs fails rather than holding the machine's one heavy slot
const watchdog = setTimeout(() => {
  console.error(`pageview: timed out after ${checks} checks; failed so far: ${problems.join(' / ') || 'none'}`)
  process.exit(1)
}, 240_000)

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {})
try {
  const ctx = await browser.newContext({ viewport: { width: 412, height: 860 }, colorScheme: 'dark' })
  const page = await ctx.newPage()
  const logged: string[] = []
  page.on('console', (m) => m.type() === 'error' && logged.push(m.text()))
  page.on('pageerror', (e) => logged.push(String(e)))
  // every request must be the app's own: the page's files come through the app, never from the page's frames
  page.on('request', (r) => {
    const u = new URL(r.url())
    if (u.origin !== origin && !/^(data|blob|about):/.test(r.url())) problems.push(`a request left the app: ${r.url()}`)
    else if (u.pathname.startsWith('/v/') && r.frame() !== page.mainFrame()) problems.push(`a page's frame asked for ${u.pathname} itself`)
  })

  if (custom) await showSite(page, logged)
  else await checkFixture(page, logged)
} finally {
  await browser.close()
  server.stop(true)
}

async function checkFixture(page: Page, logged: string[]) {
  await page.goto(`${origin}/`)
  const doc = page.frameLocator('#viewer').frameLocator('#page')
  await textIs(doc.locator('#title'), 'A page from a computer', 'the page renders in the viewer')
  await textIs(doc.locator('#theme'), 'dark', 'location.search carries ?theme=')
  await textIs(doc.locator('#progress'), '3/7', 'fetch() reads the page\'s own JSON')
  await textIs(doc.locator('#said'), 'theme dark', 'the page asks its parent for the theme and is told')
  check((await doc.locator('h1').evaluate((e) => getComputedStyle(e).color)) === 'rgb(10, 20, 30)', 'the stylesheet applies')
  check((await doc.locator('body').evaluate((e) => getComputedStyle(e).backgroundColor)) === 'rgb(250, 250, 240)', "the stylesheet's @import applies")
  check(/^url\("data:image\/png/.test(await doc.locator('#bg').evaluate((e) => getComputedStyle(e).backgroundImage)), "the stylesheet's url() is put in")
  await page.waitForTimeout(300)
  check((await doc.locator('#dot').evaluate((e: HTMLImageElement) => e.complete && e.naturalWidth)) === 4, 'an image in the HTML loads')
  check((await doc.locator('#pic').evaluate((e: HTMLImageElement) => (e.dataset.failed ? -1 : e.naturalWidth))) === 4, 'an image added with innerHTML loads, with no error first')
  await textIs(doc.locator('#escape'), 'null | parent hidden | climb 404 | outside refused', 'the page is shut in: opaque origin, no parent, nothing outside its folder or online')

  // its own frame, and screens that go to each other with location.href
  const screen = doc.frameLocator('#screen')
  await textIs(screen.locator('#name'), 'Screen A', "the page's own frame shows the screen it set")
  await screen.locator('#next').click()
  await textIs(screen.locator('#name'), 'Screen B', 'location.href in a screen goes to the next')
  await textIs(screen.locator('#where'), 'at b ?theme=dark', 'the next screen knows its own address')
  await textIs(doc.locator('#title'), 'A page from a computer', 'the page stays put around its frame')

  // an in-page jump keeps the document
  await doc.locator('#jump').click()
  await page.waitForTimeout(200)
  check((await doc.locator('#title').textContent()) === 'A page from a computer', 'a jump to #end stays on the page')
  check((await doc.locator('body').evaluate(() => location.hash)) === '#end', 'the jump sets the fragment')

  // a link to another page, then to a folder (its redirect to the slash followed)
  await doc.locator('#other').click()
  await textIs(doc.locator('#title'), 'Other page', 'a link opens another page of the site')
  await textIs(doc.locator('#where'), '/other.html?from=index', 'the other page knows its address')
  await page.goto(`${origin}/`)
  await textIs(doc.locator('#title'), 'A page from a computer', 'the page renders again')
  await doc.locator('#docs').click()
  await textIs(doc.locator('#title'), 'Docs index', 'a link to a folder opens its index.html')
  await textIs(doc.locator('#where'), 'https://page.invalid/docs/', 'the folder gets its slash')
  check((await doc.locator('h1').evaluate((e) => getComputedStyle(e).color)) === 'rgb(10, 20, 30)', "the folder's page finds ../kit/site.css")
  await page.waitForTimeout(300)
  check((await doc.locator('#dot').evaluate((e: HTMLImageElement) => e.naturalWidth)) === 4, "the folder's page finds ../kit/dot.png")

  // a forged request from the page for something outside: only ever asked under the token
  await page.goto(`${origin}/`)
  await textIs(doc.locator('#title'), 'A page from a computer', 'the page renders once more')
  const seen = requests.length
  await doc.locator('body').evaluate(() => {
    top!.postMessage({ tag: 'inyon-pageview-1', t: 'get', id: 9001, url: 'https://elsewhere.invalid/api/hosts' }, '*')
    top!.postMessage({ tag: 'inyon-pageview-1', t: 'get', id: 9002, url: 'https://page.invalid/%2e%2e/%2e%2e/etc/passwd' }, '*')
  })
  await page.waitForTimeout(1000)
  const asked = requests.slice(seen)
  // the page may still be loading its own files; the forged ones show up only as paths under the token
  check(asked.includes(`${BASE}etc/passwd`) && asked.every((r) => r.startsWith(BASE)) && !asked.some((r) => r.includes('elsewhere')), `a forged request is asked for only inside the token: ${asked.join(' ') || 'nothing'}`)

  // the 404s are the app's own requests for files the page asked for and doesn't have
  const unexpected = logged.filter((l) => !/example\.com|Content Security Policy|status of 404/i.test(l))
  check(!unexpected.length, `no errors in the page: ${unexpected.join(' / ')}`)
}

async function showSite(page: Page, logged: string[]) {
  const out = path.join(import.meta.dir, '../.scratch')
  for (const part of ['', 'mockups/', 'matrix.html']) {
    if (part && !existsSync(path.join(site, part))) continue
    await page.goto(`${origin}/?part=${encodeURIComponent(part)}`)
    const doc: FrameLocator = page.frameLocator('#viewer').frameLocator('#page')
    await doc.locator('body *').first().waitFor({ timeout: 20_000 })
    await page.waitForTimeout(2500)
    const shot = path.join(out, `pageview-${part.replace(/\W/g, '') || 'plan'}.png`)
    await page.screenshot({ path: shot })
    console.log(`  ${part || 'plan'}: ${shot}`)
  }
  const errors = logged.filter((l) => !/status of 404/.test(l))
  check(!errors.length, `no errors in the page: ${errors.join(' / ')}`)
}

clearTimeout(watchdog)
if (problems.length) {
  console.error(`pageview: ${problems.length} of ${checks} checks failed`)
  for (const p of problems) console.error(`  - ${p}`)
  process.exit(1)
}
console.log(`pageview: ${checks} checks passed`)
