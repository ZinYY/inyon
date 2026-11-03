// bun apps/app/term/build.ts <out dir> — bundles term.ts with xterm.js and its addons into one
// minified file named by its content (the hub serves /next/assets/ as never changing), with
// xterm.css and the symbols face inlined so the terminal needs nothing else. Prints the file's name.
import { readFileSync } from 'node:fs'
import path from 'node:path'

const out = process.argv[2]
if (!out) throw new Error('usage: bun apps/app/term/build.ts <out dir>')

const web = path.join(import.meta.dir, '../../web/src/components/terminal')
const xtermCss = readFileSync(Bun.resolveSync('@xterm/xterm/css/xterm.css', import.meta.dir), 'utf8')
const font = readFileSync(path.join(web, 'symbols.woff2')).toString('base64')
// the web app's symbols.css, with the font in the file rather than beside it
const symbols = readFileSync(path.join(web, 'symbols.css'), 'utf8').replace('url(./symbols.woff2)', `url(data:font/woff2;base64,${font})`)
if (!symbols.includes('base64,')) throw new Error('symbols.css no longer names ./symbols.woff2')

const r = await Bun.build({
  entrypoints: [path.join(import.meta.dir, 'term.ts')],
  outdir: out,
  naming: 'term-[hash].[ext]',
  target: 'browser',
  format: 'esm',
  minify: true,
  define: { TERM_CSS: JSON.stringify(`${xtermCss}\n${symbols}`) },
})
if (!r.success) {
  for (const l of r.logs) console.error(l)
  process.exit(1)
}
const [file] = r.outputs.filter((o) => o.kind === 'entry-point')
console.log(path.basename(file!.path))
