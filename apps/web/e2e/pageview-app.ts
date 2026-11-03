// The app's half of e2e/pageview.ts, bundled for the browser: the frame the Pages viewer puts up
// for a page on a paired computer, answered by the real bridge (src/lib/pageBridge.ts). The test
// server stands in for the phone core at /v/<token>/.
import { bridgeFrame, virtualSrc } from '../src/lib/pageBridge.ts'

const params = new URLSearchParams(location.search)
const base = params.get('base') ?? '/v/tok/'
const src = `${base}${params.get('part') ?? ''}?theme=dark`
const frame = document.createElement('iframe')
frame.id = 'viewer'
frame.title = 'page'
frame.setAttribute('sandbox', 'allow-scripts allow-forms allow-popups allow-downloads')
frame.referrerPolicy = 'no-referrer'
frame.src = '/pageview.html'
document.body.append(frame)
bridgeFrame({ win: frame.contentWindow!, base, url: virtualSrc(src, base), theme: 'dark', fetch: (path, method) => fetch(path, { method }) })
