import { useState } from 'react'
import clsx from 'clsx'
import { CircleX, Copy, GitFork, History, Link2, LogOut, Plug, Shrink, RadioTower, RotateCcw, RotateCw, Gauge, Target, Trash2 } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import type { BackgroundTask, ChatItem, SubagentInfo } from '@inyon/protocol'
import { AgentLogo } from '../../lib/skin.tsx'
import { openMenu, Segmented, Sheet, Toggle } from '../ui/primitives.tsx'
import { UsagePanel } from './Usage.tsx'
import { ActivityTab, ChangesTab, PagesTab, PreviewTab, useSessionPages } from './Panels.tsx'
import { FilesTab } from './Workspace.tsx'
import { GoalSheet, modeMatches } from './ComposerControls.tsx'
import { currentModelId, type SessionModel } from './useSession.ts'
import { confirmAction } from '../Confirm.tsx'
import { ProvidersSheet } from '../ProvidersSheet.tsx'
import { paneHref } from '../../lib/overview.ts'

const EFFORT_SHORT: Record<string, string> = { default: 'Default', none: 'None', minimal: 'Min', low: 'Low', medium: 'Med', high: 'High', xhigh: 'X-high', max: 'Max', ultra: 'Ultra' }

export function ControlsSheet({ s, open, onClose }: { s: SessionModel; open: boolean; onClose: () => void }) {
  const [goalOpen, setGoalOpen] = useState(false)
  const [providers, setProviders] = useState(false)
  const nav = useNavigate()
  const { def, status, info, agent, actions } = s
  if (!def) return null
  const modelId = currentModelId(def, status, info?.model)
  const mode = s.screen?.mode ?? s.ref?.pane.agent?.mode ?? info?.mode
  const goalOn = !!info?.goal && !info.goal.met
  // while the agent asks something, it takes no commands: typing one would answer the question
  const asking = !!s.screen?.prompt
  const Label = ({ children }: { children: React.ReactNode }) => <div className="mb-2 text-[13px] font-medium text-fg-2">{children}</div>
  return (
    <>
      <Sheet open={open} onClose={onClose} title={def.name} icon={<AgentLogo kind={agent} size={24} />}>
        <div className="space-y-5">
          {asking && (
            <div className="rounded-2xl border border-warn/35 bg-warn-soft/50 px-3.5 py-2.5 text-[13px] text-fg-2">
              {def.name} is asking you something. Answer it in the chat, and the model, effort and mode controls come back.
            </div>
          )}
          {!asking && (
          <div className="min-w-0 space-y-5">
          {(def.models.length > 0 || agent === 'opencode') && (
            <div>
              <Label>Model</Label>
              <div className="grid grid-cols-2 gap-2">
                {def.models.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    onClick={() => actions.setModel(m.id)}
                    className={clsx('rounded-2xl border px-3 py-2.5 text-left', m.id === modelId ? 'border-accent bg-accent-soft' : 'border-line hover:bg-hover')}
                  >
                    <div className="text-[14px] font-semibold">{m.label}</div>
                    {m.note && <div className="text-[12px] text-fg-3">{m.note}</div>}
                  </button>
                ))}
              </div>
              {agent === 'opencode' && (
                <button type="button" onClick={() => setProviders(true)} className="mt-2 inline-flex h-9 items-center gap-1.5 rounded-full px-2.5 text-[13px] text-fg-2 hover:bg-hover hover:text-fg">
                  <Plug className="size-3.5" /> Connect a provider…
                </button>
              )}
            </div>
          )}
          {s.efforts.length > 0 && (
            <div>
              <Label>Reasoning effort</Label>
              {/* OpenCode runs a model without a variant unless one is picked */}
              <Segmented
                full
                value={status?.effort ?? (agent === 'opencode' ? 'default' : '')}
                onChange={(v) => actions.setEffort(v)}
                options={[...(agent === 'opencode' ? ['default'] : []), ...s.efforts].map((e) => ({ v: e, label: EFFORT_SHORT[e] ?? e }))}
              />
            </div>
          )}
          {def.modes.length > 0 && (
            <div>
              <Label>{agent === 'opencode' ? 'Agent' : 'Permission mode'}</Label>
              <div className="flex flex-wrap gap-1.5">
                {def.modes.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    onClick={() => actions.setMode(m.id)}
                    className={clsx('h-9 rounded-xl border px-3 text-[13px]', modeMatches(mode, m.id, agent) ? 'border-accent bg-accent-soft font-medium' : 'border-line text-fg-2 hover:bg-hover')}
                  >
                    {m.label}
                  </button>
                ))}
              </div>
            </div>
          )}
          <div className="divide-y divide-line rounded-3xl border border-line">
            {def.set.goal && (
              <div className="flex items-center gap-3 px-3.5 py-3">
                <Target className="size-4 text-fg-3" />
                <button type="button" onClick={() => setGoalOpen(true)} className="min-w-0 flex-1 text-left">
                  <div className="text-[14px] font-medium">Goal</div>
                  <div className="truncate text-[12px] text-fg-3">{goalOn ? info!.goal!.condition : info?.goal?.met ? 'Last goal met' : 'Keep working until a condition is met'}</div>
                </button>
                <Toggle on={goalOn} onChange={(v) => (v ? setGoalOpen(true) : actions.clearGoal())} label="Goal" />
              </div>
            )}
            {def.set.ultracode && (
              <div className="flex items-center gap-3 px-3.5 py-3">
                <Gauge className={clsx('size-4', s.ultracode ? 'ultracode-mark text-fg' : 'text-fg-3')} />
                <div className="min-w-0 flex-1">
                  <div className="text-[14px] font-medium">Ultracode</div>
                  <div className="truncate text-[12px] text-fg-3">{s.ultracode ? 'On · a dynamic workflow for every task' : 'Off'}</div>
                </div>
                <Toggle on={!!s.ultracode} onChange={(v) => actions.ultracode(v)} label="Ultracode" />
              </div>
            )}
            {def.set.rc && (
              <div className="flex items-center gap-3 px-3.5 py-3">
                <RadioTower className="size-4 text-fg-3" />
                <div className="min-w-0 flex-1">
                  <div className="text-[14px] font-medium">Remote Control</div>
                  <div className="truncate text-[12px] text-fg-3">{info?.rc ? `On · reachable from ${agent === 'antigravity' ? 'Antigravity' : 'claude.ai'}` : 'Off'}</div>
                </div>
                {info?.rc && (
                  <a href={info.rc.url} target="_blank" rel="noreferrer" className="rounded-full border border-line-strong px-2.5 py-1 text-[12.5px] font-medium">
                    Open
                  </a>
                )}
                <Toggle on={!!info?.rc} onChange={(v) => actions.rc(v)} label="Remote Control" />
              </div>
            )}
          </div>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(64px,1fr))] gap-2">
            {[
              def.set.compact && { label: 'Compact', icon: Shrink, on: actions.compact },
              (agent === 'claude' || agent === 'antigravity' || agent === 'codex' || agent === 'opencode') && { label: agent === 'opencode' ? 'Undo' : 'Rewind', icon: RotateCcw, on: actions.rewind },
              agent === 'opencode' && { label: 'Redo', icon: RotateCw, on: actions.redo },
              agent === 'opencode' && { label: info?.share ? 'Unshare' : 'Share', icon: Link2, on: () => actions.share(!info?.share) },
              def.conversations?.length && { label: 'History', icon: History, on: () => (onClose(), openMenu('conversations')) },
              actions.canFork && s.ref && { label: 'Fork', icon: GitFork, on: () => (onClose(), actions.fork((p) => nav(paneHref(s.ref!.host.config.id, p)))) },
              { label: 'Copy ID', icon: Copy, on: actions.copyId },
              def.set.clear && { label: 'Clear', icon: Trash2, on: async () => (await confirmAction({ title: 'Start a fresh conversation?', body: 'The current one is saved and can be resumed later.', yes: 'Clear' })) && actions.clear() },
              { label: 'To shell', icon: LogOut, on: actions.exit },
              { label: 'End', icon: CircleX, danger: true, on: () => (onClose(), actions.end(() => nav('/'))) },
            ]
              .filter(Boolean)
              .map((a: any) => (
                <button key={a.label} type="button" onClick={a.on} className={clsx('flex h-16 flex-col items-center justify-center gap-1.5 rounded-2xl border border-line text-[12px] hover:bg-hover', a.danger && 'text-danger')}>
                  <a.icon className={clsx('size-4', a.danger ? 'text-danger' : 'text-fg-2')} />
                  {a.label}
                </button>
              ))}
          </div>
          </div>
          )}
          <UsagePanel status={status} info={info} host={s.ref?.host} agent={agent} agentInfo={s.ref?.pane.agent} pane={s.ref?.pane.id} onCompact={def.set.compact ? actions.compact : undefined} />
        </div>
      </Sheet>
      <GoalSheet key={String(goalOpen)} s={s} open={goalOpen} onClose={() => setGoalOpen(false)} />
      {agent === 'opencode' && s.ref && (
        <ProvidersSheet hostId={s.ref.host.config.id} hostName={s.ref.host.config.name} pane={s.ref.pane.id} open={providers} onClose={() => setProviders(false)} />
      )}
    </>
  )
}

export function ActivitySheet({ s, items, open, onClose, onTask, onSubagent }: { s: SessionModel; items: ChatItem[]; open: boolean; onClose: () => void; onTask: (t: BackgroundTask) => void; onSubagent: (a: SubagentInfo) => void }) {
  const [tab, setTab] = useState<'activity' | 'changes' | 'files' | 'preview' | 'pages'>('activity')
  const p = s.ref ? { host: s.ref.host.config.id, pane: s.ref.pane.id } : undefined
  const pages = useSessionPages(s)
  return (
    <Sheet open={open} onClose={onClose} wide title="Activity">
      <div className="-mx-1 mb-3">
        <Segmented full value={tab} onChange={setTab} options={[
            { v: 'activity', label: 'Running' },
            { v: 'changes', label: 'Changes' },
            { v: 'files', label: 'Files' },
            { v: 'preview', label: 'Preview' },
            ...(pages.length || tab === 'pages' ? [{ v: 'pages' as const, label: 'Pages' }] : []),
          ]} />
      </div>
      <div key={tab} className="animate-fade-in -mx-5 flex min-h-[50dvh] flex-col">
        {tab === 'activity' && <ActivityTab s={s} onTask={onTask} onSubagent={onSubagent} />}
        {tab === 'changes' && <ChangesTab items={items} p={p} onComposed={onClose} />}
        {tab === 'files' && p && <FilesTab p={p} onComposed={onClose} />}
        {tab === 'preview' && <PreviewTab storageKey={`${s.ref?.host.config.id}:${s.ref?.pane.id}`} suggestions={[]} />}
        {tab === 'pages' && <PagesTab pages={pages} />}
      </div>
    </Sheet>
  )
}
