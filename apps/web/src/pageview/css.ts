import { dataUrl, isVirtual, textOf, type Load } from './protocol.ts'

/**
 * A stylesheet with every file it names (fonts, images, other stylesheets it imports) put in as
 * data: addresses. In the viewer a stylesheet is a data: address itself, where a relative url()
 * would mean nothing, and the frame may load nothing from anywhere else.
 */

const IMPORT = /@import\s+(?:url\(\s*)?(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|([^\s"');]+))\s*\)?([^;]*);/gi
const URL_FN = /url\(\s*(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|([^\s"')]*))\s*\)/gi

/** How deep imports are followed; a loop of imports stops here. */
const MAX_DEPTH = 8

const unescape = (s: string) => s.replace(/\\(.)/g, '$1')

/** Where a url() points, against the stylesheet's own address; undefined for one left as it is. */
function target(raw: string, base: string): string | undefined {
  const v = unescape(raw.trim())
  // a fragment (an SVG filter in the same document) and a data: address stay
  if (!v || v.startsWith('#') || /^data:/i.test(v)) return
  try {
    const u = new URL(v, base)
    u.hash = ''
    return isVirtual(u.href) ? u.href : undefined
  } catch {
    return
  }
}

/** The stylesheet at `base` with its imports and url()s put in. A file that can't be had stays as it was. */
export async function inlineCss(css: string, base: string, load: Load, depth = 0): Promise<string> {
  if (!/url\(|@import/i.test(css)) return css
  let text = css
  if (depth < MAX_DEPTH && /@import/i.test(text)) {
    const imports = [...text.matchAll(IMPORT)]
    const parts = await Promise.all(
      imports.map(async (m) => {
        const url = target(m[1] ?? m[2] ?? m[3] ?? '', base)
        const got = url ? await load(url) : null
        if (!got) return m[0]
        const inner = await inlineCss(textOf(got.body), got.url, load, depth + 1)
        const media = m[4]!.trim()
        return media ? `@media ${media} {\n${inner}\n}` : inner
      }),
    )
    let k = 0
    text = text.replace(IMPORT, () => parts[k++]!)
  }
  const urls = [...new Set([...text.matchAll(URL_FN)].map((m) => target(m[1] ?? m[2] ?? m[3] ?? '', base)).filter((u): u is string => !!u))]
  if (!urls.length) return text
  const files = new Map<string, string>()
  await Promise.all(
    urls.map(async (u) => {
      const got = await load(u)
      if (got) files.set(u, dataUrl(got.type, got.body))
    }),
  )
  return text.replace(URL_FN, (whole, a?: string, b?: string, c?: string) => {
    const raw = a ?? b ?? c ?? ''
    const u = target(raw, base)
    const file = u && files.get(u)
    if (!file) return whole
    // the fragment stays (an SVG sprite's #id)
    const hash = unescape(raw.trim()).split('#')[1]
    return `url("${file}${hash !== undefined ? `#${hash}` : ''}")`
  })
}
