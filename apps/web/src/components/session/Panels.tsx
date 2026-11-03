import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import {
  Bot, ExternalLink, FileDiff, Globe, LayoutPanelLeft, Link2, Monitor as MonitorIcon, PanelsTopLeft, RefreshCw, Smartphone,
  SquareTerminal, Timer, Workflow, Layers as Stack, Copy, Trash2, ShieldAlert,
} from 'lucide-react'
import type { BackgroundTask, ChatItem, SubagentInfo } from '@inyon/protocol'
import { buildRows } from '../../lib/chat.ts'
import { api } from '../../lib/api.ts'
import { openLink, shortPath, since } from '../../lib/format.ts'
import { Diff, useHunks } from '../chat/Diff.tsx'
import { AssistantTurn, CommandRow, GoalRow, NoticeRow, UserMessage } from '../chat/Messages.tsx'
import { Button, Empty, Segmented, Sheet, Waiting, inputClass } from '../ui/primitives.tsx'
import { CommentableDiff, FilesTab, ReviewBar, WorkingTree, type PaneKey } from './Workspace.tsx'
import { UsagePanel } from './Usage.tsx'
import type { SessionModel } from './useSession.ts'
import { toast } from '../../lib/store.ts'
import { copyText } from '../../lib/clipboard.ts'
import { confirmAction } from '../Confirm.tsx'
import { appOrigins, previewRefusal, sessionPages, usePages, type Page } from '../../lib/pages.ts'
import { PageRows } from '../pages/shared.tsx'
import { useFramesIsolated } from '../../lib/native.ts'
import { FrameRefused } from '../FrameRefused.tsx'

// ---------- changes ----------

interface FileChange {
  path: string
  edits: { name: string; input: any; meta?: any; ts: string }[]
  add: number
  del: number
  created: boolean
}

/** Every file the agent edited in this conversation, newest first. */
export function useChanges(items: ChatItem[]) {
  return useMemo(() => {
    const results = new Map<string, Extract<ChatItem, { kind: 'tool_result' }>>()
    for (const i of items) if (i.kind === 'tool_result') results.set(i.toolUseId, i)
    const files = new Map<string, FileChange>()
    for (const i of items) {
      if (i.kind !== 'assistant' || i.block.type !== 'tool_use') continue
      const b = i.block
      if (!['Edit', 'MultiEdit', 'Write', 'NotebookEdit', 'ApplyPatch'].includes(b.name)) continue
      const r = results.get(b.id)
      if (r?.isError) continue
      const paths: string[] = b.name === 'ApplyPatch'
        ? (String(b.input?.patch ?? '').match(/^\*\*\* (?:Update|Add|Delete) File: (.+)$/gm) ?? []).map((l) => l.replace(/^\*\*\* \w+ File: /, ''))
        : [b.input?.file_path ?? b.input?.notebook_path].filter(Boolean)
      for (const p of paths) {
        const f = files.get(p) ?? { path: p, edits: [], add: 0, del: 0, created: false }
        f.edits.push({ name: b.name, input: b.input, meta: r?.meta, ts: i.ts })
        if (r?.meta?.patch?.length) {
          for (const h of r.meta.patch) for (const l of h.lines) l.startsWith('+') ? f.add++ : l.startsWith('-') && f.del++
        } else if (b.name === 'Write' && typeof b.input?.content === 'string') {
          f.add += b.input.content.split('\n').length
        } else if (b.name === 'Edit' && typeof b.input?.new_string === 'string') {
          f.add += b.input.new_string.split('\n').length
          f.del += String(b.input.old_string ?? '').split('\n').length
        }
        if (r?.meta?.created) f.created = true
        files.delete(p)
        files.set(p, f)
      }
    }
    return [...files.values()].reverse()
  }, [items])
}

function EditDiff({ e, file, p }: { e: FileChange['edits'][number]; file: string; p?: PaneKey }) {
  const hunks = useHunks(e.meta, e.input)
  if (e.name === 'ApplyPatch' && !hunks.length) {
    return <pre className="max-h-[50vh] overflow-auto whitespace-pre px-3 py-2 font-mono text-[12px] leading-[1.6] text-fg-2">{String(e.input?.patch ?? '')}</pre>
  }
  if (!hunks.length) return <div className="px-3 py-2 text-[12px] text-fg-3">No diff recorded</div>
  return p ? <CommentableDiff p={p} file={file} hunks={hunks} /> : <Diff hunks={hunks} />
}

type ChangesMode = 'chat' | 'tree'

/**
 * What changed: the edits the agent made in this conversation, or (with a session to ask) the git
 * working tree, which also shows changes made by shell commands or by hand. Line numbers in either
 * take comments, which go into the message box together.
 */
export function ChangesTab({ items, p, onComposed }: { items: ChatItem[]; p?: PaneKey; onComposed?: () => void }) {
  const [mode, setMode] = useState<ChangesMode>(() => {
    try {
      return localStorage.getItem('inyon.changes') === 'tree' ? 'tree' : 'chat'
    } catch {
      return 'chat'
    }
  })
  const pick = (m: ChangesMode) => {
    setMode(m)
    try {
      localStorage.setItem('inyon.changes', m)
    } catch {}
  }
  if (!p) return <ChatChanges items={items} />
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 px-2.5 pt-2.5 pb-1.5">
        <Segmented full size="sm" value={mode} onChange={pick} options={[{ v: 'chat', label: 'This chat' }, { v: 'tree', label: 'Working tree', title: 'Every change since the last commit' }]} />
      </div>
      <div key={mode} className="animate-fade-in flex min-h-0 flex-1 flex-col">
        {mode === 'chat' ? <ChatChanges items={items} p={p} onTree={() => pick('tree')} /> : <WorkingTree p={p} poke={items.length} />}
      </div>
      <ReviewBar p={p} onComposed={onComposed} />
    </div>
  )
}

function ChatChanges({ items, p, onTree }: { items: ChatItem[]; p?: PaneKey; onTree?: () => void }) {
  const changes = useChanges(items)
  const [sel, setSel] = useState<string | null>(null)
  const cur = changes.find((c) => c.path === sel) ?? changes[0]
  if (!changes.length) {
    // edits made with shell commands (cat >>, sed -i, a script) aren't edit tools: git sees them
    return (
      <Empty icon={<FileDiff className="size-5" />} title="No changes yet" action={onTree && <Button size="sm" onClick={onTree}>Show working tree</Button>}>
        Files the agent edits in this conversation show up here with their diffs.{onTree && ' Edits it makes through commands show under Working tree.'}
      </Empty>
    )
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="max-h-[40%] shrink-0 overflow-y-auto border-b border-line p-1.5">
        {changes.map((c) => (
          <button
            key={c.path}
            type="button"
            onClick={() => setSel(c.path)}
            className={clsx('flex w-full items-center gap-2.5 rounded-xl px-2.5 py-1.5 text-left text-[12.5px]', cur?.path === c.path ? 'bg-hover' : 'hover:bg-hover/60')}
          >
            <span className={clsx('w-3 font-mono text-[12px] font-semibold', c.created ? 'text-ok' : 'text-accent-text')}>{c.created ? 'A' : 'M'}</span>
            <span className="min-w-0 flex-1 truncate">
              <span className="text-fg-3">{shortPath(c.path.slice(0, c.path.lastIndexOf('/') + 1), 30)}</span>
              {c.path.split('/').pop()}
            </span>
            <span className="tabular shrink-0 font-mono text-[12px]">
              <span className="text-ok">+{c.add}</span> <span className="text-danger">−{c.del}</span>
            </span>
          </button>
        ))}
      </div>
      {cur && (
        <div className="min-h-0 flex-1 overflow-y-auto">
          {cur.edits
            .slice()
            .reverse()
            .map((e, n) => (
              <div key={n} className="border-b border-line">
                <div className="sticky top-0 z-[1] flex items-center gap-2 border-b border-line bg-bg px-3 py-1.5 text-[12px] text-fg-3">
                  <span className="font-medium text-fg-2">{e.name === 'Write' ? (e.meta?.created ? 'Created' : 'Rewrote') : 'Edited'}</span>
                  <span>{since(e.ts)} ago</span>
                </div>
                <EditDiff e={e} file={cur.path} p={p} />
              </div>
            ))}
        </div>
      )}
    </div>
  )
}

// ---------- activity ----------

function StopButton({ host, task }: { host: string; task: BackgroundTask }) {
  const [busy, setBusy] = useState(false)
  return (
    <button
      type="button"
      disabled={busy}
      onClick={async () => {
        setBusy(true)
        try {
          const r = await api.stopTask(host, { id: task.id, path: task.outputFile })
          toast({ title: r.stopped ? 'Task stopped' : 'Already finished', tone: r.stopped ? 'success' : 'info' })
        } catch (e) {
          toast({ title: 'Could not stop', body: (e as Error).message, tone: 'error' })
        } finally {
          setBusy(false)
        }
      }}
      className="shrink-0 rounded-full border border-line-strong px-2 py-1 text-[12px] text-danger hover:bg-danger-soft disabled:opacity-50"
    >
      {busy ? 'Stopping…' : 'Stop'}
    </button>
  )
}

const taskIcon = (t: BackgroundTask) => (t.type === 'monitor' ? Timer : t.type === 'workflow' ? Workflow : SquareTerminal)

export function ActivityTab({ s, onTask, onSubagent }: { s: SessionModel; onTask: (t: BackgroundTask) => void; onSubagent: (a: SubagentInfo) => void }) {
  const info = s.info
  if (!info) return <Waiting shape="rows" label="Reading the session" />
  const tasks = [...info.tasks].reverse()
  const shells = tasks.filter((t) => t.type !== 'monitor')
  const monitors = tasks.filter((t) => t.type === 'monitor')
  const subs = [...info.subagents].reverse()
  const arts = [...info.artifacts].reverse()
  if (!tasks.length && !subs.length && !arts.length && !info.queue.length) {
    return <Empty icon={<Stack className="size-5" />} title="Nothing running">Background shells, monitors, subagents and artifacts from this session appear here.</Empty>
  }
  const Group = ({ title, children }: { title: string; children: React.ReactNode }) => (
    <section className="space-y-1.5">
      <h3 className="px-0.5 text-[13px] font-medium text-fg-2">{title}</h3>
      {children}
    </section>
  )
  const Row = ({ icon, title, sub, live, onClick, right }: { icon: React.ReactNode; title: string; sub: string; live?: boolean; onClick?: () => void; right?: React.ReactNode }) => (
    <div className="flex items-center gap-3 rounded-2xl border border-line bg-elev px-3 py-2.5">
      <span className={clsx('flex size-7 shrink-0 items-center justify-center rounded-xl bg-sunken', live ? 'text-accent' : 'text-fg-3')}>{icon}</span>
      <button type="button" onClick={onClick} disabled={!onClick} className="min-w-0 flex-1 text-left">
        <div className="truncate text-[13px] font-medium">{title}</div>
        <div className="truncate text-[12px] text-fg-3">
          {live && <span className="pulse-soft mr-1 inline-block size-1.5 rounded-full bg-accent align-middle" />}
          {sub}
        </div>
      </button>
      {right}
    </div>
  )
  return (
    <div className="space-y-5 p-3">
      {info.queue.length > 0 && (
        <Group title="Queued messages">
          {info.queue.map((q, i) => (
            <div key={i} className="truncate rounded-xl border border-dashed border-line-strong px-3 py-2 text-[13px] text-fg-2">{q}</div>
          ))}
        </Group>
      )}
      {subs.length > 0 && (
        <Group title="Subagents">
          {subs.map((a) => (
            <Row
              key={a.toolUseId}
              icon={<Bot className="size-4" />}
              title={a.description}
              sub={`${a.type ?? 'agent'} · ${a.status === 'running' ? `running ${since(a.startedAt)}` : a.status === 'error' ? 'failed' : `done ${since(a.startedAt)} ago`}${a.background ? ' · background' : ''}`}
              live={a.status === 'running'}
              onClick={() => onSubagent(a)}
            />
          ))}
        </Group>
      )}
      {shells.length > 0 && (
        <Group title="Background tasks">
          {shells.map((t) => {
            const Icon = taskIcon(t)
            return (
              <Row
                key={t.id}
                icon={<Icon className="size-4" />}
                title={t.description}
                sub={t.status === 'running' ? `Running · ${since(t.startedAt)}` : `${t.status[0].toUpperCase()}${t.status.slice(1)}${t.endedAt ? ` · ${since(t.endedAt)} ago` : ''}`}
                live={t.status === 'running'}
                onClick={() => onTask(t)}
                right={t.status === 'running' ? <StopButton host={s.ref?.host.config.id ?? ''} task={t} /> : undefined}
              />
            )
          })}
        </Group>
      )}
      {monitors.length > 0 && (
        <Group title="Monitors">
          {monitors.map((t) => (
            <Row key={t.id} icon={<Timer className="size-4" />} title={t.description} sub={`${t.status === 'running' ? 'Watching' : t.status}${t.startedAt ? ` · ${since(t.startedAt)}` : ''}`} live={t.status === 'running'} onClick={() => onTask(t)} right={t.status === 'running' ? <StopButton host={s.ref?.host.config.id ?? ''} task={t} /> : undefined} />
          ))}
        </Group>
      )}
      {arts.length > 0 && (
        <Group title="Artifacts">
          {arts.map((a) => (
            <Row
              key={a.url}
              icon={<LayoutPanelLeft className="size-4" />}
              title={a.title ?? 'Artifact'}
              sub={`updated ${since(a.updatedAt)} ago`}
              onClick={() => openLink(a.url)}
              right={
                <div className="flex shrink-0 gap-1">
                  <button type="button" title="Copy link" onClick={() => copyText(a.url).then((ok) => ok && toast({ title: 'Link copied', tone: 'success' }))} className="flex size-7 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg">
                    <Copy className="size-3.5" />
                  </button>
                  <button type="button" title="Ask the agent to delete it" onClick={async () => (await confirmAction({ title: 'Delete this artifact?', body: `The agent is asked to delete “${a.title ?? a.url}”.`, yes: 'Ask to delete', danger: true })) && s.actions.send(`Delete the artifact ${a.url}`)} className="flex size-7 items-center justify-center rounded-full text-fg-3 hover:bg-danger-soft hover:text-danger">
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
              }
            />
          ))}
          <p className="px-0.5 text-[12px] leading-relaxed text-fg-3">Artifacts open on claude.ai in your browser, where you're signed in.</p>
        </Group>
      )}
    </div>
  )
}

// ---------- preview ----------

export function PreviewTab({ storageKey, suggestions }: { storageKey: string; suggestions: string[] }) {
  const [url, setUrl] = useState(() => {
    try {
      return localStorage.getItem(`inyon.preview.${storageKey}`) ?? ''
    } catch {
      return ''
    }
  })
  const [draft, setDraft] = useState(url)
  const [phone, setPhone] = useState(false)
  // this frame runs with same-origin rights, so it never shows Inyon's own address
  const refused = url ? previewRefusal(url, appOrigins()) : null
  const isolated = useFramesIsolated()
  const [n, setN] = useState(0)
  const go = (u: string) => {
    const v = /^https?:\/\//.test(u) ? u : u ? `http://${u}` : ''
    setUrl(v)
    setDraft(v)
    try {
      localStorage.setItem(`inyon.preview.${storageKey}`, v)
    } catch {}
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <form className="flex items-center gap-1.5 p-2.5" onSubmit={(e) => (e.preventDefault(), go(draft))}>
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="http://my-server:3000" className={clsx(inputClass, 'h-8 font-mono text-[12px]')} aria-label="Preview address" />
        <button type="button" title="Reload" aria-label="Reload" onClick={() => setN((x) => x + 1)} className="flex size-8 shrink-0 items-center justify-center rounded-full border border-line text-fg-2 hover:bg-hover">
          <RefreshCw className="size-3.5" />
        </button>
        <button type="button" title={phone ? 'Desktop width' : 'Phone width'} aria-label={phone ? 'Desktop width' : 'Phone width'} onClick={() => setPhone(!phone)} className="flex size-8 shrink-0 items-center justify-center rounded-full border border-line text-fg-2 hover:bg-hover">
          {phone ? <MonitorIcon className="size-3.5" /> : <Smartphone className="size-3.5" />}
        </button>
        {url && !refused && (
          <a href={url} target="_blank" rel="noreferrer" title="Open in a new tab" aria-label="Open in a new tab" className="flex size-8 shrink-0 items-center justify-center rounded-full border border-line text-fg-2 hover:bg-hover">
            <ExternalLink className="size-3.5" />
          </a>
        )}
      </form>
      {refused ? (
        <div className="p-3">
          <Empty icon={<ShieldAlert className="size-5" />} title="Not shown here">{refused}</Empty>
        </div>
      ) : url ? (
        <div className="flex min-h-0 flex-1 justify-center bg-sunken p-2.5 pt-0">
          {isolated ? (
            <iframe key={n} src={url} title="Preview" sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads" className={clsx('h-full rounded-xl border border-line bg-white', phone ? 'w-[390px] max-w-full' : 'w-full')} />
          ) : (
            <FrameRefused src={url} className="h-full w-full rounded-xl border border-line" />
          )}
        </div>
      ) : (
        <div className="p-3">
          <Empty icon={<Globe className="size-5" />} title="Preview a dev server">Enter an address your browser can reach, like a dev server running on one of your machines.</Empty>
          {suggestions.length > 0 && (
            <div className="flex flex-wrap justify-center gap-1.5">
              {suggestions.map((u) => (
                <button key={u} type="button" onClick={() => go(u)} className="rounded-full border border-line px-2.5 py-1 font-mono text-[12px] text-fg-2 hover:bg-hover">
                  <Link2 className="mr-1 inline size-3" />
                  {u.replace(/^https?:\/\//, '')}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ---------- side panel ----------

export type PanelTab = 'changes' | 'files' | 'activity' | 'preview' | 'pages' | 'usage'

// ---------- pages ----------

/** The pages this session works on (see sessionPages): the ones it added, its project's, and those in folders under it. */
export function useSessionPages(s: SessionModel): Page[] {
  const all = usePages((x) => x.pages)
  const host = s.ref?.host.config.id
  const cwd = s.ref?.pane.cwd
  const session = s.ref?.pane.agent?.sessionId
  return useMemo(() => (host ? sessionPages(all ?? [], host, cwd, session) : []), [all, host, cwd, session])
}

export function PagesTab({ pages }: { pages: Page[] }) {
  if (!pages.length) {
    return (
      <Empty icon={<PanelsTopLeft className="size-5" />} title="No pages here">
        Sites an agent adds with <code className="font-mono text-[12.5px]">inyon pages add</code> in this project show up here.
      </Empty>
    )
  }
  return (
    <div className="p-3">
      <PageRows pages={pages} />
    </div>
  )
}

/**
 * One underline for a tab strip, moved under the chosen tab by transform alone (a 100px strip
 * placed and scaled, as the Segmented pill is). Measured by the component that owns the strip,
 * so the ref is in place by the time it measures.
 */
function useTabUnderline(strip: React.RefObject<HTMLElement | null>, index: number) {
  const [at, setAt] = useState<{ left: number; width: number } | null>(null)
  useLayoutEffect(() => {
    const measure = () => {
      const el = strip.current?.querySelectorAll<HTMLElement>('[role=tab]')[index]
      setAt(el ? { left: el.offsetLeft, width: el.offsetWidth } : null)
    }
    measure()
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : undefined
    if (strip.current) ro?.observe(strip.current)
    return () => ro?.disconnect()
  }, [index])
  // it slides from the second placing on; the first draws it in place
  const moved = useRef(false)
  useEffect(() => {
    if (at) moved.current = true
  }, [at?.left])
  if (!at) return null
  return (
    <span
      aria-hidden
      data-part="underline"
      className={clsx('pointer-events-none absolute -bottom-px left-0 h-0.5 w-[100px] origin-left bg-accent', moved.current && 'transition-transform duration-(--dur-move) ease-out-soft')}
      style={{ transform: `translateX(${at.left}px) scaleX(${at.width / 100})` }}
    />
  )
}

export function SidePanel({ s, items, tab, setTab, onTask, onSubagent, closing }: { s: SessionModel; items: ChatItem[]; tab: PanelTab; setTab: (t: PanelTab) => void; onTask: (t: BackgroundTask) => void; onSubagent: (a: SubagentInfo) => void; closing?: boolean }) {
  const changes = useChanges(items)
  const pages = useSessionPages(s)
  const strip = useRef<HTMLDivElement>(null)
  const p = s.ref ? { host: s.ref.host.config.id, pane: s.ref.pane.id } : undefined
  // a background subagent is also listed as a task of the kind the transcript can't name: count it once
  const running = (s.info?.tasks.filter((t) => t.status === 'running' && t.type !== 'other').length ?? 0) + (s.info?.subagents.filter((a) => a.status === 'running').length ?? 0)
  const tabs: { id: PanelTab; label: string; badge?: number }[] = [
    { id: 'changes', label: 'Changes', badge: changes.length || undefined },
    { id: 'files', label: 'Files' },
    { id: 'activity', label: 'Activity', badge: running || undefined },
    { id: 'preview', label: 'Preview' },
    // only for a session that has pages, or while it's the tab on screen
    ...(pages.length || tab === 'pages' ? [{ id: 'pages' as const, label: 'Pages', badge: pages.length || undefined }] : []),
    { id: 'usage', label: 'Usage' },
  ]
  const underline = useTabUnderline(strip, tabs.findIndex((t) => t.id === tab))
  const previewSuggestions = useMemo(() => {
    const out = new Set<string>()
    for (const i of items.slice(-400)) {
      const t = i.kind === 'tool_result' ? i.text : i.kind === 'assistant' && i.block.type === 'text' ? i.block.text : ''
      for (const m of t.matchAll(/https?:\/\/(?:localhost|127\.0\.0\.1|[\w-]+):(\d{4,5})\b/g)) {
        const port = Number(m[1])
        if ((port >= 3000 && port <= 5999) || (port >= 8000 && port <= 8009)) out.add(`http://${location.hostname}:${port}`)
      }
    }
    return [...out].slice(-6)
  }, [items])
  return (
    // in from its edge; out with a fade while PanePage keeps it mounted
    <aside className={clsx('flex min-h-0 w-[420px] shrink-0 flex-col border-l border-line bg-sidebar 2xl:w-[480px]', closing ? 'animate-fade-out' : 'animate-panel-in')}>
      <div ref={strip} role="tablist" className="relative flex shrink-0 gap-0.5 border-b border-line px-2.5 pt-2">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={clsx('flex h-9 items-center gap-1.5 px-2.5 text-[12.5px] font-medium transition-colors', tab === t.id ? 'text-fg' : 'text-fg-3 hover:text-fg-2')}
          >
            {t.label}
            {t.badge !== undefined && <span className="tabular rounded-full bg-sunken px-1.5 text-[12px] text-fg-2">{t.badge}</span>}
          </button>
        ))}
        {underline}
      </div>
      {/* each tab's body fades in as it takes over */}
      <div key={tab} className="animate-fade-in flex min-h-0 flex-1 flex-col overflow-y-auto">
        {tab === 'changes' && <ChangesTab items={items} p={p} />}
        {tab === 'files' && p && <FilesTab key={`${p.host}:${p.pane}`} p={p} />}
        {tab === 'activity' && <ActivityTab s={s} onTask={onTask} onSubagent={onSubagent} />}
        {tab === 'preview' && <PreviewTab storageKey={`${s.ref?.host.config.id}:${s.ref?.pane.id}`} suggestions={previewSuggestions} />}
        {tab === 'pages' && <PagesTab pages={pages} />}
        {tab === 'usage' && (
          <div className="p-3">
            <UsagePanel status={s.status} info={s.info} host={s.ref?.host} agent={s.agent} agentInfo={s.ref?.pane.agent} pane={s.ref?.pane.id} onCompact={s.def?.set.compact ? s.actions.compact : undefined} />
          </div>
        )}
      </div>
    </aside>
  )
}

// ---------- viewers ----------

/** how often a running task's output is fetched while the sheet is open (the label says so) */
const TASK_REFRESH_MS = 3000

export function TaskOutputSheet({ host, task, onClose }: { host: string; task: BackgroundTask | null; onClose: () => void }) {
  const [out, setOut] = useState<string | null>(null)
  const pre = useRef<HTMLPreElement>(null)
  useEffect(() => {
    if (!task) return
    let alive = true
    const load = () =>
      api
        .taskOutput(host, { id: task.id, path: task.outputFile })
        .then((r) => {
          if (!alive) return
          setOut(r.output)
          requestAnimationFrame(() => pre.current && (pre.current.scrollTop = pre.current.scrollHeight))
        })
        .catch((e) => alive && setOut(`Could not read output: ${e.message}`))
    load()
    // only while it's on screen; a hidden app doesn't need the output every few seconds
    const t = task.status === 'running' ? setInterval(() => document.hidden || load(), TASK_REFRESH_MS) : undefined
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [host, task?.id, task?.status])
  return (
    <Sheet open={!!task} onClose={onClose} wide title={task?.description ?? ''} icon={<SquareTerminal className="size-4 text-fg-3" />}>
      {task?.command && <pre className="mb-3 overflow-x-auto rounded-xl border border-line bg-code px-3 py-2 font-mono text-[12px]"><span className="text-accent-text">$ </span>{task.command}</pre>}
      <pre ref={pre} className="max-h-[60dvh] overflow-auto whitespace-pre-wrap rounded-xl border border-line bg-code px-3 py-2.5 font-mono text-[12px] leading-relaxed text-fg-2">
        {out ?? 'Loading…'}
      </pre>
      <div className="mt-2 text-[12px] text-fg-3">{task?.status === 'running' ? `Live · refreshes every ${TASK_REFRESH_MS / 1000} seconds` : task?.summary ?? task?.status}</div>
    </Sheet>
  )
}

export function SubagentSheet({ s, sub, onClose }: { s: SessionModel; sub: SubagentInfo | null; onClose: () => void }) {
  const [items, setItems] = useState<ChatItem[] | null>(null)
  const [err, setErr] = useState('')
  const host = s.ref?.host.config.id ?? s.info?.hostId ?? ''
  useEffect(() => {
    if (!sub || !s.info?.transcriptPath) return
    let alive = true
    setItems(null)
    setErr('')
    const load = async () => {
      try {
        const list = await api.subagents(host, s.info!.transcriptPath!)
        const hit = list.find((x) => x.toolUseId === sub.toolUseId) ?? list.find((x) => x.description === sub.description)
        if (!hit) throw new Error('This subagent did not save a transcript')
        const its = await api.transcript(host, hit.path)
        if (alive) setItems(its)
      } catch (e) {
        if (alive) setErr((e as Error).message)
      }
    }
    load()
    const t = sub.status === 'running' ? setInterval(() => document.hidden || load(), 5000) : undefined
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [sub?.toolUseId, sub?.status])
  const rows = useMemo(() => buildRows(items ?? []), [items])
  return (
    <Sheet open={!!sub} onClose={onClose} wide title={sub?.description ?? ''} icon={<Bot className="size-4 text-fg-3" />}>
      <div className="mb-3 text-[12px] text-fg-3">
        {sub?.type ?? 'Subagent'} · {sub?.status === 'running' ? 'running · live' : sub?.status}
      </div>
      {err ? (
        <div className="rounded-xl bg-sunken px-3 py-2.5 text-[13px] text-fg-2">{err}</div>
      ) : !items ? (
        <Waiting shape="lines" label="Loading the subagent's conversation" />
      ) : (
        <div className="space-y-4">
          {rows.map((r) =>
            r.type === 'user' ? <UserMessage key={r.key} row={r} /> : r.type === 'assistant' ? <AssistantTurn key={r.key} row={r} live={sub?.status === 'running'} /> : r.type === 'command' ? <CommandRow key={r.key} row={r} /> : r.type === 'goal' ? <GoalRow key={r.key} row={r} /> : <NoticeRow key={r.key} row={r} />,
          )}
        </div>
      )}
    </Sheet>
  )
}
