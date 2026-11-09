// bun apps/app/term/build.ts <out dir> — bundles term.ts with xterm.js and its addons into one
// minified file named by its content (the hub serves /next/assets/ as never changing), with
// xterm.css and the symbols faces inlined so the terminal needs nothing else. Prints the file's name.
import { readFileSync } from 'node:fs'
import path from 'node:path'

const out = process.argv[2]
if (!out) throw new Error('usage: bun apps/app/term/build.ts <out dir>')

const web = path.join(import.meta.dir, '../../web/src/components/terminal')
const xtermCss = readFileSync(Bun.resolveSync('@xterm/xterm/css/xterm.css', import.meta.dir), 'utf8')
// Inline the same faces as the web terminal, including its private-use icons.
const symbols = readFileSync(path.join(web, 'symbols.css'), 'utf8').replace(/url\(\.\/([\w.-]+\.woff2)\)/g, (_, name: string) =>
  `url(data:font/woff2;base64,${readFileSync(path.join(web, name)).toString('base64')})`)
if (!symbols.includes('base64,') || symbols.includes('url(./')) throw new Error('Unbundled terminal font')

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
