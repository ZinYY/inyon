import { createContext, useContext, useState } from 'react'
import { ChatActionsContext } from '../../lib/chatActions.ts'
import clsx from 'clsx'
import { AlertCircle, Check, ChevronRight, CircleDashed, ClipboardList, ExternalLink, ListTodo, Loader2, MessageCircleQuestion } from 'lucide-react'
import { agentDef, type BackgroundTask, type SubagentInfo } from '@inyon/protocol'
import type { ToolCall } from '../../lib/chat.ts'
import { describeStep, summarize, type ScriptStep } from '../../lib/tools.ts'
import { ChatAgent } from './agent.ts'
import { duration, shortPath } from '../../lib/format.ts'
import { useStore } from '../../lib/store.ts'
import { Diff, useHunks } from './Diff.tsx'
import { CopyButton, Markdown } from './Markdown.tsx'
import { Fold, useFold } from '../ui/Fold.tsx'

/** Background jobs of the session, so a tool row can show the live state of the job it started. */
export const ChatJobs = createContext<{ tasks: Map<string, BackgroundTask>; subagents: Map<string, SubagentInfo> }>({ tasks: new Map(), subagents: new Map() })

/**
 * Whether the reply this part belongs to was already on screen (AssistantTurn sets it after its
 * first paint). A step that joins a reply later comes in rising; the steps drawn with the reply,
 * or with the whole conversation, hold still.
 */
export const TurnSettled = createContext<() => boolean>(() => false)
export function useArrived() {
  const settled = useContext(TurnSettled)
  const [arrived] = useState(() => settled())
  return arrived
}

function Status({ call, live, detail }: { call: ToolCall; live: boolean; detail?: boolean }) {
  if (call.result?.isError) return <AlertCircle className="size-3.5 text-danger" />
  if (call.result) return detail ? <Check className="size-3.5 text-fg-3" /> : null
  if (live) return <Loader2 className="size-3.5 animate-spin text-fg-3" />
  return <CircleDashed className="size-3.5 text-fg-3" />
}

type Job = { kind: 'task'; task?: BackgroundTask } | { kind: 'subagent'; sub?: SubagentInfo }

/** The background job a call started, if it started one. */
function useJob(call: ToolCall): Job | undefined {
  const { tasks, subagents } = useContext(ChatJobs)
  const i = call.input ?? {}
  if ((call.name === 'Agent' || call.name === 'Task') && i.run_in_background) return { kind: 'subagent', sub: subagents.get(call.id) }
  if (call.name === 'Monitor' || (call.name === 'Bash' && i.run_in_background)) {
    const id = call.result?.meta?.taskId ?? call.result?.text.match(/\bID: (\w+)/)?.[1]
    return { kind: 'task', task: id ? tasks.get(id) : undefined }
  }
  return undefined
}

const JOB_STATE: Record<string, { label: string; className: string; dot?: string }> = {
  running: { label: 'Running', className: 'text-fg bg-sunken', dot: 'bg-fg live-ring' },
  completed: { label: 'Finished', className: 'text-fg-3 bg-sunken' },
  done: { label: 'Finished', className: 'text-fg-3 bg-sunken' },
  failed: { label: 'Failed', className: 'text-danger bg-danger-soft' },
  error: { label: 'Failed', className: 'text-danger bg-danger-soft' },
  stopped: { label: 'Stopped', className: 'text-fg-3 bg-sunken' },
}

/** "Background · Running" chip; opens the job's output or transcript. */
function JobBadge({ call, job, live }: { call: ToolCall; job: Job; live: boolean }) {
  const actions = useContext(ChatActionsContext)
  const status = job.kind === 'task' ? job.task?.status : job.sub?.status
  // until the hub has seen the job, a call that hasn't returned is still starting
  const st = JOB_STATE[status ?? (call.result ? 'running' : '')] ?? (live ? JOB_STATE.running : undefined)
  const open = job.kind === 'task' ? (job.task ? () => actions.openTask(job.task!) : undefined) : () => actions.openSubagent(call.id)
  return (
    <span
      role={open ? 'button' : undefined}
      tabIndex={open ? 0 : undefined}
      title={open ? 'Open output' : undefined}
      onClick={(e) => {
        if (!open) return
        e.stopPropagation()
        open()
      }}
      onKeyDown={(e) => {
        if (open && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault()
          e.stopPropagation()
          open()
        }
      }}
      className={clsx('inline-flex h-5 shrink-0 items-center gap-1.5 rounded-full px-2 text-[12px] font-medium', st?.className ?? 'bg-sunken text-fg-3', open && 'hover:brightness-110')}
    >
      {st?.dot && <span className={clsx('size-1.5 rounded-full', st.dot)} />}
      Background{st ? ` · ${st.label}` : ''}
    </span>
  )
}

/** How long a call took, from the call to its result. */
function took(call: ToolCall) {
  if (!call.ts || !call.result?.ts) return undefined
  const ms = Date.parse(call.result.ts) - Date.parse(call.ts)
  if (!(ms >= 0)) return undefined
  return ms < 1000 ? `${Math.max(1, Math.round(ms / 100)) / 10}s` : duration(ms)
}

function Output({ text, label }: { text: string; label?: string }) {
  if (!text.trim()) return null
  return (
    <div className="relative">
      {label && <div className="px-3 pt-2 font-sans text-[12.5px] font-medium text-fg-3">{label}</div>}
      <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-[12.5px] leading-[1.55] text-fg-2">{text}</pre>
    </div>
  )
}

function PatchText({ text }: { text: string }) {
  return (
    <pre className="max-h-96 overflow-auto px-3 py-2 font-mono text-[12px] leading-[1.6]">
      {text.split('\n').map((l, n) => (
        <div
          key={n}
          className={
            l.startsWith('+') && !l.startsWith('+++')
              ? 'bg-[var(--diff-add)] text-fg'
              : l.startsWith('-') && !l.startsWith('---')
                ? 'bg-[var(--diff-del)] text-fg'
                : l.startsWith('***') || l.startsWith('@@')
                  ? 'font-medium text-fg-2'
                  : 'text-fg-3'
          }
        >
          {l || ' '}
        </div>
      ))}
    </pre>
  )
}

function Details({ call }: { call: ToolCall }) {
  const i = call.input ?? {}
  const r = call.result
  const hunks = useHunks(r?.meta, i)
  const actions = useContext(ChatActionsContext)
  switch (call.name) {
    case 'ApplyPatch':
      return (
        <>
          {hunks.length ? <Diff hunks={hunks} /> : <PatchText text={String(i.patch ?? '')} />}
          {r && <Output text={r.text} label={r.isError ? 'Error' : 'Result'} />}
        </>
      )
    case 'Bash':
      return (
        <>
          <div className="group/cmd flex items-start gap-2 border-b border-line/70 px-3 py-2">
            <span className="select-none font-mono text-[12.5px] text-accent">$</span>
            <pre className="min-w-0 flex-1 whitespace-pre-wrap break-words font-mono text-[12.5px] leading-[1.55]">{i.command}</pre>
            <CopyButton text={i.command ?? ''} className="-mr-1 -mt-0.5" />
          </div>
          {r ? (
            <>
              <Output text={r.meta?.stdout ?? r.text} />
              {r.meta?.stderr && <Output text={r.meta.stderr} label="stderr" />}
              {!r.text.trim() && !r.meta?.stdout && !r.meta?.stderr && <div className="px-3 py-2 text-xs text-fg-3">No output</div>}
            </>
          ) : null}
        </>
      )
    case 'Edit':
    case 'MultiEdit':
    case 'Write':
      if (hunks.length) {
        return (
          <>
            <div className="border-b border-line/70 px-3 py-1.5 font-mono text-[12px] text-fg-3">{shortPath(i.file_path, 80)}</div>
            <Diff hunks={hunks} />
            {r?.isError && <Output text={r.text} />}
          </>
        )
      }
      break
    case 'Read':
      return <Output text={r?.text ?? ''} />
    case 'Script': {
      // Codex ran several tools from one small script: each step, then the diffs and the output
      const steps: ScriptStep[] = Array.isArray(i.steps) ? i.steps : []
      return (
        <>
          {steps.length > 0 && (
            <ol className="space-y-1.5 border-b border-line/70 px-3 py-2">
              {steps.map((s, n) => (
                <li key={n} className="flex items-start gap-2 text-[12.5px]">
                  <span className="shrink-0 pt-px font-sans font-medium text-fg-3">{describeStep(s).verb}</span>
                  {s.arg && <pre className="min-w-0 flex-1 whitespace-pre-wrap break-words font-mono leading-[1.55] text-fg-2">{s.tool === 'apply_patch' ? s.arg.split('\n').filter((l) => /^\*\*\* (Update|Add|Delete) File:/.test(l)).map((l) => l.replace(/^\*\*\* \w+ File: /, '')).join(', ') || 'a patch' : s.arg}</pre>}
                </li>
              ))}
            </ol>
          )}
          {hunks.length > 0 && <Diff hunks={hunks} />}
          {r && <Output text={r.text} label={r.isError ? 'Error' : 'Output'} />}
          <details className="border-t border-line/70 px-3 py-2">
            <summary className="cursor-pointer text-xs font-medium text-fg-3">Script</summary>
            <pre className="mt-1.5 max-h-72 overflow-auto whitespace-pre-wrap break-words font-mono text-[12px] leading-[1.55] text-fg-2">{String(i.code ?? '')}</pre>
          </details>
        </>
      )
    }
    case 'Task':
    case 'Agent':
      return (
        <div className="space-y-2 px-3 py-2.5">
          <button
            type="button"
            onClick={() => actions.openSubagent(call.id)}
            className="inline-flex h-7 items-center gap-1.5 rounded-full border border-line-strong px-2.5 text-[12px] font-medium hover:bg-hover"
          >
            <ExternalLink className="size-3.5" /> Open subagent transcript
          </button>
          {i.prompt && (
            <details className="text-sm">
              <summary className="cursor-pointer text-xs font-medium text-fg-3">Prompt</summary>
              <div className="mt-1.5 whitespace-pre-wrap text-[13px] leading-relaxed text-fg-2">{i.prompt}</div>
            </details>
          )}
          {r && <Markdown text={r.text} sans className="text-sm" />}
        </div>
      )
  }
  return (
    <>
      <Output text={JSON.stringify(i, null, 2)} label="Input" />
      {r && <Output text={r.text} label={r.isError ? 'Error' : 'Result'} />}
    </>
  )
}

export function ToolRow({ call, live }: { call: ToolCall; live: boolean }) {
  const [open, mounted, toggle] = useFold()
  const arrived = useArrived()
  const detail = useStore((st) => st.settings.toolDetail) !== 'compact'
  const s = summarize(call)
  const Icon = s.icon
  const error = call.result?.isError
  const job = useJob(call)
  const time = detail && !job ? took(call) : undefined
  // the command itself, under its description
  const command = detail && (call.name === 'Bash' || call.name === 'Monitor') && call.input?.description && typeof call.input.command === 'string' ? call.input.command.split('\n')[0] : undefined
  return (
    // the border is there all along, clear until the row opens, so opening doesn't shift the text
    <div className={clsx('rounded-xl border transition-colors', open ? 'border-line bg-elev' : 'border-transparent', arrived && 'animate-row-in')}>
      <button
        type="button"
        onClick={toggle}
        className={clsx(
          'group flex w-full min-w-0 items-center gap-2.5 rounded-xl px-2 py-[6px] text-left text-[13.5px] transition-colors',
          !open && 'hover:bg-hover',
        )}
      >
        <Icon className={clsx('size-4 shrink-0', command && 'mt-[2px] self-start', error ? 'text-danger' : 'text-fg-3')} />
        <span className={clsx('flex min-w-0 flex-1 gap-x-2.5', command ? 'flex-col' : 'items-center')}>
          <span className="flex min-w-0 items-center gap-2.5">
            <span className="shrink-0 font-medium text-fg-2">{s.verb}</span>
            {s.target && (
              <span className={clsx('min-w-0 truncate text-fg-3', s.mono && 'font-mono text-[12.5px]')}>{s.target}</span>
            )}
          </span>
          {command && <span className="min-w-0 truncate font-mono text-[12px] text-fg-3/80">$ {command}</span>}
        </span>
        {s.stats && (s.stats.add > 0 || s.stats.del > 0) && (
          <span className="shrink-0 font-mono text-xs">
            <span className="text-ok">+{s.stats.add}</span> <span className="text-danger">−{s.stats.del}</span>
          </span>
        )}
        <span className={clsx('ml-auto flex shrink-0 items-center gap-1.5 pl-1', command && 'self-start')}>
          {detail && job && <JobBadge call={call} job={job} live={live} />}
          {time && <span className="tabular text-[12px] text-fg-3">{time}</span>}
          {!(detail && job && call.result && !call.result.isError) && <Status call={call} live={live} detail={detail} />}
          <ChevronRight className={clsx('size-3.5 text-fg-3 transition-transform', open && 'rotate-90')} />
        </span>
      </button>
      <Fold open={open} mounted={mounted}>
        <div className="border-t border-line">
          <Details call={call} />
        </div>
      </Fold>
    </div>
  )
}

export function ToolGroup({ calls, live }: { calls: ToolCall[]; live: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const all = useStore((st) => st.settings.toolDetail) !== 'compact'
  const collapsible = !all && calls.length > 6
  const visible = collapsible && !expanded ? calls.slice(-4) : calls
  return (
    // a pixel further out than the rows' padding, for the border each row keeps around itself
    <div className="my-1.5 -mx-[9px] space-y-0.5">
      {collapsible && !expanded && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="flex w-full items-center gap-2.5 rounded-xl px-2.5 py-[7px] text-left text-[13.5px] text-fg-3 hover:bg-hover"
        >
          <ChevronRight className="size-4" />
          {calls.length - visible.length} more steps
        </button>
      )}
      {visible.map((c) => (
        <ToolRow key={c.id} call={c} live={live} />
      ))}
    </div>
  )
}

// ---------- special tools ----------

function Card({ icon, title, children, tone = 'default' }: { icon: React.ReactNode; title: string; children: React.ReactNode; tone?: 'default' | 'accent' }) {
  const arrived = useArrived()
  return (
    <div className={clsx('my-3 overflow-hidden rounded-2xl border bg-elev', tone === 'accent' ? 'border-accent/40' : 'border-line', arrived && 'animate-row-in')}>
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5 text-[13px] font-semibold text-fg-2">
        {icon}
        {title}
      </div>
      <div className="px-4 py-3">{children}</div>
    </div>
  )
}

export function SpecialTool({ call, live }: { call: ToolCall; live: boolean }) {
  const i = call.input ?? {}
  const agent = useContext(ChatAgent)
  if (call.name === 'ExitPlanMode') {
    const plan = call.result?.meta?.plan ?? i.plan ?? ''
    const status = call.result ? (call.result.isError ? 'Not approved' : 'Approved') : live ? 'Waiting for approval' : 'Proposed'
    return (
      <Card icon={<ClipboardList className="size-4 text-accent" />} title={`Plan · ${status}`} tone="accent">
        <PlanBody text={plan} />
      </Card>
    )
  }
  if (call.name === 'AskUserQuestion') {
    const answers = call.result?.meta?.answers ?? {}
    const qs: any[] = i.questions ?? []
    return (
      <Card icon={<MessageCircleQuestion className="size-4 text-accent" />} title={call.result ? (qs.length === 1 ? 'Question answered' : 'Questions answered') : `${agent ? agentDef(agent).name : 'The agent'} is asking`}>
        <div className="space-y-3">
          {qs.map((q, n) => (
            <div key={n}>
              {q.header && q.header !== q.question && <div className="text-[12px] font-medium text-fg-3">{q.header}</div>}
              <div className="text-sm font-medium">{q.question}</div>
              {answers[q.question] ? (
                <div className="mt-1 flex items-start gap-1.5 text-sm text-accent-text">
                  <Check className="mt-0.5 size-3.5 shrink-0" />
                  {answers[q.question]}
                </div>
              ) : (q.options ?? []).some((o: any) => o.description || o.preview) ? (
                // what each choice means, and its preview: the terminal shows only the highlighted one's
                <ul className="mt-2 space-y-2.5">
                  {(q.options ?? []).map((o: any, k: number) => (
                    <li key={k} className="min-w-0">
                      <div className="text-sm">{o.label}</div>
                      {o.description && <div className="text-[13px] leading-relaxed text-fg-3">{o.description}</div>}
                      {o.preview && <pre className="mt-1.5 max-h-72 overflow-auto rounded-xl border border-line bg-bg px-3 py-2 font-mono text-[12px] leading-[1.35] text-fg-2">{o.preview}</pre>}
                    </li>
                  ))}
                </ul>
              ) : (
                <div className="mt-1 text-sm text-fg-3">{(q.options ?? []).map((o: any) => o.label).join(' · ')}</div>
              )}
            </div>
          ))}
          {call.result?.isError && <div className="text-sm text-fg-3">{call.result.text.slice(0, 300)}</div>}
        </div>
      </Card>
    )
  }
  if (call.name === 'TodoWrite') {
    const todos: any[] = i.todos ?? []
    return (
      <Card icon={<ListTodo className="size-4 text-accent" />} title="To-dos">
        <ul className="space-y-1.5">
          {todos.map((t, n) => (
            <li key={n} className="flex items-start gap-2.5 text-sm">
              <span
                className={clsx(
                  'mt-[3px] flex size-4 shrink-0 items-center justify-center rounded-full border',
                  t.status === 'completed' ? 'border-fg bg-fg text-bg' : t.status === 'in_progress' ? 'border-accent' : 'border-line-strong',
                )}
              >
                {t.status === 'completed' && <Check className="size-3" strokeWidth={3} />}
                {t.status === 'in_progress' && <span className="size-1.5 rounded-full bg-accent" />}
              </span>
              <span className={clsx(t.status === 'completed' && 'text-fg-3 line-through', t.status === 'in_progress' && 'font-medium')}>
                {t.status === 'in_progress' ? t.activeForm || t.content : t.content}
              </span>
            </li>
          ))}
        </ul>
      </Card>
    )
  }
  return <ToolRow call={call} live={live} />
}

function PlanBody({ text }: { text: string }) {
  const [full, setFull] = useState(false)
  const long = text.length > 1600
  return (
    <div className="relative">
      <div className={clsx(!full && long && 'max-h-96 overflow-hidden')}>
        <Markdown text={text} className="text-[15px]" />
      </div>
      {long && !full && (
        <div className="absolute inset-x-0 bottom-0 flex justify-center border-t border-line bg-elev py-2">
          <button type="button" onClick={() => setFull(true)} className="press rounded-full border border-line bg-elev px-4 py-1.5 text-sm font-medium shadow-card hover:border-line-strong">
            Show full plan
          </button>
        </div>
      )}
    </div>
  )
}
