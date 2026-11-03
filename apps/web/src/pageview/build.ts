/**
 * Builds pageview.html, the loader for a page on a paired computer (see protocol.ts): frame.ts
 * bundled into one classic inline script (the Android app's local server sends no CORS header, so
 * a module script from it wouldn't run in an opaque origin), under a CSP of its own. Not part of
 * the app: vite.config.ts and the browser check (e2e/pageview.ts) use it.
 */
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import type { Plugin } from 'vite'

const ENTRY = fileURLToPath(new URL('./frame.ts', import.meta.url))

/**
 * The loader's policy, which every document of the page takes from it (a srcdoc document has its
 * parent's): files only as data: and blob: addresses, which the viewer makes from what the app
 * fetched, and no network at all, as the hub allows a page nothing outside its own link. The base
 * may only be the page's made-up address, and frames only srcdoc documents.
 */
export const PAGEVIEW_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' data: blob:",
  "style-src 'unsafe-inline' data: blob:",
  'img-src data: blob:',
  'font-src data: blob:',
  'media-src data: blob:',
  'connect-src data: blob:',
  "frame-src 'none'",
  'base-uri https://page.invalid',
  "form-action 'none'",
  "object-src 'none'",
].join('; ')

async function bundle(): Promise<string> {
  if (typeof Bun !== 'undefined') {
    const r = await Bun.build({ entrypoints: [ENTRY], format: 'iife', minify: true, target: 'browser' })
    if (!r.success) throw new AggregateError(r.logs, 'Could not bundle the page viewer')
    return r.outputs[0]!.text()
  }
  // the dev server under Node
  return execFileSync('bun', ['build', ENTRY, '--format=iife', '--minify', '--target=browser'], { encoding: 'utf8', maxBuffer: 16 << 20 })
}

/** The loader page. */
export async function pageviewHtml(): Promise<string> {
  // the same source goes in a <script> in every document of the page: nothing in it may end the tag
  const js = (await bundle()).replace(/<\/(script)/gi, '<\\/$1')
  if (/<!--/.test(js)) throw new Error('The page viewer bundle has "<!--" in it, which changes how a browser reads an inline script')
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${PAGEVIEW_CSP}">
<meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>html,body{margin:0;height:100%;overflow:hidden;background:transparent}iframe{display:block;border:0;width:100%;height:100%}</style>
</head>
<body><iframe id="page" title="Page"></iframe><script>${js}</script></body>
</html>
`
}

/** Emits pageview.html with the build, and serves it in development. */
export function pageview(): Plugin {
  return {
    name: 'inyon-pageview',
    async generateBundle() {
      this.emitFile({ type: 'asset', fileName: 'pageview.html', source: await pageviewHtml() })
    },
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.url?.split('?')[0] !== '/pageview.html') return next()
        pageviewHtml().then(
          (html) => {
            res.setHeader('content-type', 'text/html; charset=utf-8')
            res.end(html)
          },
          (e) => next(e),
        )
      })
    },
  }
}
