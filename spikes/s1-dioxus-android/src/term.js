// Runs once through document::eval. Opens xterm.js at the 200x50 resting grid, tries the WebGL
// renderer, prints a few thousand lines and reports back to Rust over dioxus.send().
const LINES = 5000
const css = getComputedStyle(document.documentElement)
const v = (n) => css.getPropertyValue(n).trim()
const el = document.getElementById('term')
// xterm measures the cell and builds its glyph atlas when it opens: with the font not loaded yet it
// measures the fallback and draws clipped glyphs
await document.fonts.load("11px 'Geist Mono'")
await document.fonts.ready
const term = new Terminal({
  cols: 200,
  rows: 50,
  scrollback: 10000,
  fontFamily: "'Geist Mono', ui-monospace, monospace",
  fontSize: 11,
  cursorBlink: true,
  allowProposedApi: true,
  theme: { background: v('--code'), foreground: v('--fg'), cursor: v('--fg'), selectionBackground: v('--accent-soft') },
})
term.open(el)
let renderer = 'dom'
let webglError = null
try {
  const gl = new WebglAddon.WebglAddon()
  gl.onContextLoss(() => { gl.dispose(); dioxus.send(JSON.stringify({ kind: 'log', text: 'webgl context lost' })) })
  term.loadAddon(gl)
  window.__gl = gl
  renderer = 'webgl'
} catch (e) {
  webglError = String(e && e.message || e)
}
term.onData((d) => dioxus.send(JSON.stringify({ kind: 'term-input', data: d })))

const colors = [31, 32, 33, 34, 35, 36, 37, 90]
const start = performance.now()
let chunk = ''
const writes = []
for (let i = 1; i <= LINES; i++) {
  const c = colors[i % colors.length]
  chunk += `\x1b[${c}m${String(i).padStart(5, ' ')}\x1b[0m  agent output line ${i}: ${'lorem ipsum dolor sit amet '.repeat(5)}\r\n`
  if (i % 250 === 0) { writes.push(chunk); chunk = '' }
}
writes.push('\x1b[1mdone\x1b[0m $ ')
await new Promise((resolve) => {
  writes.forEach((w, i) => term.write(w, i === writes.length - 1 ? resolve : undefined))
})
const parsed = performance.now() - start
// one more frame so the last rows are painted before the time is taken
await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
const painted = performance.now() - start
window.__spike = Object.assign(window.__spike || {}, {
  renderer, webglError, lines: LINES, cols: term.cols, rows: term.rows,
  writeMs: Math.round(parsed), paintedMs: Math.round(painted),
  sinceNavigationMs: Math.round(performance.now()),
  monoFont: document.fonts.check("11px 'Geist Mono'"), sansFont: document.fonts.check("15px 'Geist'"),
  origin: location.origin, dpr: devicePixelRatio, viewport: [innerWidth, innerHeight],
  ua: navigator.userAgent,
})
window.__term = term
dioxus.send(JSON.stringify({ kind: 'term-ready', ...window.__spike }))
// keep the channel open for onData
await new Promise(() => {})
