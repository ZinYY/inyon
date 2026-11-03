// The terminal view's drawing for the app at /next: xterm.js 6 with its fit, Unicode 11, links and
// WebGL addons, as the web app's TerminalView sets it up. term/build.ts bundles it into one file,
// which build.sh puts in dist/assets and names in index.html; the app loads it the first time a
// terminal opens and drives it from Rust (src/term.rs) through `window.inyonTerm`. Everything about
// the hub (term.open, term.in, reconnecting) stays in Rust; this file only draws and reads keys.

import { Terminal, type ITheme } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { WebglAddon } from '@xterm/addon-webgl'
import { DARK, LIGHT, MIN_CONTRAST, TERMINAL_FONT } from '../../web/src/components/terminal/theme.ts'
import { attachWebgl } from '../../web/src/components/terminal/webgl.ts'
import { getRenderer } from '../../web/src/lib/renderer.ts'
import { touchScroll } from '../../web/src/components/terminal/touchScroll.ts'

/** xterm.css and the symbols face (symbols.woff2, inlined), put in by build.ts */
declare const TERM_CSS: string

const isDark = () => document.documentElement.dataset.theme === 'dark'

/** The ANSI palette from the theme; ground, ink and cursor from the session's (skinned) colours. */
function themeFor(el: HTMLElement): ITheme {
  const base = isDark() ? DARK : LIGHT
  const cs = getComputedStyle(el)
  const v = (n: string) => cs.getPropertyValue(n).trim()
  const bg = v('--bg') || base.background
  const fg = v('--fg') || base.foreground
  return { ...base, background: bg, foreground: fg, cursor: fg, cursorAccent: bg }
}

/** WebGL keeps each glyph it drew: one drawn before the symbols face loaded stays a box. */
const symbolsReady = () => (document.fonts?.load ? document.fonts.load("16px 'Inyon Symbols'", '⏵').then(() => true, () => false) : Promise.resolve(false))

let styled = false
function style() {
  if (styled) return
  styled = true
  const s = document.createElement('style')
  s.textContent = TERM_CSS
  document.head.append(s)
}

export interface Handle {
  write(d: string): void
  /** a fresh screen, for a new attach (tmux draws the whole window again) */
  reset(): void
  focus(): void
  blur(): void
  focused(): boolean
  cols(): number
  rows(): number
  setFontSize(px: number): void
  hasSelection(): boolean
  selection(): string
  clearSelection(): void
  selectAll(): void
  /** text as typed by a paste: bracketed when the program asked for it */
  paste(text: string): void
  /** 'webgl' or 'dom' */
  renderer(): string
  dispose(): void
}

/**
 * Opens a terminal in `el`. `onData` gets what the keyboard, IME and pastes type; `onResize` the
 * size once the terminal is fitted to `el`, and again whenever that changes (the window, the
 * phone's keyboard, the text size).
 */
function open(el: HTMLElement, fontSize: number, onData: (d: string) => void, onResize: (cols: number, rows: number) => void): Handle {
  style()
  const term = new Terminal({
    fontFamily: TERMINAL_FONT,
    fontSize,
    lineHeight: 1.15,
    minimumContrastRatio: MIN_CONTRAST,
    cursorBlink: true,
    allowProposedApi: true,
    scrollback: 5000,
    theme: themeFor(el),
    macOptionIsMeta: true,
  })
  const fit = new FitAddon()
  term.loadAddon(fit)
  term.loadAddon(new WebLinksAddon())
  term.loadAddon(new Unicode11Addon())
  term.unicode.activeVersion = '11'
  term.open(el)

  let closed = false
  let raf = 0
  let size = ''
  const refit = () => {
    cancelAnimationFrame(raf)
    raf = requestAnimationFrame(() => {
      if (closed) return
      try {
        fit.fit()
      } catch {
        return
      }
      const now = `${term.cols}x${term.rows}`
      if (now !== size) {
        size = now
        onResize(term.cols, term.rows)
      }
    })
  }
  // WebGL when the browser has it, the default renderer otherwise (and after a lost context)
  const webgl = attachWebgl(term, async () => ({ WebglAddon }), () => {
    el.dataset.renderer = getRenderer()
    refit()
  })
  el.dataset.renderer = getRenderer()
  void symbolsReady().then((loaded) => {
    if (!loaded || closed) return
    term.clearTextureAtlas()
    term.refresh(0, term.rows - 1)
  })

  // Agents ask the terminal for its colours (OSC 4/10/11/12). An answer from here would go back
  // through tmux as keystrokes and land in the agent's input box as junk; tmux answers for the pane.
  for (const code of [4, 10, 11, 12]) term.parser.registerOscHandler(code, (data) => data.includes('?'))
  // Ctrl/Cmd+V: the browser's paste event brings the text. ^V would make Codex look for an image
  // in the machine's own clipboard, which a server doesn't have.
  term.attachCustomKeyEventHandler((e) => !(e.type === 'keydown' && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'v'))
  term.onData(onData)

  // a finger drag scrolls tmux's history, as the wheel does
  const untouch = touchScroll(el, () => term.rows)
  // the phone's keyboard: where it covers the page without resizing it (iOS), the rows under it
  // go, so the line being typed stays in sight
  const vv = window.visualViewport
  const keyboard = () => {
    if (!vv) return
    const covered = Math.max(0, window.innerHeight - vv.height - vv.offsetTop)
    el.style.paddingBottom = covered > 0 ? `${covered}px` : ''
    refit()
  }
  vv?.addEventListener('resize', keyboard)
  const ro = new ResizeObserver(refit)
  ro.observe(el)
  // the theme or the session's skin changed
  const mo = new MutationObserver(() => {
    term.options.theme = themeFor(el)
  })
  mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] })
  // the skin goes on the session, often after the terminal opened (the agent wasn't known yet)
  const session = el.closest('.session')
  if (session) mo.observe(session, { attributes: true, attributeFilter: ['class'] })
  fit.fit()
  size = `${term.cols}x${term.rows}`
  queueMicrotask(() => !closed && onResize(term.cols, term.rows))

  return {
    write: (d) => term.write(d),
    reset: () => term.reset(),
    focus: () => term.focus(),
    blur: () => term.blur(),
    focused: () => !!term.textarea && document.activeElement === term.textarea,
    cols: () => term.cols,
    rows: () => term.rows,
    setFontSize(px) {
      term.options.fontSize = px
      refit()
    },
    hasSelection: () => term.hasSelection(),
    selection: () => term.getSelection(),
    clearSelection: () => term.clearSelection(),
    selectAll: () => term.selectAll(),
    paste: (text) => term.paste(text),
    renderer: () => getRenderer(),
    dispose() {
      closed = true
      cancelAnimationFrame(raf)
      vv?.removeEventListener('resize', keyboard)
      ro.disconnect()
      mo.disconnect()
      untouch()
      webgl.dispose()
      term.dispose()
    },
  }
}

declare global {
  interface Window {
    inyonTerm?: { open: typeof open; renderer: () => string }
  }
}
// renderer: how terminals draw on this device ('webgl' or 'dom'), for Settings › About
window.inyonTerm = { open, renderer: () => getRenderer() }
