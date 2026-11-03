import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { appendTo, onCompose } from '../../lib/compose.ts'
import clsx from 'clsx'
import { ArrowUp, CalendarClock, Camera, Clock3, FileText, Folder, Hourglass, Paperclip, RotateCw, Square, SquareTerminal, X, Zap } from 'lucide-react'
import { agentDef, chatKey, type AgentKind, type SlashCommand } from '@inyon/protocol'
import { haptic } from '../../lib/haptics.ts'
import { api, isTransient, withRetries, type Uploaded } from '../../lib/api.ts'
import { canSchedule, patchChat, toast, useStore } from '../../lib/store.ts'
import { SendLaterSheet } from '../session/OrganizeSheets.tsx'
import { socket } from '../../lib/socket.ts'
import { nativeFile, onNative, plugin, type NativeFile } from '../../lib/native.ts'
import { bytes } from '../../lib/format.ts'
import { openMenu } from '../ui/primitives.tsx'
import { contextMenu } from '../ui/ContextMenu.tsx'
import { keyLabel } from '../../lib/keys.ts'

/** commands that open a picker in the agent's terminal; the composer's own menus (and OpenCode's providers sheet) do the job */
const MENU_COMMANDS: Record<string, string> = { '/model': 'model', '/models': 'model', '/effort': 'effort', '/variants': 'effort', '/agents': 'mode', '/agent': 'mode', '/connect': 'providers' }

/** The app's own sheet for a command typed bare, if it has one: a menu above, or past conversations (/resume). */
function menuFor(command: string, agent?: AgentKind) {
  const c = command.toLowerCase()
  const menu = MENU_COMMANDS[c]
  if (menu) return (menu !== 'mode' && menu !== 'providers') || agent === 'opencode' ? menu : undefined
  return agent && agentDef(agent).conversations?.includes(c) ? 'conversations' : undefined
}

interface Attachment {
  id: string
  name: string
  size: number
  type: string
  preview?: string
  progress: number
  status: 'uploading' | 'done' | 'error'
  /** the connection dropped mid-upload: it carries on by itself once it's back */
  waiting?: boolean
  path?: string
  error?: string
  pastedLines?: number
  /** kept until it has uploaded, so a failed one can be tried again */
  file?: File
}

const DRAFTS = new Map<string, string>()
/** per pane, the draft both sides last agreed on when the composer went away (the terminal view, another chat) */
const SYNCED = new Map<string, string>()

/**
 * Attachments per session, outside the component like the text drafts: leaving the page (or an
 * upload finishing while you're elsewhere) must not drop a file you meant to send.
 */
const ATTACH = new Map<string, Attachment[]>()
/** uploads in progress by attachment id, so removing one stops it */
const UPLOADS = new Map<string, AbortController>()
const attachSubs = new Set<() => void>()
const NO_FILES: Attachment[] = []
function updateFiles(key: string, fn: (f: Attachment[]) => Attachment[]) {
  ATTACH.set(key, fn(ATTACH.get(key) ?? NO_FILES))
  attachSubs.forEach((s) => s())
}
function useFiles(key: string) {
  return useSyncExternalStore(
    (cb) => (attachSubs.add(cb), () => attachSubs.delete(cb)),
    () => ATTACH.get(key) ?? NO_FILES,
  )
}
const coarsePointer = () => typeof matchMedia !== 'undefined' && matchMedia('(pointer: coarse)').matches
const touch = coarsePointer()
const LONG_PASTE_LINES = 40

/** Subsequence match score; higher is better, -1 for no match. */
function fuzzy(query: string, target: string): number {
  if (!query) return 0
  const q = query.toLowerCase()
  const t = target.toLowerCase()
  const at = t.indexOf(q)
  if (at >= 0) return 1000 - at - (t.length - q.length) * 0.1 + (t.lastIndexOf('/') < at ? 200 : 0)
  let ti = 0
  let score = 0
  let streak = 0
  for (const ch of q) {
    const found = t.indexOf(ch, ti)
    if (found < 0) return -1
    streak = found === ti ? streak + 1 : 0
    score += 10 + streak * 5 - (found - ti) * 0.5
    ti = found + 1
  }
  return score
}

type Popover =
  | { kind: 'slash'; query: string; start: number; end: number }
  | { kind: 'skill'; query: string; start: number; end: number }
  | { kind: 'mention'; query: string; start: number; end: number }

/** Finds a `/command` (at the start), a Codex `$skill` or an `@path` token around the cursor. */
function tokenAt(text: string, cursor: number, skills = false): Popover | null {
  const before = text.slice(0, cursor)
  const slash = before.match(/^\/([\w:.-]*)$/)
  if (slash) return { kind: 'slash', query: slash[1], start: 0, end: cursor }
  const skill = skills && before.match(/(^|\s)\$([\w:.-]*)$/)
  if (skill) return { kind: 'skill', query: skill[2], start: cursor - skill[2].length - 1, end: cursor }
  const at = before.match(/(^|\s)@([^\s@]*)$/)
  if (at) return { kind: 'mention', query: at[2], start: cursor - at[2].length - 1, end: cursor }
  return null
}

export function Composer({
  host,
  pane,
  agent,
  working,
  disabled,
  controls,
  ultracode,
}: {
  host: string
  pane: string
  agent?: AgentKind
  working: boolean
  disabled?: boolean
  /** model, effort, mode and goal pickers for this agent */
  controls?: React.ReactNode
  /** Claude Code's ultracode is on: the box carries the label Claude draws on its own */
  ultracode?: boolean
}) {
  const key = chatKey(host, pane)
  const serverDraft = useStore((s) => s.chats[key]?.draft)
  const [text, setTextState] = useState(() => DRAFTS.get(key) ?? serverDraft?.text ?? '')
  const shellMode = !!agent && agentDef(agent).shell && text.trimStart().startsWith('!')
  // Codex takes a message two ways while it works: queued for after this turn (its default), or
  // steered into the turn under way. The choice sits by the send button only then.
  const [steer, setSteer] = useState(false)
  const canSteer = agent === 'codex' && working && !shellMode
  // each turn starts from the safe choice
  useEffect(() => {
    if (!working) setSteer(false)
  }, [working])
  const [focused, setFocused] = useState(false)
  const [origin, setOrigin] = useState<'app' | 'terminal' | null>(null)
  /**
   * the text both sides last agreed on, so we can tell who changed what. Coming back to a chat it
   * starts from what was agreed when the composer went away: a draft the terminal sent meanwhile
   * then reads as new and replaces the old copy, instead of looking already seen.
   */
  const synced = useRef(SYNCED.get(key) ?? serverDraft?.text ?? text)
  useEffect(() => () => void SYNCED.set(key, synced.current), [key])
  const pushTimer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const pending = useRef<(() => void) | undefined>(undefined)
  /** the message just sent, and when, so a keyboard putting it back isn't taken for typing */
  const sent = useRef<{ text: string; at: number }>(undefined)
  /** user edits: save locally and push to the hub (which mirrors into the terminal) */
  const setText = (v: string | ((p: string) => string)) => {
    setTextState((prev) => {
      const next = typeof v === 'function' ? v(prev) : v
      clearTimeout(pushTimer.current)
      const push = (pending.current = () => {
        pending.current = undefined
        if (next !== synced.current) {
          synced.current = next
          socket.send({ t: 'draft', host, pane, text: next })
          setOrigin('app')
        }
      })
      pushTimer.current = setTimeout(push, 450)
      return next
    })
  }
  // leaving the app (or reloading) mid-sentence: the draft goes now, not in half a second
  useEffect(() => {
    const now = () => {
      if (!pending.current) return
      clearTimeout(pushTimer.current)
      pending.current()
    }
    const hidden = () => document.visibilityState === 'hidden' && now()
    document.addEventListener('visibilitychange', hidden)
    window.addEventListener('pagehide', now)
    return () => {
      document.removeEventListener('visibilitychange', hidden)
      window.removeEventListener('pagehide', now)
    }
  }, [])
  // a file mention or review comments from the side panel
  useEffect(
    () =>
      onCompose(host, pane, (more) => {
        setText((v) => appendTo(v, more))
        if (!matchMedia('(pointer: coarse)').matches) requestAnimationFrame(() => ta.current?.focus())
      }),
    [host, pane],
  )
  // a draft from the terminal (or another device): adopt it unless you're mid-edit with other text
  useEffect(() => {
    if (!serverDraft) return
    if (serverDraft.text === synced.current) return
    const mine = text !== synced.current && focused
    if (mine) return
    synced.current = serverDraft.text
    setTextState(serverDraft.text)
    setOrigin(serverDraft.source)
  }, [serverDraft?.at])
  const files = useFiles(key)
  const setFiles = useCallback((v: Attachment[] | ((f: Attachment[]) => Attachment[])) => updateFiles(key, typeof v === 'function' ? v : () => v), [key])
  const [pop, setPop] = useState<Popover | null>(null)
  const [active, setActive] = useState(0)
  const [commands, setCommands] = useState<SlashCommand[] | null>(null)
  const [paths, setPaths] = useState<string[] | null>(null)
  const [dragging, setDragging] = useState(false)
  const ta = useRef<HTMLTextAreaElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const cameraInput = useRef<HTMLInputElement>(null)
  const agentName = agent ? agentDef(agent).name : 'the agent'

  useEffect(() => {
    DRAFTS.set(key, text)
  }, [key, text])

  useLayoutEffect(() => {
    const el = ta.current
    if (!el) return
    const max = window.innerHeight * 0.4
    el.style.height = '0px'
    el.style.height = `${Math.min(el.scrollHeight, max)}px`
    // phones round line heights, which leaves a 1px overflow and a pointless scrollbar
    el.style.overflowY = el.scrollHeight > max + 1 ? 'auto' : 'hidden'
  }, [text])

  // ---------- suggestions ----------

  useEffect(() => {
    // fetched each time the list opens, so a skill installed meanwhile shows up; the last list stays
    // on screen until the new one arrives
    if (pop?.kind === 'slash' || pop?.kind === 'skill') api.commands(host, pane).then(setCommands).catch(() => setCommands((c) => c ?? []))
    if (pop?.kind === 'mention' && !paths) api.files(host, pane).then((r) => setPaths(r.files)).catch(() => setPaths([]))
  }, [pop?.kind])

  const suggestions = useMemo(() => {
    if (!pop) return []
    if (pop.kind === 'slash' || pop.kind === 'skill') {
      const list = (commands ?? agentDef(agent ?? 'claude').commands).filter((c) => c.name.startsWith(pop.kind === 'skill' ? '$' : '/'))
      return list
        .map((c) => ({ c, s: fuzzy(pop.query, c.name.slice(1)) }))
        .filter((x) => x.s >= 0)
        .sort((a, b) => b.s - a.s || a.c.name.localeCompare(b.c.name))
        .slice(0, 40)
        .map(({ c }) => ({ key: c.name, label: c.name, hint: c.description, tag: c.source === 'built-in' ? undefined : c.source, item: c }))
    }
    const all = paths ?? []
    const dirs = new Set<string>()
    for (const p of all) {
      const parts = p.split('/')
      for (let i = 1; i < parts.length; i++) dirs.add(`${parts.slice(0, i).join('/')}/`)
    }
    const pool = [...all, ...dirs]
    return pool
      .map((p) => ({ p, s: fuzzy(pop.query, p) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s || a.p.length - b.p.length)
      .slice(0, 40)
      .map(({ p }) => ({ key: p, label: p, hint: undefined as string | undefined, tag: p.endsWith('/') ? 'folder' : undefined, item: p }))
  }, [pop, commands, paths, agent])

  useEffect(() => setActive(0), [pop?.query, pop?.kind])

  const updatePop = () => {
    const el = ta.current
    if (!el) return
    setPop(tokenAt(el.value, el.selectionStart ?? el.value.length, agent === 'codex'))
  }

  const pick = (i: number) => {
    const s = suggestions[i]
    if (!s || !pop) return
    if (pop.kind === 'slash') {
      const c = s.item as SlashCommand
      const menu = menuFor(c.name, agent)
      if (menu && !text.slice(pop.end).trim() && openMenu(menu)) {
        setText('')
        setPop(null)
        return
      }
      if (c.instant && !files.length) {
        setPop(null)
        send(c.name)
        return
      }
      const next = `${c.name} ${text.slice(pop.end).trimStart()}`
      setText(next)
      requestAnimationFrame(() => ta.current?.setSelectionRange(c.name.length + 1, c.name.length + 1))
    } else if (pop.kind === 'skill') {
      const ins = `${(s.item as SlashCommand).name} `
      setText(text.slice(0, pop.start) + ins + text.slice(pop.end))
      const at = pop.start + ins.length
      requestAnimationFrame(() => ta.current?.setSelectionRange(at, at))
    } else {
      const ins = `@${s.item as string}${(s.item as string).endsWith('/') ? '' : ' '}`
      const next = text.slice(0, pop.start) + ins + text.slice(pop.end)
      setText(next)
      const at = pop.start + ins.length
      requestAnimationFrame(() => ta.current?.setSelectionRange(at, at))
    }
    setPop(null)
    ta.current?.focus()
  }

  // ---------- attachments ----------

  const upload = useCallback(
    (id: string, file: File) => {
      const set = (fn: (a: Attachment) => Attachment) => updateFiles(key, (f) => f.map((a) => (a.id === id ? fn(a) : a)))
      const stop = new AbortController()
      UPLOADS.set(id, stop)
      api
        .upload(host, pane, file, (progress, waiting) => set((a) => ({ ...a, progress, waiting })), stop.signal)
        .then((u: Uploaded) => set((a) => ({ ...a, status: 'done', progress: 1, waiting: false, path: u.path, file: undefined })))
        .catch((e) => !stop.signal.aborted && set((a) => ({ ...a, status: 'error', waiting: false, error: e.message })))
        .finally(() => UPLOADS.get(id) === stop && UPLOADS.delete(id))
    },
    [host, pane, key],
  )

  const addFiles = useCallback(
    (list: File[], pastedLines?: number) => {
      for (const file of list) {
        const id = Math.random().toString(36).slice(2)
        const preview = file.type.startsWith('image/') ? URL.createObjectURL(file) : undefined
        updateFiles(key, (f) => [...f, { id, name: file.name || 'pasted', size: file.size, type: file.type, preview, progress: 0, status: 'uploading', pastedLines, file }])
        upload(id, file)
      }
    },
    [key, upload],
  )

  const retryFile = (a: Attachment) => {
    if (!a.file) return
    // it picks up where it stopped, so the bar stays where it was
    setFiles((f) => f.map((x) => (x.id === a.id ? { ...x, status: 'uploading', error: undefined } : x)))
    upload(a.id, a.file)
  }

  const removeFile = (id: string) => {
    // one still going stops, and what reached the machine is deleted
    UPLOADS.get(id)?.abort()
    UPLOADS.delete(id)
    setFiles((f) => {
      const a = f.find((x) => x.id === id)
      if (a?.preview) URL.revokeObjectURL(a.preview)
      return f.filter((x) => x.id !== id)
    })
  }

  /**
   * Handles a paste from the text box or from anywhere on the page. Returns true when it took
   * care of it: files and screenshots become attachments, very long text becomes a text file.
   */
  const handlePaste = (data: DataTransfer, insertText: boolean): boolean => {
    const files = clipboardFiles(data)
    if (files.length) {
      addFiles(files)
      return true
    }
    const pasted = data.getData('text/plain')
    const lines = pasted.split('\n').length
    if (lines > LONG_PASTE_LINES) {
      addFiles([new File([pasted], `pasted-${new Date().toISOString().slice(11, 19).replace(/:/g, '')}.txt`, { type: 'text/plain' })], lines)
      return true
    }
    // the phone app's own Paste can bring an image the WebView doesn't see: ask for it
    if (!pasted && plugin('InyonDevice')?.readClipboardFile) {
      plugin('InyonDevice')!.readClipboardFile().then(
        (f: Partial<NativeFile>) => f?.data && addFiles([nativeFile(f as NativeFile)]),
        (err: Error) => toast({ title: "Couldn't paste that", body: err.message, tone: 'error' }),
      )
      return true
    }
    if (insertText && pasted) {
      setText((v) => (v ? `${v}${/\s$/.test(v) ? '' : ' '}${pasted}` : pasted))
      requestAnimationFrame(() => {
        const el = ta.current
        el?.focus()
        el?.setSelectionRange(el.value.length, el.value.length)
      })
      return true
    }
    return false
  }

  const onPaste = (e: React.ClipboardEvent) => {
    if (handlePaste(e.clipboardData, false)) e.preventDefault()
  }

  // Ctrl+V anywhere on the page lands here, and typing with nothing focused starts a message
  const pasteRef = useRef(handlePaste)
  pasteRef.current = handlePaste
  useEffect(() => {
    const free = (t: EventTarget | null) => !(t instanceof HTMLElement) || !(t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.closest('.xterm, [role=dialog]'))
    const onDocPaste = (e: ClipboardEvent) => {
      if (disabled || !e.clipboardData || !free(e.target) || e.target === ta.current) return
      if (pasteRef.current(e.clipboardData, true)) e.preventDefault()
    }
    const onKey = (e: KeyboardEvent) => {
      if (disabled || touch || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || !free(e.target)) return
      if (e.key.length === 1 && !window.getSelection()?.toString()) ta.current?.focus() // the key then types into the box
    }
    document.addEventListener('paste', onDocPaste)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('paste', onDocPaste)
      document.removeEventListener('keydown', onKey)
    }
  }, [disabled])

  // the phone app: images from the keyboard (Gboard's clipboard, stickers, GIFs) arrive natively
  const addRef = useRef(addFiles)
  addRef.current = addFiles
  useEffect(() => {
    if (disabled) return
    const stops = [
      onNative<NativeFile>('InyonDevice', 'pasteFile', (f) => addRef.current([nativeFile(f)])),
      onNative<{ message: string }>('InyonDevice', 'pasteError', (e) => toast({ title: "Couldn't paste that", body: e.message, tone: 'error' })),
    ]
    return () => stops.forEach((stop) => stop())
  }, [disabled])

  // the box is what this page is for: ready to type on arrival (not on phones, where that pops the keyboard)
  useEffect(() => {
    if (!touch && !disabled) ta.current?.focus({ preventScroll: true })
  }, [key, disabled])

  // ---------- sending ----------

  const uploading = files.some((f) => f.status === 'uploading')
  // a file that didn't upload holds the message until it's retried or removed: the text often
  // refers to it ("use the attached spec"), so sending without it would lose what was meant
  const failed = files.some((f) => f.status === 'error')
  const ready = files.filter((f) => f.status === 'done' && f.path)

  const send = async (value = text) => {
    let msg = value.trim()
    if (uploading) return
    // a bare "/model" or "/resume" would open the agent's terminal menu; the app has its own
    const menu = !ready.length && menuFor(msg, agent)
    if (menu && openMenu(menu)) {
      setText('')
      setPop(null)
      return
    }
    if (failed) return
    if (ready.length) msg = `${msg}${msg ? '\n\n' : ''}Attached files:\n${ready.map((f) => f.path).join('\n')}`
    if (!msg) return
    const id = Math.random().toString(36).slice(2)
    patchChat(key, (c) => ({ pending: [...c.pending, { id, text: msg, status: 'sending', at: Date.now() }] }))
    if (value === text) {
      clearTimeout(pushTimer.current)
      pending.current = undefined
      sent.current = { text: value, at: Date.now() }
      synced.current = ''
      setTextState('')
      setOrigin(null)
      // on a touch screen the keyboard goes down, so the reply has the screen (a tablet's keyboard
      // covers half of it); with a keyboard you keep typing
      if (coarsePointer()) ta.current?.blur()
    }
    // the image previews aren't shown any more: free them (phones run out of memory otherwise)
    const gone = new Set(ready.map((f) => f.id))
    setFiles((cur) =>
      cur.filter((f) => {
        if (!gone.has(f.id)) return true
        if (f.preview) URL.revokeObjectURL(f.preview)
        return false
      }),
    )
    setPop(null)
    const steering = canSteer && steer
    try {
      // the same id on every try: the hub submits it once however many times it arrives
      await withRetries(() => api.send(host, pane, msg, id, steering || undefined))
      // the tap on Send was the tick; the agent's reply is the confirmation
      patchChat(key, (c) => ({ pending: c.pending.map((p) => (p.id === id ? { ...p, status: 'sent' } : p)) }))
    } catch (e) {
      haptic('error')
      patchChat(key, (c) => ({ pending: c.pending.map((p) => (p.id === id ? { ...p, status: 'failed', error: (e as Error).message, retryable: isTransient(e) } : p)) }))
    }
  }

  // the hub keeps it and types it in once the agent is free; it can still be edited or dropped there
  /** The box empties once the hub has the message (held, or kept to send later). */
  const handedOver = () => {
    clearTimeout(pushTimer.current)
    pending.current = undefined
    sent.current = { text, at: Date.now() }
    synced.current = ''
    setTextState('')
    setOrigin(null)
    setPop(null)
  }
  const [later, setLater] = useState(false)
  const hold = async () => {
    const msg = text.trim()
    if (!msg || uploading || files.length) return
    try {
      await api.hold(host, pane, msg)
      handedOver()
    } catch (e) {
      haptic('error')
      toast({ title: "Couldn't hold the message", body: (e as Error).message, tone: 'error' })
    }
  }

  const [stopping, setStopping] = useState(false)
  const stop = async () => {
    setStopping(true)
    haptic('warning')
    try {
      await api.interrupt(host, pane)
    } catch (e) {
      toast({ title: 'Could not stop it', body: (e as Error).message, tone: 'error' })
    } finally {
      setStopping(false)
    }
  }

  const canSend = (!!text.trim() || ready.length > 0) && !uploading && !failed && !disabled
  // while it works, an empty box offers Stop; once you type, the same button queues a follow-up
  const showStop = working && !canSend && !uploading && !failed

  return (
    // it fades in when it takes the place of a question card
    <div
      className="animate-fade-in relative"
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes('Files')) {
          e.preventDefault()
          setDragging(true)
        }
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault()
        setDragging(false)
        addFiles([...e.dataTransfer.files])
      }}
    >
      {pop && suggestions.length > 0 && (
        // it sits over the box, so it rises out of it
        <div className="animate-pop-up absolute inset-x-0 bottom-full z-20 mb-2 origin-bottom overflow-hidden rounded-3xl border border-line bg-elev shadow-float">
          <div className="flex items-center gap-2 border-b border-line px-3 py-2 text-[12px] text-fg-3">
            {pop.kind === 'slash' ? (agent === 'codex' ? 'Commands' : 'Commands and skills') : pop.kind === 'skill' ? 'Skills' : `Files in ${paths ? 'this folder' : '…'}`}
            <span className="ml-auto hidden font-mono sm:inline">↑↓ · ⏎ · esc</span>
          </div>
          <div role="listbox" className="max-h-[min(320px,45dvh)] overflow-y-auto p-1">
            {suggestions.map((s, i) => (
              <button
                key={s.key}
                type="button"
                role="option"
                aria-selected={i === active}
                onMouseEnter={() => setActive(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(i)}
                className={clsx('flex w-full items-center gap-3 rounded-2xl px-3 py-2 text-left', i === active && 'bg-hover')}
              >
                {pop.kind === 'mention' &&
                  (s.tag === 'folder' ? <Folder className="size-4 shrink-0 text-fg-3" /> : <FileText className="size-4 shrink-0 text-fg-3" />)}
                <span className={clsx('font-mono text-[13px]', pop.kind === 'slash' ? 'shrink-0' : 'min-w-0 truncate', pop.kind === 'skill' && 'max-w-[70%]')}>{s.label}</span>
                {s.hint && <span className="min-w-0 flex-1 truncate text-[12.5px] text-fg-3">{s.hint}</span>}
                {s.tag && pop.kind !== 'mention' && (
                  <span className="ml-auto shrink-0 rounded-full border border-line px-2 py-px font-mono text-[12px] text-fg-3">{s.tag}</span>
                )}
              </button>
            ))}
          </div>
        </div>
      )}

      <div
        className={clsx(
          'rounded-3xl border bg-elev shadow-card transition-colors',
          dragging ? 'border-accent' : 'border-line focus-within:border-line-strong',
          disabled && 'opacity-60',
          ultracode && 'ultracode-box',
        )}
      >
        {ultracode && <span className="ultracode-label" aria-hidden>ultracode</span>}
        {files.length > 0 && (
          <div className="no-scrollbar flex gap-2 overflow-x-auto px-2.5 pt-2.5 pb-0.5">
            {files.map((f) => (
              <div key={f.id} className="group/att animate-fade-in relative shrink-0">
                {f.preview ? (
                  <div className="relative size-14 overflow-hidden rounded-xl border border-line bg-sunken">
                    <img src={f.preview} alt={f.name} className="size-full object-cover" />
                    {f.status === 'error' && (
                      <div className="absolute inset-0 flex items-center justify-center bg-black/45 text-white">
                        <RotateCw className="size-4" />
                      </div>
                    )}
                    {f.status === 'uploading' && (
                      <>
                        <div className="absolute inset-0 bg-black/35" />
                        <div className="absolute inset-x-0 bottom-0 h-[3px] bg-white/25">
                          <div className="h-full origin-left bg-white transition-transform" style={{ transform: `scaleX(${f.progress})` }} />
                        </div>
                      </>
                    )}
                  </div>
                ) : (
                  <div className="flex h-14 max-w-[220px] items-center gap-2.5 rounded-xl border border-line bg-sunken px-3">
                    {f.status === 'error' ? <RotateCw className="size-4 shrink-0 text-danger" /> : <FileText className="size-4 shrink-0 text-fg-2" />}
                    <div className="min-w-0">
                      <div className="truncate text-[12.5px] font-medium">{f.pastedLines ? 'Pasted text' : f.name}</div>
                      <div className="truncate text-[12px] text-fg-3">
                        {f.status === 'uploading' ? (f.waiting ? 'Reconnecting…' : `${Math.round(f.progress * 100)}%`) : f.status === 'error' ? f.error : f.pastedLines ? `${f.pastedLines} lines` : bytes(f.size)}
                      </div>
                    </div>
                  </div>
                )}
                {f.status === 'error' && (
                  <button
                    type="button"
                    aria-label={`Retry ${f.name}`}
                    title={`${f.error ?? "Couldn't upload"} · tap to try again`}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => retryFile(f)}
                    className="absolute inset-0 rounded-xl ring-2 ring-danger"
                  />
                )}
                <button
                  type="button"
                  aria-label={`Remove ${f.name}`}
                  onClick={() => removeFile(f.id)}
                  className="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full border-2 border-elev bg-fg text-bg"
                >
                  <X className="size-3" strokeWidth={3} />
                </button>
              </div>
            ))}
          </div>
        )}
        {/* inset like the text below it: at the box's edge it ran over the rounded corner */}
        {shellMode && (
          <div className="flex items-center gap-1.5 px-4 pt-2.5 text-[12px] text-fg-3">
            <SquareTerminal className="size-3.5 shrink-0" />
            <span className="truncate">Shell command · runs in this session’s folder</span>
          </div>
        )}
        <textarea
          ref={ta}
          rows={1}
          value={text}
          disabled={disabled}
          onChange={(e) => {
            // The Send button keeps the keyboard open, and an Android keyboard that was still
            // composing puts its text back once the box is emptied under it: that isn't typing,
            // and taking it for a draft brought the sent message back into the box
            const v = e.target.value
            const s = sent.current
            if (s && !text && v.trim().length > 1 && Date.now() - s.at < 2000 && s.text.trim().endsWith(v.trim())) {
              e.target.value = ''
              return
            }
            setText(v)
            requestAnimationFrame(updatePop)
          }}
          onSelect={updatePop}
          onFocus={() => setFocused(true)}
          onBlur={() => (setFocused(false), setTimeout(() => setPop(null), 120))}
          onPaste={onPaste}
          onKeyDown={(e) => {
            if (pop && suggestions.length) {
              if (e.key === 'ArrowDown') {
                e.preventDefault()
                return setActive((a) => (a + 1) % suggestions.length)
              }
              if (e.key === 'ArrowUp') {
                e.preventDefault()
                return setActive((a) => (a - 1 + suggestions.length) % suggestions.length)
              }
              if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
                e.preventDefault()
                return pick(active)
              }
              if (e.key === 'Escape') {
                e.preventDefault()
                return setPop(null)
              }
            }
            if (e.key === 'Escape' && working && !text) {
              e.preventDefault()
              return void stop()
            }
            if (e.key === 'Enter' && e.altKey && !e.nativeEvent.isComposing) {
              e.preventDefault()
              return void hold()
            }
            if (e.key === 'Enter' && !e.shiftKey && !touch && !e.nativeEvent.isComposing) {
              e.preventDefault()
              send()
            }
          }}
          placeholder={canSteer && steer ? `Steer ${agentName} now…` : working ? `Queue a message for ${agentName}…` : `Message ${agentName}…`}
          aria-label={`Message ${agentName}`}
          className={clsx('block max-h-[40vh] w-full resize-none bg-transparent px-4 pb-1 text-[15.5px] leading-[1.5] text-fg placeholder:text-fg-3', shellMode ? 'pt-1' : 'pt-3')}
        />
        {/* a container: next to the side panel the box is narrower than the screen, and its labels fold to icons by its own width */}
        <div className="@container flex items-center gap-0.5 px-1.5 pb-1.5">
          <ToolbarButton label="Attach files" onClick={() => fileInput.current?.click()}>
            <Paperclip className="size-[17px]" />
          </ToolbarButton>
          {touch && (
            <ToolbarButton label="Take a photo" onClick={() => cameraInput.current?.click()}>
              <Camera className="size-[17px]" />
            </ToolbarButton>
          )}
          {controls && <div className="no-scrollbar flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">{controls}</div>}
          <span className={clsx('truncate pr-1 text-[12px] text-fg-3', !controls && 'ml-auto')}>
            {uploading ? 'Uploading…' : failed ? 'Upload failed' : text && origin === 'terminal' ? 'From the terminal' : ''}
          </span>
          {canSteer && (
            <button
              type="button"
              role="switch"
              aria-checked={steer}
              aria-label={steer ? 'Steer now: Codex reads the message in the middle of this turn. Switch to queue it for after the turn' : 'Queue: Codex reads the message after this turn. Switch to steer it in now'}
              title={steer ? 'Steer now (Enter in the terminal): Codex reads it in the middle of this turn' : 'Queue (Tab in the terminal): Codex reads it after this turn'}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setSteer((v) => !v)}
              className={clsx(
                'relative inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full px-2.5 font-mono text-[12px] transition-colors after:absolute after:-inset-1 hover:bg-hover hover:text-fg',
                steer ? 'text-accent-text' : 'text-fg-2',
              )}
            >
              {steer ? <Zap className="size-3.5" /> : <Clock3 className="size-3.5" />}
              {steer ? 'Steer now' : 'Queue'}
            </button>
          )}
          {/* one button for Send and Stop: its colour fades between the two and the icon swaps with a fade */}
          <button
            type="button"
            {...contextMenu(() => [
              !showStop && !shellMode && { key: 'send', label: canSteer && steer ? 'Steer now' : working ? 'Queue follow-up' : 'Send', icon: <ArrowUp className="size-4" />, disabled: !canSend, onSelect: () => void send() },
              !shellMode && { key: 'hold', label: `Hold until ${agentName} is free`, keys: keyLabel('alt', 'Enter'), icon: <Hourglass className="size-4" />, disabled: !text.trim() || uploading || files.length > 0, onSelect: () => void hold() },
              // from 0.24 the hub can keep it to send at a set time
              !shellMode && canSchedule(host) && { key: 'later', label: 'Send later…', icon: <CalendarClock className="size-4" />, disabled: !text.trim() || uploading || files.length > 0, onSelect: () => setLater(true) },
              showStop && { key: 'stop', label: 'Stop', icon: <Square className="size-4" />, onSelect: () => void stop() },
            ])}
            onClick={() => (showStop ? stop() : send())}
            onMouseDown={(e) => e.preventDefault()}
            disabled={showStop ? stopping : !canSend}
            className={clsx(
              'press flex size-9 shrink-0 items-center justify-center rounded-full',
              showStop ? 'bg-fg text-bg hover:opacity-85 disabled:opacity-50' : canSend ? 'bg-accent text-accent-fg hover:opacity-90' : 'bg-sunken text-fg-3',
            )}
            aria-label={showStop ? 'Stop' : shellMode ? 'Run command' : canSteer && steer ? 'Steer now' : working ? 'Queue follow-up' : 'Send'}
            title={showStop ? 'Stop (Esc in the terminal)' : shellMode ? 'Run shell command' : canSteer && steer ? 'Steer Codex now' : working ? 'Queue a follow-up' : 'Send'}
          >
            <span key={showStop ? 'stop' : 'send'} className="animate-fade-in flex">
              {showStop ? <Square className="size-3 fill-current" /> : <ArrowUp className="size-[17px]" strokeWidth={2.4} />}
            </span>
          </button>
        </div>
      </div>
      <SendLaterSheet host={host} pane={pane} text={text} open={later} onClose={() => setLater(false)} onSent={handedOver} />
      <input ref={fileInput} type="file" multiple hidden onChange={(e) => (addFiles([...(e.target.files ?? [])]), (e.target.value = ''))} />
      <input ref={cameraInput} type="file" accept="image/*" capture="environment" hidden onChange={(e) => (addFiles([...(e.target.files ?? [])]), (e.target.value = ''))} />
    </div>
  )
}

/** Files on a clipboard or drop, from whichever field the browser filled (screenshots often only in items). */
function clipboardFiles(data: DataTransfer): File[] {
  const files = [...data.files]
  if (!files.length) for (const it of data.items ?? []) if (it.kind === 'file') {
    const f = it.getAsFile()
    if (f) files.push(f)
  }
  return files.map((f, i) => (f.name && f.name !== 'image.png' ? f : new File([f], `pasted-${Date.now()}-${i}.${f.type.split('/')[1] || 'bin'}`, { type: f.type })))
}

function ToolbarButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className="flex size-8 items-center justify-center rounded-full text-fg-3 transition-colors hover:bg-hover hover:text-fg"
    >
      {children}
    </button>
  )
}
