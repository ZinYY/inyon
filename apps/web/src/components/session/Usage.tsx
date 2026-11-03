import { useEffect, useState } from 'react'
import clsx from 'clsx'
import { Clock, Coins, Gauge, Layers, RadioTower, RotateCw } from 'lucide-react'
import type { AgentKind, AgentPaneInfo, AgentStatus, ChatInfo, HostState } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { duration, liveWindow, money, resetClock, resetsIn, since, tokens } from '../../lib/format.ts'
import { Button, Meter } from '../ui/primitives.tsx'
import { IntegrationNote } from './IntegrationNote.tsx'
import { CopyButton } from '../chat/copy.tsx'

const showsCost = (agent?: AgentKind) => !agent || !['codex', 'gemini', 'antigravity', 'copilot'].includes(agent)

/** Re-render every `ms` (for countdowns). */
export function useTick(ms = 30_000) {
  const [, set] = useState(0)
  useEffect(() => {
    const t = setInterval(() => set((n) => n + 1), ms)
    return () => clearInterval(t)
  }, [ms])
}

/**
 * Context window from whichever source is newer: the agent's live status, or the transcript
 * (which knows about a compaction the moment it lands).
 */
export function contextOf(status?: AgentStatus, info?: ChatInfo) {
  const size = status?.contextSize
  const stats = info?.stats
  const transcriptNewer = !!stats?.contextAt && (!status || stats.contextAt > status.updatedAt + 2000)
  const live = status?.contextUsed !== undefined && !transcriptNewer
  const used = live ? status!.contextUsed : stats?.contextTokens ?? (transcriptNewer ? undefined : status?.contextUsed)
  const pct = live && status?.contextPct !== undefined ? status.contextPct : used && size ? Math.min(100, (used / size) * 100) : undefined
  const compacted = !!stats?.compactedAt && stats.compactedAt === stats.contextAt ? true : status?.stale?.context === 'compacted'
  return { size, used, pct, compacted }
}

function Stat({ icon, label, value, sub }: { icon: React.ReactNode; label: string; value: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <div className="rounded-2xl border border-line bg-elev px-3 py-2.5">
      <div className="flex items-center gap-1.5 text-[12px] text-fg-3">
        {icon}
        {label}
      </div>
      <div className="tabular mt-1 text-[16px] font-semibold tracking-tight">{value}</div>
      {sub && <div className="mt-0.5 truncate text-[12px] text-fg-3">{sub}</div>}
    </div>
  )
}

function RateRow({ label, w: raw }: { label: string; w?: { pct: number; resetsAt?: number } }) {
  const w = liveWindow(raw)
  if (!w) return null
  return (
    <div>
      <div className="mb-1.5 flex items-baseline gap-2 text-[13px]">
        <span className="font-medium">{label}</span>
        <span className={clsx('tabular ml-auto font-semibold', w.pct >= 85 ? 'text-danger' : w.pct >= 60 ? 'text-warn' : 'text-fg')}>{Math.round(w.pct)}%</span>
      </div>
      <Meter value={w.pct} tone="auto" />
      {w.reset ? (
        <div className="mt-1 text-[12px] text-fg-3">Reset · the new window shows after the next reply</div>
      ) : w.resetsAt ? (
        <div className="mt-1 text-[12px] text-fg-3">
          {resetsIn(w.resetsAt)} · {resetClock(w.resetsAt)}
        </div>
      ) : null}
    </div>
  )
}

/** The session stopped reporting live numbers; resuming it fixes that. */
export function StoppedNote({ host, pane, compact }: { host: string; pane: string; compact?: boolean }) {
  const [busy, setBusy] = useState<'reconnect' | 'restart' | null>(null)
  const run = async (what: 'reconnect' | 'restart') => {
    setBusy(what)
    try {
      if (what === 'reconnect') {
        await api.reconnect(host, pane)
        toast({ title: 'Reconnected', body: 'Numbers return with its next update. The session keeps running.', tone: 'success' })
      } else {
        await api.restart(host, pane)
        toast({ title: 'Session resumed', body: 'Same conversation, fresh settings.', tone: 'success' })
      }
    } catch (e) {
      toast({ title: what === 'reconnect' ? 'Could not reconnect' : 'Could not restart', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }
  return (
    <div className={clsx('flex items-start gap-3 rounded-2xl border border-line bg-elev', compact ? 'p-2.5' : 'p-3.5')}>
      <RotateCw className="mt-0.5 size-4 shrink-0 text-warn" />
      <p className="min-w-0 flex-1 text-[12.5px] leading-relaxed text-fg-2">
        This session stopped sending cost and effort after its settings changed. Reconnect fixes it without stopping anything; Inyon tries that by itself. Restart is the fallback.
      </p>
      <div className="flex shrink-0 flex-col gap-1.5">
        <Button size="sm" variant="primary" onClick={() => run('reconnect')} loading={busy === 'reconnect'}>
          Reconnect
        </Button>
        <Button size="sm" variant="ghost" onClick={() => run('restart')} loading={busy === 'restart'}>
          Restart
        </Button>
      </div>
    </div>
  )
}

/** Compact usage for the header's hover card: everything at a glance. */
export function UsageSummary({ status, info, agentInfo }: { status?: AgentStatus; info?: ChatInfo; agentInfo?: AgentPaneInfo }) {
  const ctx = contextOf(status, info)
  const cost = status?.costUsd ?? info?.stats.costUsd
  const dur = status?.durationMs ?? info?.stats.durationMs
  const five = liveWindow(status?.fiveHour)
  const week = liveWindow(status?.sevenDay)
  const row = (label: string, w?: { pct: number; resetsAt?: number; reset?: boolean }) =>
    w && (
      <div>
        <div className="mb-1 flex items-baseline gap-2 text-[12px]">
          <span className="text-fg-2">{label}</span>
          <span className="text-fg-3">{w.reset ? 'reset' : resetsIn(w.resetsAt)}</span>
          <span className={clsx('tabular ml-auto font-semibold', w.pct >= 85 ? 'text-danger' : w.pct >= 60 ? 'text-warn' : 'text-fg')}>{Math.round(w.pct)}%</span>
        </div>
        <Meter value={w.pct} tone="auto" />
      </div>
    )
  return (
    <div className="space-y-3 text-[12px]">
      <div>
        <div className="mb-1 flex items-baseline gap-2">
          <span className="font-semibold text-fg">Context</span>
          <span className="tabular ml-auto text-fg-2">{ctx.size ? `${tokens(ctx.used)} / ${tokens(ctx.size)}` : tokens(ctx.used)}</span>
        </div>
        <Meter value={ctx.pct ?? 0} tone="auto" />
        <div className="mt-1 text-fg-3">
          {ctx.pct !== undefined ? `${Math.round(ctx.pct)}% used` : ctx.compacted ? 'Compacted · size shows after the next reply' : 'Not reported yet'}
          {!!info?.stats.compactions && ` · compacted ${info.stats.compactions}×`}
        </div>
      </div>
      {(five || week) && (
        <div className="space-y-2.5 border-t border-line pt-3">
          {row('5-hour', five)}
          {row('Weekly', week)}
        </div>
      )}
      {status?.stale?.limits === 'account' && <div className="border-t border-line pt-3 text-fg-3">New account: its limits show after the next reply.</div>}
      {agentInfo?.statusStopped && <div className="border-t border-line pt-3 text-fg-3">Cost and effort paused; reconnecting…</div>}
      <div className={clsx('grid gap-2 border-t border-line pt-3', showsCost(info?.agent) ? 'grid-cols-3' : 'grid-cols-2')}>
        {showsCost(info?.agent) && <div>
          <div className="text-fg-3">Cost</div>
          <div className="tabular font-semibold text-fg">{cost === undefined && (info?.agent === 'antigravity' || info?.agent === 'codex') ? 'Not reported' : money(cost)}</div>
        </div>}
        <div>
          <div className="text-fg-3">Running</div>
          <div className="tabular font-semibold text-fg">{dur ? duration(dur) : info?.stats.startedAt ? since(info.stats.startedAt) : '—'}</div>
        </div>
        <div>
          <div className="text-fg-3">Lines</div>
          <div className="tabular font-semibold">
            {status?.linesAdded === undefined && info?.stats.linesAdded === undefined && status?.linesRemoved === undefined && info?.stats.linesRemoved === undefined ? '—' : <><span className="text-ok">+{status?.linesAdded ?? info?.stats.linesAdded ?? 0}</span> <span className="text-danger">−{status?.linesRemoved ?? info?.stats.linesRemoved ?? 0}</span></>}
          </div>
        </div>
      </div>
    </div>
  )
}

/** Context window, plan limits, cost, time and session details. */
export function UsagePanel({ status, info, onCompact, host, agent, agentInfo, pane }: { status?: AgentStatus; info?: ChatInfo; onCompact?: () => void; host?: HostState; agent?: AgentKind; agentInfo?: AgentPaneInfo; pane?: string }) {
  useTick()
  const ctx = contextOf(status, info)
  const stats = info?.stats
  const cost = status?.costUsd ?? stats?.costUsd
  const dur = status?.durationMs ?? stats?.durationMs
  const added = status?.linesAdded ?? stats?.linesAdded
  const removed = status?.linesRemoved ?? stats?.linesRemoved
  const hasLimits = !!(status?.fiveHour || status?.sevenDay)
  const accountChanged = status?.stale?.limits === 'account'
  const integration = agent === 'antigravity' ? host?.agyIntegration : host?.integration
  return (
    <div className="space-y-4">
      {(agent === 'claude' || agent === 'antigravity') && host && integration && integration !== 'installed' && integration !== 'unknown' && <IntegrationNote host={host} agent={agent} />}
      {agentInfo?.statusStopped && host && pane && <StoppedNote host={host.config.id} pane={pane} />}
      <section className="rounded-3xl border border-line bg-elev shadow-card p-3.5">
        <div className="flex items-baseline gap-2">
          <Gauge className="size-4 translate-y-0.5 text-fg-3" />
          <span className="text-[13.5px] font-semibold">Context window</span>
          <span className="tabular ml-auto text-[13px] text-fg-2">{ctx.size ? `${tokens(ctx.used)} / ${tokens(ctx.size)}` : tokens(ctx.used)}</span>
        </div>
        <Meter value={ctx.pct ?? 0} tone="auto" className="mt-2.5 h-2" />
        <div className="mt-2 flex items-center gap-2 text-[12px] text-fg-3">
          <span className="tabular">
            {ctx.pct !== undefined ? `${Math.round(ctx.pct)}% used` : ctx.compacted ? 'Compacted · size shows after the next reply' : 'No usage reported yet'}
          </span>
          {!!stats?.compactions && (
            <span className="flex items-center gap-1">
              · <Layers className="size-3" /> compacted {stats.compactions}×
            </span>
          )}
          {onCompact && (
            <button type="button" onClick={onCompact} className="ml-auto rounded-full border border-line-strong px-2 py-0.5 text-[12px] font-medium text-fg hover:bg-hover">
              Compact now
            </button>
          )}
        </div>
      </section>

      {accountChanged && !hasLimits && (
        <section className="rounded-3xl border border-line bg-elev shadow-card p-3.5 text-[12.5px] text-fg-2">
          <div className="mb-0.5 text-[13.5px] font-semibold text-fg">Plan limits</div>
          Signed in to a different account since the last report. Its limits show after the next reply.
        </section>
      )}

      {(hasLimits || status?.credits) && (
        <section className="space-y-3.5 rounded-3xl border border-line bg-elev shadow-card p-3.5">
          <div className="flex items-baseline text-[13.5px] font-semibold">
            Plan limits
            {status?.plan && <span className="ml-2 text-[12px] font-normal capitalize text-fg-3">{status.plan}</span>}
            {status?.credits && <span className="ml-auto text-[12px] font-normal text-fg-2">Credits: {status.credits.unlimited ? 'unlimited' : status.credits.balance ?? '—'}</span>}
          </div>
          <RateRow label="5-hour window" w={status?.fiveHour} />
          <RateRow label="Weekly" w={status?.sevenDay} />
        </section>
      )}

      <div className="grid grid-cols-2 gap-2">
        {showsCost(agent) && <Stat icon={<Coins className="size-3.5" />} label="Session cost" value={money(cost)} sub={status?.apiDurationMs ? `API time ${duration(status.apiDurationMs)}` : undefined} />}
        <Stat icon={<Clock className="size-3.5" />} label="Running for" value={dur ? duration(dur) : stats?.startedAt ? since(stats.startedAt) : '—'} sub={stats?.turns ? `${stats.turns} turns` : undefined} />
        <Stat
          icon={<span className="font-mono text-[12px]">±</span>}
          label="Lines changed"
          value={
            <span>
              {added === undefined && removed === undefined ? '—' : <><span className="text-ok">+{added ?? 0}</span> <span className="text-danger">−{removed ?? 0}</span></>}
            </span>
          }
          sub={agent === 'codex' ? 'Recorded patches' : undefined}
        />
        <Stat icon={<RadioTower className="size-3.5" />} label="Remote Control" value={info?.rc ? 'On' : 'Off'} sub={info?.rc ? <a className="underline" href={info.rc.url} target="_blank" rel="noreferrer">{agent === 'antigravity' ? 'Open in Antigravity' : 'Open on claude.ai'}</a> : undefined} />
      </div>

      <section className="divide-y divide-line rounded-3xl border border-line bg-elev shadow-card text-[12.5px]">
        {[
          ['Model', status?.model ?? info?.model],
          ['Effort', status?.effort],
          ['Fast mode', status?.fast === undefined ? undefined : status.fast ? 'On' : 'Off'],
          ['Output style', status?.outputStyle],
          ['Version', status?.version ?? info?.version],
          ['Branch', info?.gitBranch],
          ['Folder', info?.cwd],
          ['Session', info?.sessionId],
          ['Shared link', info?.share],
          ['Matched by', info?.mappedBy === 'hook' ? 'Integration hook (exact)' : info?.mappedBy === 'process' ? 'Open conversation (exact)' : info?.mappedBy === 'resume' ? 'Resume flag' : info?.mappedBy ? 'Folder (best guess)' : undefined],
        ]
          .filter(([, v]) => v)
          .map(([k, v]) => (
            <div key={k} className="flex items-center gap-3 px-3.5 py-2">
              <span className="w-24 shrink-0 text-fg-3">{k}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{v}</span>
              {(k === 'Session' || k === 'Folder' || k === 'Shared link') && <CopyButton text={String(v)} />}
            </div>
          ))}
      </section>
    </div>
  )
}
