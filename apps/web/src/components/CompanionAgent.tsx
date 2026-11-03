import { useEffect, useMemo, useState } from 'react'
import { Link, NavLink, useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { ArrowRight, MessageCircle } from 'lucide-react'
import { agentDef, isBusy, type HostState } from '@inyon/protocol'
import { ApiError, api, type CompanionStatus } from '../lib/api.ts'
import { toast, useStore } from '../lib/store.ts'
import { allPanes, paneHref, type PaneRef } from '../lib/overview.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Segmented, Waiting } from './ui/primitives.tsx'
import { StateDot, StatePill, shownState } from './ui/status.tsx'

export type CompanionAgentKind = 'claude' | 'codex'

/**
 * The companion agents' panes, while they run: each hub marks its own, so an app that shows several
 * hubs can have one from each.
 */
export const companionPanes = (hosts: HostState[]): PaneRef[] => allPanes(hosts).filter((r) => r.pane.companion && r.pane.agent)

export function useCompanionPanes() {
  const hosts = useStore((s) => s.hosts)
  return useMemo(() => companionPanes(hosts), [hosts])
}

/** "Companion", with its machine when there's more than one. */
const companionName = (r: PaneRef, all: PaneRef[]) => (all.length > 1 ? `Companion on ${r.host.config.name}` : 'Companion')

export const OLD_HUB = 'This hub runs on Bun, which has no companion. Run the default Rust hub to use it.'

/**
 * Settings › Companion: start it with Claude Code or Codex, open its chat, stop it. The hub runs it
 * on its own machine, in a tmux session called companion.
 */
export function CompanionRows() {
  const nav = useNavigate()
  const panes = useCompanionPanes()
  const [status, setStatus] = useState<CompanionStatus | 'old' | Error | null>(null)
  const [agent, setAgent] = useState<CompanionAgentKind>('claude')
  const [busy, setBusy] = useState<'start' | 'stop' | null>(null)
  const [tries, setTries] = useState(0)
  // this hub's own, among the ones from other hubs an app may show
  const own = status && typeof status === 'object' && !(status instanceof Error) ? status.host : 'local'
  const running = panes.find((r) => r.host.config.id === own)
  const runningId = running?.pane.id
  useEffect(() => {
    let live = true
    api.companion().then(
      (s) => live && setStatus(s),
      (e) => live && setStatus(e instanceof ApiError && e.status === 404 ? 'old' : (e as Error)),
    )
    return () => {
      live = false
    }
  }, [runningId, tries])

  const start = async (kind: CompanionAgentKind) => {
    setBusy('start')
    try {
      const s = await api.startCompanion(kind)
      setStatus(s)
      toast({ title: 'Companion started', body: `${agentDef(kind).name} in the tmux session companion.`, tone: 'success', href: s.pane ? paneHref(s.host, s.pane) : undefined })
    } catch (e) {
      toast({ title: 'Could not start the companion', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }
  const stop = async () => {
    setBusy('stop')
    try {
      await api.stopCompanion()
      setStatus({ agent: null, host: 'local', pane: null })
      toast({ title: 'Companion stopped', tone: 'info' })
    } catch (e) {
      toast({ title: 'Could not stop the companion', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }

  if (status === null) return <Waiting shape="rows" count={1} label="Loading the companion" className="px-4 py-3.5" />
  if (status === 'old') return <p className="px-4 py-3.5 text-[13px] text-fg-3">{OLD_HUB}</p>
  if (status instanceof Error) {
    return (
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5">
        <div className="min-w-0 flex-[1_1_220px]">
          <div className="text-[14px]">Couldn&rsquo;t reach the companion</div>
          <div className="mt-0.5 text-[12.5px] leading-snug text-fg-3">{status.message}</div>
        </div>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setStatus(null)
            setTries((n) => n + 1)
          }}
        >
          Try again
        </Button>
      </div>
    )
  }
  const kind = (running?.pane.agent?.kind ?? status.agent) as CompanionAgentKind | null
  if (running && kind) {
    return (
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5">
        <span className="flex min-w-0 flex-[1_1_220px] items-center gap-3">
          <AgentLogo kind={kind} size={28} />
          <span className="min-w-0">
            <span className="block text-[14px] font-medium">{agentDef(kind).name}</span>
            <span className="block truncate text-[12px] text-fg-3">On {running.host.config.name}, in the tmux session companion</span>
          </span>
        </span>
        <StatePill state={shownState(running.pane.agent!, running.host)} />
        <div className="flex gap-2">
          <Button size="sm" variant="primary" onClick={() => nav(paneHref(running.host.config.id, running.pane.id))}>
            Open
          </Button>
          <Button size="sm" variant="ghost" loading={busy === 'stop'} onClick={() => void stop()}>
            Stop
          </Button>
        </div>
      </div>
    )
  }
  if (status.agent) {
    // turned on, but its agent quit or its session was closed
    return (
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5">
        <div className="min-w-0 flex-[1_1_220px]">
          <div className="text-[14px]">{agentDef(status.agent).name} isn&rsquo;t running</div>
          <div className="mt-0.5 text-[12.5px] leading-snug text-fg-3">It quit, or its session was closed.</div>
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant="primary" loading={busy === 'start'} onClick={() => void start(status.agent!)}>
            Start again
          </Button>
          <Button size="sm" variant="ghost" loading={busy === 'stop'} onClick={() => void stop()}>
            Turn off
          </Button>
        </div>
      </div>
    )
  }
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5">
      <div className="min-w-0 flex-[1_1_220px]">
        <div className="text-[14px]">Agent</div>
        <div className="mt-0.5 text-[12.5px] leading-snug text-fg-3">Runs on the hub&rsquo;s machine with your own sign-in.</div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          size="sm"
          value={agent}
          onChange={setAgent}
          label="Companion agent"
          options={[
            { v: 'claude', label: 'Claude Code' },
            { v: 'codex', label: 'Codex' },
          ]}
        />
        <Button size="sm" variant="primary" loading={busy === 'start'} onClick={() => void start(agent)}>
          Start
        </Button>
      </div>
    </div>
  )
}

/** The sidebar's way to each companion, under Home, while it runs. */
export function CompanionNavItem({ className }: { className: (active: boolean) => string }) {
  const panes = useCompanionPanes()
  return panes.map((r) => (
    <NavLink key={`${r.host.config.id}/${r.pane.id}`} to={paneHref(r.host.config.id, r.pane.id)} className={({ isActive }) => className(isActive)}>
      <MessageCircle className="size-4 shrink-0" /> <span className="truncate">{companionName(r, panes)}</span>
      <StateDot state={shownState(r.pane.agent!, r.host)} className="ml-auto" />
    </NavLink>
  ))
}

/** Home's line about each companion, while it runs: what it's doing and a way into its chat. */
export function CompanionBar() {
  const panes = useCompanionPanes()
  if (!panes.length) return null
  return (
    <div className="flex flex-col gap-3">
      {panes.map((r) => (
        <CompanionLine key={`${r.host.config.id}/${r.pane.id}`} r={r} name={companionName(r, panes)} />
      ))}
    </div>
  )
}

function CompanionLine({ r, name }: { r: PaneRef; name: string }) {
  const a = r.pane.agent!
  const state = shownState(a, r.host)
  const line = state === 'waiting' ? a.waitingFor ?? 'It needs your answer.' : isBusy(a) ? 'Working on it…' : 'Ask about your sessions, or tell it what to send.'
  return (
    <Link
      to={paneHref(r.host.config.id, r.pane.id)}
      className="press group flex items-center gap-3 rounded-3xl border border-line bg-elev px-5 py-3.5 shadow-card transition-colors hover:bg-hover"
    >
      <AgentLogo kind={a.kind} size={22} active={isBusy(a)} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2 text-[14px] font-medium">
          {name} <StateDot state={state} />
        </span>
        <span className={clsx('block truncate text-[13px]', state === 'waiting' ? 'text-fg' : 'text-fg-3')}>{line}</span>
      </span>
      <ArrowRight className="size-4 shrink-0 text-fg-3 transition-transform group-hover:translate-x-0.5" />
    </Link>
  )
}
