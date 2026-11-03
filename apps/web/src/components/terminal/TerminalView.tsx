import { useEffect, useRef, useState } from 'react'
import clsx from 'clsx'
import { Terminal, type ITheme } from '@xterm/xterm'
import { DARK, LIGHT, MIN_CONTRAST, TERMINAL_FONT } from './theme.ts'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import '@xterm/xterm/css/xterm.css'
import './symbols.css'
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, ClipboardPaste, Copy, Keyboard, Minus, Plus, RotateCw, TextSelect } from 'lucide-react'
import type { AgentKind } from '@inyon/protocol'
import { socket } from '../../lib/socket.ts'
import { useStore } from '../../lib/store.ts'
import { Button } from '../ui/primitives.tsx'
import { agentDef } from '@inyon/protocol'
import { AgentLogo } from '../../lib/skin.tsx'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { copyText } from '../../lib/clipboard.ts'
import { readClipboard } from '../../lib/native.ts'
import { startingNote } from '../../lib/starting.ts'
import { touchScroll } from './touchScroll.ts'
import { attachWebgl } from './webgl.ts'
import { contextMenu } from '../ui/ContextMenu.tsx'

/**
 * WebGL draws each glyph once and keeps it: a glyph drawn before the symbols face loaded stays a
 * box. Load it up front, and redraw with it once it's in.
 */
const symbolsReady = () => (typeof document !== 'undefined' && document.fonts?.load ? document.fonts.load("16px 'Inyon Symbols'", '⏵').then(() => true, () => false) : Promise.resolve(false))

const isDark = () => document.documentElement.dataset.theme === 'dark'

/** ANSI palette from the theme, ground/ink/cursor from the (possibly agent-skinned) CSS variables. */
function themeFor(el: HTMLElement | null): ITheme {
  const base = isDark() ? DARK : LIGHT
  if (!el) return base
  const cs = getComputedStyle(el)
  const v = (n: string) => cs.getPropertyValue(n).trim()
  return { ...base, background: v('--bg') || base.background, foreground: v('--fg') || base.foreground, cursor: v('--accent') || base.cursor, cursorAccent: v('--bg') || base.cursorAccent }
}
const touch = typeof matchMedia !== 'undefined' && matchMedia('(pointer: coarse)').matches

export function TerminalView({ host, pane, agent }: { host: string; pane: string; agent?: AgentKind }) {
  const el = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const refit = useRef<() => void>(() => {})
  const idRef = useRef('')
  const mods = useRef({ ctrl: false, alt: false })
  const [, force] = useState(0)
  const [exited, setExited] = useState<string | null>(null)
  const [generation, setGeneration] = useState(0)
  const fontSize = useStore((s) => s.settings.termFontSize)
  const theme = useStore((s) => s.settings.theme)
  const connected = useStore((s) => s.connected)
  const setSettings = useStore((s) => s.setSettings)

  useEffect(() => {
    if (!el.current || !connected) return
    setExited(null)
    const id = Math.random().toString(36).slice(2)
    idRef.current = id
    const term = new Terminal({
      fontFamily: TERMINAL_FONT,
      fontSize: useStore.getState().settings.termFontSize,
      lineHeight: 1.15,
      minimumContrastRatio: MIN_CONTRAST,
      cursorBlink: true,
      allowProposedApi: true,
      scrollback: 5000,
      theme: themeFor(el.current),
      macOptionIsMeta: true,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.loadAddon(new WebLinksAddon())
    term.loadAddon(new Unicode11Addon())
    term.unicode.activeVersion = '11'
    term.open(el.current)
    termRef.current = term
    fit.fit()
    // WebGL when the browser has it, the default renderer otherwise (and after a lost context)
    const webgl = attachWebgl(term, undefined, () => refit.current())
    let closed = false
    void symbolsReady().then((loaded) => {
      if (!loaded || closed) return
      term.clearTextureAtlas()
      term.refresh(0, term.rows - 1)
    })

    // Agents ask the terminal for its colours (OSC 4/10/11/12). If this browser terminal answers,
    // the reply goes back through tmux as keystrokes and lands in the agent's input box as junk
    // like "b3b/5454…". Swallow the questions instead; tmux answers them for the pane itself.
    for (const code of [4, 10, 11, 12]) term.parser.registerOscHandler(code, (data) => data.includes('?'))
    const reply = /^\x1b\](?:4|1[0-2]);[^\x07\x1b]*(?:\x07|\x1b\\)$/
    const send = (d: string) => { if (!reply.test(d)) socket.send({ t: 'term.in', id, d }) }
    // Ctrl/Cmd+V: the browser's paste event delivers the text. Passing the key on as ^V would make
    // Codex try to read an image from the machine's own clipboard, which a server doesn't have.
    term.attachCustomKeyEventHandler((e) => !(e.type === 'keydown' && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'v'))
    // a pasted image is uploaded to the machine, and its path typed in (agents attach image paths)
    const host_ = el.current
    const onPaste = (e: ClipboardEvent) => {
      const file = [...(e.clipboardData?.files ?? [])].find((f) => f.type.startsWith('image/'))
      if (!file) return
      e.preventDefault()
      e.stopPropagation()
      api.upload(host, pane, file).then(
        (u) => send(`\x1b[200~${u.path}\x1b[201~`),
        (err) => toast({ title: 'Could not upload the image', body: (err as Error).message, tone: 'error' }),
      )
    }
    host_.addEventListener('paste', onPaste, true)
    // a finger drag scrolls tmux's history, as the mouse wheel does
    const untouch = touchScroll(host_, () => term.rows)
    term.onData((d) => {
      // apply sticky Ctrl/Alt from the touch key bar
      const m = mods.current
      if ((m.ctrl || m.alt) && d.length === 1) {
        let out = d
        if (m.ctrl) {
          const c = d.toUpperCase().charCodeAt(0)
          if (c >= 64 && c <= 95) out = String.fromCharCode(c - 64)
        }
        if (m.alt) out = `\x1b${out}`
        mods.current = { ctrl: false, alt: false }
        force((n) => n + 1)
        send(out)
        return
      }
      send(d)
    })

    const off = socket.on((m) => {
      if (m.t === 'term.data' && m.id === id) term.write(m.d)
      if (m.t === 'term.exit' && m.id === id) setExited(m.reason ?? 'Detached')
    })
    socket.send({ t: 'term.open', id, host, pane, cols: term.cols, rows: term.rows })
    if (!touch) term.focus()

    let raf = 0
    refit.current = () => {
      cancelAnimationFrame(raf)
      raf = requestAnimationFrame(() => {
        try {
          fit.fit()
          socket.send({ t: 'term.resize', id, cols: term.cols, rows: term.rows })
        } catch {}
      })
    }
    const ro = new ResizeObserver(() => refit.current())
    ro.observe(el.current)

    return () => {
      closed = true
      host_.removeEventListener('paste', onPaste, true)
      untouch()
      ro.disconnect()
      off()
      socket.send({ t: 'term.close', id })
      webgl.dispose()
      term.dispose()
      termRef.current = null
    }
  }, [host, pane, connected, generation])

  useEffect(() => {
    const t = termRef.current
    if (!t) return
    t.options.fontSize = fontSize
    t.options.theme = themeFor(el.current)
    refit.current()
  }, [fontSize, theme, agent])

  const key = (d: string) => {
    socket.send({ t: 'term.in', id: idRef.current, d })
    termRef.current?.focus()
  }
  const copySelection = async () => {
    const sel = termRef.current?.getSelection()
    if (!sel) return
    await copyText(sel)
    termRef.current?.clearSelection()
  }
  const paste = async () => {
    try {
      const t = await readClipboard()
      if (t) socket.send({ t: 'term.in', id: idRef.current, d: `\x1b[200~${t}\x1b[201~` })
      else toast({ title: 'Nothing to paste', body: 'Copy some text first, then tap Paste.', tone: 'info' })
    } catch {
      toast({ title: "Couldn't read the clipboard", body: 'Long-press in the terminal and paste from the keyboard instead.', tone: 'error' })
    }
    termRef.current?.focus()
  }
  const toggle = (m: 'ctrl' | 'alt') => {
    mods.current = { ...mods.current, [m]: !mods.current[m] }
    force((n) => n + 1)
    termRef.current?.focus()
  }

  const installed = useStore((s) => s.hosts.find((h) => h.config.id === host)?.agents) ?? []
  const start = async (kind: (typeof installed)[number]) => {
    try {
      await api.start(host, pane, kind)
      toast({ title: `Starting ${agentDef(kind).name}…`, body: startingNote(kind), tone: 'info', agent: kind })
    } catch (e) {
      toast({ title: 'Could not start', body: (e as Error).message, tone: 'error' })
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      {!agent && installed.length > 0 && (
        <div className="no-scrollbar flex shrink-0 items-center gap-1.5 overflow-x-auto border-b border-line px-3 py-1.5">
          <span className="shrink-0 pr-1 text-[12px] text-fg-3">Start here:</span>
          {installed.map((k) => (
            <button key={k} type="button" onClick={() => start(k)} className="flex h-7 shrink-0 items-center gap-1.5 rounded-full border border-line px-2 text-[12px] font-medium hover:bg-hover">
              <AgentLogo kind={k} size={15} /> {agentDef(k).name}
            </button>
          ))}
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        <div
          ref={el}
          className="absolute inset-0"
          onClick={() => termRef.current?.focus()}
          {...contextMenu(
            () => [
              !!termRef.current?.hasSelection() && { key: 'copy', label: 'Copy', icon: <Copy className="size-4" />, onSelect: () => void copySelection() },
              { key: 'paste', label: 'Paste', icon: <ClipboardPaste className="size-4" />, onSelect: () => void paste() },
              { key: 'all', label: 'Select all', icon: <TextSelect className="size-4" />, separated: true, onSelect: () => termRef.current?.selectAll() },
            ],
            { ownsTextBox: true },
          )}
        />
        {exited && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-bg">
            <p className="text-sm text-fg-2">{exited === 'Detached' ? 'Terminal closed' : exited}</p>
            <Button onClick={() => setGeneration((g) => g + 1)}>
              <RotateCw className="size-4" /> Reconnect
            </Button>
          </div>
        )}
      </div>
      <div className="pb-safe border-t border-line bg-sidebar">
        <div className="flex items-center gap-1 overflow-x-auto px-1.5 py-1.5 [scrollbar-width:none]">
          <KeyBtn onClick={() => key('\x1b')}>esc</KeyBtn>
          <KeyBtn onClick={() => key('\t')}>tab</KeyBtn>
          <KeyBtn active={mods.current.ctrl} onClick={() => toggle('ctrl')}>ctrl</KeyBtn>
          <KeyBtn active={mods.current.alt} onClick={() => toggle('alt')}>alt</KeyBtn>
          <KeyBtn onClick={() => key('\x1b[A')} label="Up"><ArrowUp className="size-4" /></KeyBtn>
          <KeyBtn onClick={() => key('\x1b[B')} label="Down"><ArrowDown className="size-4" /></KeyBtn>
          <KeyBtn onClick={() => key('\x1b[D')} label="Left"><ArrowLeft className="size-4" /></KeyBtn>
          <KeyBtn onClick={() => key('\x1b[C')} label="Right"><ArrowRight className="size-4" /></KeyBtn>
          <KeyBtn onClick={() => key('\x03')}>^C</KeyBtn>
          <KeyBtn onClick={() => key('\x12')} label="Search history (Ctrl+R)">^R</KeyBtn>
          {['|', '/', '-', '~', '`'].map((c) => (
            <KeyBtn key={c} onClick={() => key(c)}>{c}</KeyBtn>
          ))}
          <div className="mx-1 h-5 w-px shrink-0 bg-line" />
          <KeyBtn label="Copy selection" onClick={copySelection}><Copy className="size-4" /></KeyBtn>
          <KeyBtn label="Paste" onClick={paste}><ClipboardPaste className="size-4" /></KeyBtn>
          <KeyBtn label="Smaller text" onClick={() => setSettings({ termFontSize: Math.max(9, fontSize - 1) })}><Minus className="size-4" /></KeyBtn>
          <KeyBtn label="Larger text" onClick={() => setSettings({ termFontSize: Math.min(22, fontSize + 1) })}><Plus className="size-4" /></KeyBtn>
          {touch && (
            <KeyBtn label="Keyboard" onClick={() => termRef.current?.focus()}><Keyboard className="size-4" /></KeyBtn>
          )}
        </div>
      </div>
    </div>
  )
}

function KeyBtn({ children, onClick, active, label }: { children: React.ReactNode; onClick: () => void; active?: boolean; label?: string }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      // keep focus (and the phone keyboard) on the terminal
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={clsx(
        'press flex h-9 min-w-10 shrink-0 items-center justify-center rounded-lg border px-2.5 font-mono text-[13px]',
        active ? 'border-accent bg-accent text-accent-fg' : 'border-line bg-elev text-fg-2 hover:text-fg',
      )}
    >
      {children}
    </button>
  )
}
