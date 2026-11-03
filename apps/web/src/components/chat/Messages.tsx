import { createContext, memo, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import {
  Clock3,
  AlertTriangle, Check, ChevronRight, CircleAlert, CornerDownLeft, File as FileIcon, Image as ImageIcon, Info, Layers, Lightbulb,
  Code as CodeIcon, Loader2, MonitorX, RadioTower, RotateCcw, SquareTerminal, Target, CircleCheck, CircleX, X,
} from 'lucide-react'
import { agentDef, type AgentKind, type LiveItem } from '@inyon/protocol'
import type { Part, Row } from '../../lib/chat.ts'
import type { PendingMessage } from '../../lib/store.ts'
import { clock, duration } from '../../lib/format.ts'
import { AgentSpinner } from '../ui/status.tsx'
import { AntigravityMark, CodexMark, TileMark } from '../ui/AgentMarks.tsx'
import { CopyButton, Markdown } from './Markdown.tsx'
import { contextMenu, copyItem } from '../ui/ContextMenu.tsx'
import { Fold, useFold } from '../ui/Fold.tsx'
import { SpecialTool, ToolGroup, TurnSettled, useArrived } from './Tools.tsx'
import { ChatAgent, ChatHost } from './agent.ts'
import { linkedPages, usePages } from '../../lib/pages.ts'
import { PageRows } from '../pages/shared.tsx'

export { ChatAgent }
/** Rewinds the conversation to just before a message, when the agent can (Codex). */
export const ChatRewind = createContext<((text: string) => void) | undefined>(undefined)

export const UserMessage = memo(function UserMessage({ row }: { row: Extract<Row, { type: 'user' }> }) {
  const { item } = row
  const rewind = useContext(ChatRewind)
  const [expanded, setExpanded] = useState(false)
  const long = item.text.length > 1200 || item.text.split('\n').length > 18
  if (item.shell) {
    return (
      <div className="flex justify-end">
        {/* commands keep their lines, and a long path wraps instead of running out of the bubble */}
        <div {...contextMenu(() => [copyItem('copy', 'Copy command', item.text, 'Command')])} className="max-w-[88%] min-w-0 whitespace-pre-wrap rounded-3xl bg-bubble px-4 py-2.5 font-mono text-[13px] [overflow-wrap:anywhere]">
          <span className="mr-2 text-accent-text">!</span>
          {item.text}
        </div>
      </div>
    )
  }
  const menu = contextMenu(() => [
    copyItem('copy', 'Copy message', item.text, 'Message'),
    rewind && { key: 'rewind', label: 'Rewind to here…', icon: <RotateCcw className="size-4" />, onSelect: () => rewind(item.text) },
  ])
  return (
    <div className="group flex flex-col items-end">
      <div
        {...menu}
        className={clsx(
          'relative max-w-[88%] whitespace-pre-wrap break-words rounded-3xl bg-bubble px-4 py-2.5 text-[15px] leading-[1.55] sm:max-w-[78%]',
          long && !expanded && 'max-h-80 overflow-hidden',
        )}
      >
        {item.images > 0 && (
          <span className="mb-1.5 mr-1.5 inline-flex items-center gap-1 rounded-full border border-line bg-elev/70 px-2 py-0.5 text-xs text-fg-2">
            <ImageIcon className="size-3.5" />
            {item.images} image{item.images > 1 ? 's' : ''}
          </span>
        )}
        <LinkedPaths text={item.text} />
        {long && !expanded && (
          <button
            type="button"
            onClick={() => setExpanded(true)}
            className="absolute inset-x-0 bottom-0 border-t border-line bg-bubble py-2 text-[13px] font-medium text-fg-2 hover:text-fg"
          >
            Show more
          </button>
        )}
      </div>
      <div className="mt-1 flex h-6 items-center gap-1 px-1 text-[12px] text-fg-3 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 max-sm:opacity-100">
        {item.queued && <span className="mr-1">Sent while the agent was working ·</span>}
        <span>{clock(item.ts)}</span>
        <CopyButton text={item.text} />
        {rewind && (
          <button
            type="button"
            title="Rewind to here"
            aria-label="Rewind to here"
            onClick={() => rewind(item.text)}
            className="grid size-6 place-items-center rounded-md hover:bg-elev hover:text-fg"
          >
            <RotateCcw className="size-3.5" />
          </button>
        )}
      </div>
    </div>
  )
})

const IMAGE = /\.(png|jpe?g|gif|webp|heic|heif|avif|bmp|svg)$/i
/** an upload's name as the person picked it: the hub adds <time>-<id>- in front (features.ts uploadPath) */
export const uploadName = (path: string) => path.split('/').pop()!.replace(/^\d{6}-(?:[0-9a-f]{6}-)?/, '')

/** Uploaded files show as chips instead of long paths. */
function LinkedPaths({ text }: { text: string }) {
  const parts = text.split(/(@?\/[^\s]*\/\.inyon\/uploads\/[^\s]+)/g)
  return (
    <>
      {parts.map((p, i) =>
        /\.inyon\/uploads\//.test(p) ? (
          <span key={i} className="mx-0.5 inline-flex max-w-full items-center gap-1 rounded-md border border-line bg-elev/70 px-1.5 py-px align-baseline font-mono text-[12px] text-fg-2">
            {IMAGE.test(p) ? <ImageIcon className="size-3 shrink-0" /> : <FileIcon className="size-3 shrink-0" />}
            <span className="truncate">{uploadName(p)}</span>
          </span>
        ) : (
          <span key={i}>{p}</span>
        ),
      )}
    </>
  )
}

export function PendingBubble({ p, onRetry, onDismiss, onScreen }: { p: PendingMessage; onRetry: () => void; onDismiss: () => void; onScreen?: () => void }) {
  return (
    <div className="flex flex-col items-end">
      <div
        {...contextMenu(() => [
          copyItem('copy', 'Copy message', p.text, 'Message'),
          p.status === 'failed' && { key: 'retry', label: 'Retry', icon: <RotateCcw className="size-4" />, onSelect: onRetry },
          p.status === 'failed' && onScreen && { key: 'screen', label: 'What was on screen', icon: <MonitorX className="size-4" />, onSelect: onScreen },
          p.status === 'failed' && { key: 'dismiss', label: 'Dismiss', icon: <X className="size-4" />, separated: true, onSelect: onDismiss },
        ])}
        // the same box as the message it becomes, so nothing moves when the agent takes it
        className={clsx('max-w-[88%] whitespace-pre-wrap break-words rounded-3xl bg-bubble px-4 py-2.5 text-[15px] leading-[1.55] transition-opacity sm:max-w-[78%]', p.status !== 'failed' && 'opacity-70')}
      >
        <LinkedPaths text={p.text} />
      </div>
      <div className="mt-1 flex h-6 items-center gap-1.5 px-1 text-[12px] text-fg-3">
        {p.status === 'sending' && (
          <>
            <Loader2 className="size-3 animate-spin" /> {p.remote ? 'Sending from another device' : 'Sending'}
          </>
        )}
        {p.status === 'sent' && (
          <>
            <Check className="size-3" /> {p.remote ? 'Sent from another device' : 'Sent'}
          </>
        )}
        {p.status === 'failed' && (
          <button type="button" onClick={onRetry} className="flex items-center gap-1 text-danger">
            <CircleAlert className="size-3" /> {p.error ?? 'Not sent'} · <RotateCcw className="size-3" /> Retry
          </button>
        )}
        <CopyButton text={p.text} />
        {p.status === 'failed' && (
          <button type="button" onClick={onDismiss} title="Dismiss" aria-label="Dismiss" className="flex size-6 items-center justify-center rounded-md hover:bg-hover hover:text-fg">
            <X className="size-3.5" />
          </button>
        )}
      </div>
    </div>
  )
}

/** A message sitting in the agent's queue: it becomes a normal bubble here once the agent takes it. */
export function QueuedBubble({ text, agent }: { text: string; agent?: AgentKind }) {
  return (
    <div className="flex flex-col items-end">
      <div className="max-w-[88%] whitespace-pre-wrap break-words rounded-3xl border border-dashed border-line-strong px-4 py-2.5 text-[15px] leading-[1.55] text-fg-2 sm:max-w-[78%]">
        <LinkedPaths text={text} />
      </div>
      <div className="mt-1 flex h-6 items-center gap-1.5 px-1 text-[12px] text-fg-3">
        <Clock3 className="size-3" /> Queued · {agent ? agentDef(agent).name : 'the agent'} reads it at its next step
      </div>
    </div>
  )
}

function Thinking({ text }: { text: string }) {
  const [open, mounted, toggle] = useFold()
  const arrived = useArrived()
  return (
    <div className={clsx('my-1 -mx-2', arrived && 'animate-row-in')}>
      <button type="button" onClick={toggle} className="flex items-center gap-2.5 rounded-full px-2 py-1.5 text-[13px] text-fg-3 hover:bg-hover">
        <Lightbulb className="size-[15px]" />
        <span className="font-medium">Thinking</span>
        <ChevronRight className={clsx('size-3.5 transition-transform', open && 'rotate-90')} />
      </button>
      <Fold open={open} mounted={mounted}>
        <div className="ml-[30px] whitespace-pre-wrap border-l border-line-strong pl-3 text-[13.5px] italic leading-relaxed text-fg-2">{text}</div>
      </Fold>
    </div>
  )
}

function PartView({ part, live }: { part: Part; live: boolean }) {
  switch (part.type) {
    case 'text':
      return <Markdown text={part.text} className="my-2" />
    case 'thinking':
      return <Thinking text={part.text} />
    case 'tools':
      return <ToolGroup calls={part.calls} live={live} />
    case 'special':
      return <SpecialTool call={part.call} live={live} />
    case 'notice':
      return part.item.subtype === 'task' ? <JobFinished item={part.item} /> : <div className="my-2 text-[13px] text-fg-3">Interrupted</div>
  }
}

/** A background shell, monitor or subagent finishing, where it happened in the conversation. */
function JobFinished({ item }: { item: Extract<Row, { type: 'notice' }>['item'] }) {
  const failed = item.level !== 'info'
  const Icon = failed ? CircleX : CircleCheck
  const arrived = useArrived()
  return (
    <div className={clsx('my-1 flex items-center gap-2.5 px-0 py-1 text-[12.5px] text-fg-3', arrived && 'animate-row-in')}>
      <Icon className={clsx('size-4 shrink-0', failed ? 'text-danger' : 'text-fg-3')} />
      <span className="min-w-0 truncate">{item.text}</span>
      <span className="ml-auto shrink-0 tabular text-[12px]">{clock(item.ts)}</span>
    </div>
  )
}

export function AgentMark({ active }: { active?: boolean }) {
  const kind = useContext(ChatAgent)
  if (kind === 'antigravity') return <AntigravityMark active={active} className="size-[18px]" />
  if (kind === 'codex') return <CodexMark active={active} className="size-[19px]" />
  // a still frame of a text spinner reads as stray dots; these agents show their tile instead
  if (kind === 'gemini' || kind === 'copilot' || kind === 'opencode') return <TileMark kind={kind} active={active} className="size-[19px] rounded-[5px]" />
  return <AgentSpinner kind={kind} active={active} className="text-[19px]" />
}

/**
 * The pages a finished reply links to (what `inyon pages add` prints), as cards that open them.
 * Only pages the list has, so a link to one that's gone or on a hub this app doesn't reach shows
 * nothing.
 */
function PageCards({ text }: { text: string }) {
  const host = useContext(ChatHost)
  const all = usePages((s) => s.pages)
  const ids = useMemo(() => (host ? linkedPages(text, host) : []), [text, host])
  const pages = useMemo(() => ids.flatMap((id) => all?.find((p) => p.id === id) ?? []).slice(0, 3), [ids, all])
  if (!pages.length) return null
  return <PageRows pages={pages} className="my-3" />
}

export const AssistantTurn = memo(function AssistantTurn({ row, live }: { row: Extract<Row, { type: 'assistant' }>; live: boolean }) {
  const text = row.parts.filter((p) => p.type === 'text').map((p) => (p as { text: string }).text).join('\n\n')
  const menu = contextMenu((e) => {
    const code = (e.target as Element).closest?.('pre')?.textContent
    return [code && copyItem('copy-code', 'Copy code', code, 'Code', <CodeIcon className="size-4" />), !row.open && !!text && copyItem('copy', 'Copy reply', text, 'Reply')]
  })
  // once this reply has been painted, the steps that join it afterwards come in rising
  const settled = useRef(false)
  useEffect(() => {
    settled.current = true
  }, [])
  const isSettled = useCallback(() => settled.current, [])
  return (
    <div className="group flex gap-3">
      <div className="mt-[2px] hidden size-6 shrink-0 items-center justify-center sm:flex">
        <AgentMark />
      </div>
      <div {...menu} className="min-w-0 flex-1">
        <TurnSettled.Provider value={isSettled}>
          {row.parts.map((p) => (
            <PartView key={p.key} part={p} live={live && row.open} />
          ))}
        </TurnSettled.Provider>
        {!row.open && text.includes('/pages/') && <PageCards text={text} />}
        {!row.open && (
          <div className="mt-1 flex h-7 items-center gap-1 text-[12px] text-fg-3 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 max-sm:opacity-100">
            {text && <CopyButton text={text} className="-ml-1.5" />}
            <span>{clock(row.ts)}</span>
            {row.durationMs ? <span>· worked {duration(row.durationMs)}</span> : null}
          </div>
        )}
      </div>
    </div>
  )
})

export function CommandRow({ row }: { row: Extract<Row, { type: 'command' }> }) {
  const [open, mounted, toggle] = useFold()
  const shell = row.name === '!'
  const out = row.output?.replace(/\x1b\[[0-9;]*m/g, '').trim()
  return (
    <div className="flex flex-col items-center">
      <button
        type="button"
        onClick={() => out && toggle()}
        className={clsx('inline-flex max-w-full items-center gap-2 rounded-full border border-line bg-elev px-3 py-1 text-[12.5px] text-fg-2', out && 'hover:bg-hover')}
      >
        {shell && <SquareTerminal className="size-3.5 text-fg-3" />}
        <span className="truncate font-mono text-[12px]">
          {shell ? 'shell output' : row.name ? `/${row.name.replace(/^\/+/, '')}${row.args ? ` ${row.args}` : ''}` : 'command output'}
        </span>
        {out && <ChevronRight className={clsx('size-3.5 text-fg-3 transition-transform', open && 'rotate-90')} />}
      </button>
      {out && (
        <Fold open={open} mounted={mounted} className="w-full">
          <pre className="mt-2 max-h-72 w-full overflow-auto whitespace-pre-wrap rounded-2xl border border-line bg-code px-4 py-3 font-mono text-[12px] leading-relaxed text-fg-2">{out}</pre>
        </Fold>
      )}
    </div>
  )
}

export function GoalRow({ row }: { row: Extract<Row, { type: 'goal' }> }) {
  const { item } = row
  return (
    <div className={clsx('flex items-start gap-2.5 rounded-2xl border px-4 py-3 text-[13.5px]', item.met ? 'border-line bg-sunken' : 'border-dashed border-line-strong')}>
      <Target className={clsx('mt-0.5 size-4 shrink-0', item.met ? 'text-fg' : 'text-fg-3')} />
      <div className="min-w-0">
        <div className="font-medium">{item.met ? 'Goal met' : item.cleared ? 'Goal cleared' : 'Goal set'}</div>
        <div className="mt-0.5 line-clamp-3 text-fg-2">{item.condition}</div>
        {item.met && item.reason && <div className="mt-1 line-clamp-3 text-[12.5px] text-fg-3">{item.reason}</div>}
      </div>
    </div>
  )
}

export function NoticeRow({ row }: { row: Extract<Row, { type: 'notice' }> }) {
  const { item } = row
  if (item.subtype === 'task') return <JobFinished item={item} />
  if (item.subtype === 'compact') {
    return (
      <div className="flex items-center gap-3 py-1 text-[12px] text-fg-3">
        <div className="h-px flex-1 bg-line" />
        <Layers className="size-3.5" />
        {item.text}
        <div className="h-px flex-1 bg-line" />
      </div>
    )
  }
  if (item.subtype === 'rc') {
    return (
      <div className="flex items-center justify-center gap-2 py-0.5 text-[12px] text-fg-3">
        <RadioTower className="size-3.5" />
        Remote Control turned on
      </div>
    )
  }
  if (item.subtype === 'recap') {
    return (
      <div className="rounded-2xl border border-line bg-sunken px-4 py-3 text-[13.5px] leading-relaxed text-fg-2">
        <div className="mb-1 flex items-center gap-1.5 text-[12.5px] font-medium text-fg-3">
          <CornerDownLeft className="size-3.5" /> While you were away
        </div>
        {item.text}
      </div>
    )
  }
  const Icon = item.level === 'info' ? Info : AlertTriangle
  return (
    <div
      className={clsx(
        'flex items-start gap-2 rounded-xl px-3.5 py-2.5 text-[13.5px]',
        item.level === 'error' ? 'bg-danger-soft text-danger' : item.level === 'warn' ? 'bg-warn-soft text-warn' : 'bg-sunken text-fg-2',
      )}
    >
      <Icon className="mt-0.5 size-4 shrink-0" />
      <span className="min-w-0 break-words">{item.text}</span>
    </div>
  )
}

/** A tick every second while `on`, so an elapsed time keeps counting. */
function useTicking(on: boolean) {
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!on) return
    const t = setInterval(() => setTick((n) => n + 1), 1000)
    return () => clearInterval(t)
  }, [on])
}

/**
 * `since`: when the turn started (unix ms), counted up every second. `output`: the last of what a
 * running command has printed, shown as the line's detail (Codex).
 */
export function WorkingIndicator({ activity, since, output }: { activity?: string; since?: number; output?: string }) {
  const [verb, rest] = (activity ?? 'Working…').split(/ \((.*)\)$/)
  useTicking(!!since && !rest)
  const elapsed = since ? Math.max(0, Date.now() - since) : 0
  const tail = output?.trimEnd().split('\n').pop()?.trim()
  return (
    <div className="animate-fade-in flex items-center gap-3 py-1">
      <div className="flex size-6 shrink-0 items-center justify-center">
        <AgentMark active />
      </div>
      <div className="min-w-0 truncate text-[13.5px]">
        <span className="font-medium text-accent-text">{verb}</span>
        {rest ? (
          <span className="text-fg-3"> {rest.replace(/\s*·\s*esc to interrupt/i, '')}</span>
        ) : since && elapsed >= 1000 ? (
          <span className="text-fg-3 tabular-nums"> {duration(elapsed)}</span>
        ) : null}
        {tail && <span className="ml-2 font-mono text-[12px] text-fg-3">{tail}</span>}
      </div>
    </div>
  )
}

/**
 * The reply as the agent writes it, above the working line: Codex's from its app-server, with its
 * thinking folded and a plan, and Claude Code's, Copilot's and Antigravity's as read off the screen,
 * as Markdown. It fades in once and then only grows; the finished message takes its place with the
 * same text, so nothing moves when it lands.
 */
export function LiveTurn({ items }: { items: LiveItem[] }) {
  const shown = items.filter((i) => i.kind !== 'output' && i.text.trim())
  if (!shown.length) return null
  return (
    <div className="animate-fade-in flex gap-3" data-live>
      <div className="mt-[2px] hidden size-6 shrink-0 items-center justify-center sm:flex">
        <AgentMark />
      </div>
      <div className="min-w-0 flex-1">
        {shown.map((i) => (i.kind === 'thinking' ? <Thinking key={i.id} text={i.text} /> : <Markdown key={i.id} text={i.text} className="my-2" />))}
      </div>
    </div>
  )
}
