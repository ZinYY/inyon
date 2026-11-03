import { memo, useEffect, useRef, useState, type FocusEvent, type PointerEvent } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { AlarmClock, ChevronRight, ArrowRight, Check, Gauge, MoreHorizontal, Pin, SquareTerminal, StickyNote } from 'lucide-react'
import { agentDef, AGENTS, isBusy, type HostState, type RateWindow } from '@inyon/protocol'
import { type PaneRef, paneHref, paneTitle } from '../lib/overview.ts'
import { RESPONSE_TITLE, liveWindow, money, resetsIn, responseTime, shortPath, timeAgo, tokens } from '../lib/format.ts'
import { api } from '../lib/api.ts'
import { haptic } from '../lib/haptics.ts'
import { type ActivityEvent, toast, useStore } from '../lib/store.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { HostDot, StatePill, backgroundText, failureText, hostLine, shownState, sshAddress } from './ui/status.tsx'
import { Menu, Meter, openMenu } from './ui/primitives.tsx'
import { useLongPress } from '../lib/longPress.ts'
import { contextMenu } from './ui/ContextMenu.tsx'
import { machineMenu, paneMenu } from './menus.tsx'
import { tmuxTarget } from '../lib/groups.ts'
import { isSnoozed, markFor, wakeText } from '../lib/organize.ts'

/** The parts of a pane ref a card draws; the rest of the machine changing shouldn't redraw it. */
const samePane = (a: { r: PaneRef; showHost?: boolean }, b: { r: PaneRef; showHost?: boolean }) =>
  a.r.pane === b.r.pane && a.r.window === b.r.window && a.r.host.config === b.r.host.config && a.r.host.status === b.r.host.status && a.r.host.hostname === b.r.host.hostname && a.showHost === b.showHost

/** Answers a waiting agent: Sending… while the request runs, then Sent until the agent moves on. */
function useAnswer(r: PaneRef) {
  const a = r.pane.agent!
  const [busy, setBusy] = useState<number | null>(null)
  const [done, setDone] = useState(false)
  // pressed, then sent, then the agent took it: two short pulses, once
  const sent = useRef(false)
  useEffect(() => {
    if (!sent.current || a.state === 'waiting') return
    sent.current = false
    haptic('success')
  }, [a.state])
  const answer = async (n: number) => {
    setBusy(n)
    try {
      await api.answer(r.host.config.id, r.pane.id, n)
      sent.current = true
      setDone(true)
      // still asking after a moment: show the choices again
      setTimeout(() => setDone(false), 4000)
    } catch (e) {
      toast({ title: 'Could not answer', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }
  return { busy, done, answer }
}

/** A waiting session's answers, in the row where the card shows its model and cost, so the card keeps its size. */
function Answers({ r }: { r: PaneRef }) {
  const nav = useNavigate()
  const { busy, done, answer } = useAnswer(r)
  const opts = r.pane.agent!.options ?? []
  if (done) return <span className="animate-fade-in flex items-center gap-1.5 font-sans text-[13px] text-fg-2"><Check className="size-4" /> Sent</span>
  return (
    <span className="pointer-events-auto relative flex min-w-0 flex-1 gap-1.5 overflow-hidden font-sans">
      {opts.length ? (
        opts.map((o, i) => (
          <button
            key={o.n}
            type="button"
            data-answer={i === 0 || undefined}
            disabled={busy !== null}
            onClick={() => answer(o.n)}
            title={o.label}
            className={clsx(
              'press h-8 min-w-0 shrink truncate rounded-full px-3.5 text-[13px] font-medium disabled:opacity-50',
              i === 0 ? 'shrink-0 bg-fg text-bg hover:opacity-90' : /^no\b/i.test(o.label) ? 'border border-line-strong text-danger hover:bg-danger-soft' : 'border border-line-strong hover:bg-hover',
            )}
          >
            {busy === o.n ? 'Sending…' : o.label}
          </button>
        ))
      ) : (
        <button type="button" data-answer onClick={() => nav(paneHref(r.host.config.id, r.pane.id))} className="press h-8 shrink-0 rounded-full bg-fg px-3.5 text-[13px] font-medium text-bg hover:opacity-90">
          Answer
        </button>
      )}
    </span>
  )
}

/**
 * An agent session as a card. The whole card opens it; ⋯ (shown on hover with a mouse, always on a
 * touch screen) and a long-press offer the actions that don't need the session open. A session
 * that asks something shows its answers in place and keeps its size, so nothing around it moves.
 */
export const SessionCard = memo(function SessionCard({ r, showHost = true }: { r: PaneRef; showHost?: boolean }) {
  const a = r.pane.agent!
  const st = a.status
  const pct = st?.contextPct
  const nav = useNavigate()
  const href = paneHref(r.host.config.id, r.pane.id)
  const menu = `card:${r.host.config.id}:${r.pane.id}`
  const press = useLongPress(() => openMenu(menu))
  const context = contextMenu(() => paneMenu(r, nav))
  // pinned, parked or snoozed, tags and a note: the hub's, the same on every device
  const mark = useStore((s) => markFor(s.organize, r.host.config.id, r.pane.id))
  const snoozed = isSnoozed(mark) ? mark!.snoozedUntil! : undefined
  // a title given in Inyon takes the place of the session's own name
  const title = mark?.title ?? paneTitle(r)
  return (
    <div
      {...press}
      onContextMenu={(e) => {
        press.onContextMenu(e)
        context.onContextMenu(e)
      }}
      // a tap sinks the whole card a touch, as a button sinks under a press
      data-waiting={a.state === 'waiting' || undefined}
      className={clsx(
        'group relative flex flex-col gap-3 rounded-3xl border bg-elev p-4 shadow-card transition-[color,background-color,border-color,transform] [-webkit-touch-callout:none] active:scale-[0.99] sm:gap-4 sm:p-5',
        a.state === 'waiting' ? 'border-warn/35' : 'border-line hover:border-line-strong',
      )}>
      <Link to={href} aria-label={title} className="absolute inset-0 rounded-3xl" />
      <div className="pointer-events-none flex items-center gap-2.5">
        <AgentLogo kind={a.kind} size={26} active={isBusy(a)} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-fg-3">{a.custom ?? agentDef(a.kind).name}</span>
        {(mark?.pinned !== undefined || mark?.parked || mark?.note || snoozed) && (
          <span className="pointer-events-auto flex shrink-0 items-center gap-1.5 text-fg-3">
            {snoozed ? (
              <span className="flex items-center gap-1 font-mono text-[12px]" title="Snoozed">
                <AlarmClock className="size-3.5" aria-label="Snoozed" /> {wakeText(snoozed)}
              </span>
            ) : mark?.parked ? (
              <span className="text-[12px]">Parked</span>
            ) : (
              mark?.pinned !== undefined && <Pin className="size-3.5" aria-label="Pinned" />
            )}
            {mark?.note && <StickyNote className="size-3.5" aria-label={`Note: ${mark.note}`}><title>{mark.note}</title></StickyNote>}
          </span>
        )}
        <StatePill state={shownState(a, r.host)} title={a.background} />
        <span className="pointer-events-auto relative -my-1 -mr-1.5">
          <Menu
            name={menu}
            align="end"
            width={220}
            items={paneMenu(r, nav)}
            trigger={({ toggle, ref, open }) => (
              <button
                ref={ref}
                type="button"
                onClick={toggle}
                aria-label={`Actions for ${title}`}
                className={clsx(
                  'flex size-8 items-center justify-center rounded-full text-fg-3 transition-opacity hover:bg-hover hover:text-fg focus-visible:opacity-100 pointer-coarse:opacity-100',
                  open ? 'opacity-100' : 'opacity-0 group-hover:opacity-100',
                )}
              >
                <MoreHorizontal className="size-4" />
              </button>
            )}
          />
        </span>
      </div>
      <div className="pointer-events-none min-w-0">
        <div className="truncate text-[17px] font-semibold tracking-[-0.03em]">{title}</div>
        <div className="mt-1 truncate font-mono text-[12px] text-fg-3">
          {[showHost && r.host.config.name, tmuxTarget(r), shortPath(r.pane.cwd, 34), mark?.tags?.map((t) => `#${t}`).join(' ')].filter(Boolean).join(' · ')}
        </div>
      </div>
      <div className="pointer-events-none truncate text-[13.5px] text-fg-2">
        {a.state === 'working' ? a.activity ?? 'Working…' : a.state === 'waiting' ? a.waitingFor : a.background ? backgroundText(a.background) : a.failure ? failureText(a.failure) : `Active ${timeAgo(r.window.activity * 1000)}`}
      </div>
      <div className="pointer-events-none mt-auto flex h-[calc(2rem+0.875rem+1px)] items-center gap-2 border-t border-line pt-3.5 font-mono text-[12px] text-fg-3">
        {a.state === 'waiting' && r.host.status === 'online' ? (
          <Answers r={r} />
        ) : (
          <>
        <span className="truncate">{[st?.model, st?.effort].filter(Boolean).join(' · ') || ' '}</span>
        {st?.costUsd !== undefined && <span className="tabular shrink-0">{money(st.costUsd)}</span>}
        {pct !== undefined && (
          <span className="pointer-events-auto ml-auto flex shrink-0 items-center gap-1.5" title={`${tokens(st?.contextUsed)} of ${tokens(st?.contextSize)} tokens`}>
            <span className="tabular">{Math.round(pct)}%</span>
            <Meter value={pct} tone="auto" className="w-12" />
          </span>
        )}
          </>
        )}
      </div>
    </div>
  )
}, samePane)

type Windows = { fiveHour?: RateWindow; sevenDay?: RateWindow; updatedAt: number }

/**
 * Plan limits per machine and agent, from the freshest reading there: a session's status, or the
 * account itself, asked with no session running.
 */
export function LimitsCard({ hosts }: { hosts: HostState[] }) {
  const rows = hosts.flatMap((h) => {
    const best = new Map<string, Windows>()
    const offer = (k: string, st: Windows | undefined) => {
      if (!st || !(st.fiveHour || st.sevenDay)) return
      if (!best.has(k) || best.get(k)!.updatedAt < st.updatedAt) best.set(k, st)
    }
    for (const s of h.sessions) for (const w of s.windows) for (const p of w.panes) if (p.agent) offer(p.agent.kind, p.agent.status)
    for (const l of h.limits ?? []) offer(l.kind, l)
    return [...best.entries()].map(([kind, st]) => ({ host: h, kind: kind as keyof typeof AGENTS, st, resets: h.limits?.find((l) => l.kind === kind)?.resets }))
  })
  if (!rows.length) return null
  return (
    <section className="rounded-3xl border border-line bg-elev shadow-card p-5">
      <h2 className="mb-4 flex items-center gap-2 text-[15px] font-semibold tracking-[-0.02em]">
        <Gauge className="size-4 text-fg-3" /> Plan limits
      </h2>
      <div className="space-y-4">
        {rows.map(({ host, kind, st, resets }) => (
          <div key={host.config.id + kind}>
            <div className="mb-2.5 flex items-center gap-2 text-[13px]">
              <AgentLogo kind={kind} size={16} />
              <span className="font-medium">{host.config.name}</span>
              {resets ? (
                <span className="ml-auto font-mono text-[12px] text-fg-3" title="Limit resets this account has banked. Inyon never uses them.">
                  {resets} {resets === 1 ? 'reset' : 'resets'} banked
                </span>
              ) : null}
            </div>
            {([
              ['5-hour', liveWindow(st.fiveHour)],
              ['Weekly', liveWindow(st.sevenDay)],
            ] as const).map(([label, w]) =>
              w ? (
                <div key={label} className="mb-2">
                  <div className="mb-1.5 flex items-baseline text-[12.5px]">
                    <span className="text-fg-2">{label}</span>
                    <span className="ml-2 font-mono text-[12px] text-fg-3">{w.reset ? 'reset' : resetsIn(w.resetsAt)}</span>
                    <span className="tabular ml-auto font-mono text-[12px] font-medium">{Math.round(w.pct)}%</span>
                  </div>
                  <Meter value={w.pct} tone="auto" />
                </div>
              ) : null,
            )}
          </div>
        ))}
      </div>
    </section>
  )
}

/**
 * The coding CLIs on a machine: in colour when one is running there now, grey when it's only
 * installed. Hover says which is which.
 */
export function AgentBadges({ host, size = 18 }: { host: HostState; size?: number }) {
  if (!host.agents.length) return null
  const running = new Map<string, number>()
  for (const s of host.sessions) for (const w of s.windows) for (const p of w.panes) if (p.agent) running.set(p.agent.kind, (running.get(p.agent.kind) ?? 0) + 1)
  return (
    <span className="flex shrink-0 items-center gap-1.5 pt-1 pr-1">
      {host.agents.map((k) => {
        const n = running.get(k) ?? 0
        return (
          <span key={k} title={n ? `${agentDef(k).name}: ${n} running` : `${agentDef(k).name}: installed, not running`} className={clsx('flex items-center gap-1', !n && 'opacity-35 grayscale')}>
            <AgentLogo kind={k} size={size} />
            {n > 1 && <span className="tabular font-mono text-[12px] text-fg-3">{n}</span>}
          </span>
        )
      })}
    </span>
  )
}

export function MachinesTable({ hosts }: { hosts: HostState[] }) {
  const nav = useNavigate()
  // the table needs about 560px; the list takes over whenever the column is narrower than that
  // (a phone, or a tablet held upright with the sidebar open), so nothing scrolls sideways
  return (
    <div className="@container">
    <div className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card @[600px]:hidden">
      {hosts.map((h) => {
        const panes = h.sessions.reduce((n, s) => n + s.windows.reduce((m, w) => m + w.panes.length, 0), 0)
        return (
          <Link key={h.config.id} to={`/machines/${encodeURIComponent(h.config.id)}`} {...contextMenu(() => machineMenu(h, nav), { touch: true })} className={clsx('flex items-center gap-3 px-4 py-3 transition-[opacity,background-color] active:bg-hover', h.status !== 'online' && 'opacity-60')}>
            <HostDot status={h.status} />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-[14px] font-medium">{h.config.name}</span>
                <AgentBadges host={h} size={15} />
              </div>
              <div className="truncate font-mono text-[12px] text-fg-3">
                {/* the response time is on the machine's page; here it would push the counts off a phone */}
                {hostLine(h, h.tmux === 'missing' ? 'tmux not installed' : `${plural(h.sessions.length, 'session')} · ${plural(panes, 'pane')}`)}
              </div>
            </div>
            <ChevronRight className="size-4 shrink-0 text-fg-3" />
          </Link>
        )
      })}
    </div>
    <div className="hidden overflow-hidden rounded-3xl border border-line bg-elev shadow-card @[600px]:block">
      <div className="overflow-x-auto">
        <table className="w-full min-w-[560px] border-collapse text-[13px]">
          <thead>
            <tr className="text-left text-[12px] text-fg-3">
              <th className="border-b border-line px-5 py-3 font-medium">Machine</th>
              <th className="border-b border-line px-5 py-3 font-medium" title="In colour: running now. Grey: installed.">Agents</th>
              <th className="border-b border-line px-5 py-3 font-medium">tmux</th>
              <th className="border-b border-line px-5 py-3 text-right font-medium" title={RESPONSE_TITLE}>Response</th>
            </tr>
          </thead>
          <tbody>
            {hosts.map((h) => {
              const panes = h.sessions.reduce((n, s) => n + s.windows.reduce((m, w) => m + w.panes.length, 0), 0)
              return (
                <tr key={h.config.id} {...contextMenu(() => machineMenu(h, nav))} className={clsx('transition-[opacity,background-color] hover:bg-hover/50', h.status !== 'online' && 'opacity-60')}>
                  <td className="border-b border-line/60 px-5 py-3.5">
                    <Link to={`/machines/${encodeURIComponent(h.config.id)}`} className="flex items-center gap-2.5">
                      <HostDot status={h.status} />
                      <div className="min-w-0">
                        <div className="font-medium">{h.config.name}</div>
                        <div className="truncate font-mono text-[12px] text-fg-3">
                          {h.status === 'error' ? h.error : h.config.kind === 'local' ? `This hub${h.os ? ` · ${h.os}` : ''}` : sshAddress(h.config, hosts)}
                        </div>
                      </div>
                    </Link>
                  </td>
                  <td className="border-b border-line/60 px-5 py-3.5">
                    <div className="flex items-center gap-1">
                      {h.agents.length ? <AgentBadges host={h} /> : <span className="text-fg-3">—</span>}
                    </div>
                  </td>
                  <td className="border-b border-line/60 px-5 py-3.5 text-fg-2">
                    {h.status !== 'online' ? (h.lastSeen ? 'Offline' : '—') : h.tmux === 'missing' ? 'Not installed' : `${plural(h.sessions.length, 'session')} · ${plural(panes, 'pane')}`}
                  </td>
                  <td className="tabular border-b border-line/60 px-5 py-3.5 text-right font-mono text-[12px] text-fg-2" title={h.latencyMs !== undefined ? RESPONSE_TITLE : undefined}>{h.latencyMs !== undefined ? responseTime(h.latencyMs) : '—'}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </div>
    </div>
  )
}

/**
 * What a "needs you" entry says now. Only the latest question of a session still waiting needs
 * you; an earlier one, or one the session has moved on from, was answered (from here, from an
 * alert or at the keyboard), and one whose session has ended was just asked.
 */
export function activityLabel(e: ActivityEvent, activity: ActivityEvent[], hosts: HostState[]) {
  if (e.kind === 'done') return 'finished'
  const pane = hosts.find((h) => h.config.id === e.host)?.sessions.flatMap((s) => s.windows.flatMap((w) => w.panes)).find((p) => p.id === e.pane)
  if (!pane?.agent) return 'asked'
  const latest = activity.find((a) => a.host === e.host && a.pane === e.pane && a.kind === 'waiting')
  return pane.agent.state === 'waiting' && latest === e ? 'needs you' : 'answered'
}

/** How many events the feed shows. */
const FEED_ROWS = 8

/**
 * The latest events, newest first. While the pointer or focus is on it the list holds still, and
 * what arrived meanwhile waits behind a "new" button, so a click lands on the row it aimed at.
 */
export function ActivityFeed() {
  const live = useStore((s) => s.activity)
  const hosts = useStore((s) => s.hosts)
  const [held, setHeld] = useState<ActivityEvent[] | null>(null)
  const activity = held ?? live
  const fresh = held ? live.filter((e) => e.at > (held[0]?.at ?? 0)).length : 0
  const hold = () => setHeld((h) => h ?? live)
  // let go once neither the pointer nor the focus is on it
  const leave = (e: PointerEvent<HTMLElement>) => !e.currentTarget.contains(document.activeElement) && setHeld(null)
  const blur = (e: FocusEvent<HTMLElement>) => !e.currentTarget.contains(e.relatedTarget as Node | null) && !e.currentTarget.matches(':hover') && setHeld(null)
  if (!live.length) return null
  return (
    <section onPointerEnter={hold} onPointerLeave={leave} onFocus={hold} onBlur={blur} className="rounded-3xl border border-line bg-elev shadow-card p-5">
      <h2 className="mb-4 flex h-6 items-center gap-2 text-[15px] font-semibold tracking-[-0.02em]">
        Recent activity
        {fresh > 0 && (
          <button type="button" onClick={() => setHeld(live)} className="animate-fade-in ml-auto rounded-full border border-line px-2.5 font-mono text-[12px] leading-6 font-normal text-fg-2 hover:bg-hover">
            {fresh} new
          </button>
        )}
      </h2>
      <div className="space-y-3">
        {activity.slice(0, FEED_ROWS).map((e) => (
          <Link key={e.id} to={paneHref(e.host, e.pane)} className="flex h-[2.4rem] gap-2.5">
            <AgentLogo kind={e.agent as any} size={18} />
            <div className="min-w-0">
              <div className="truncate text-[13px]">
                <span className="font-medium">{e.title}</span> <span className="text-fg-2">· {activityLabel(e, activity, hosts)}</span>
              </div>
              <div className="font-mono text-[12px] text-fg-3">{timeAgo(e.at)}</div>
            </div>
          </Link>
        ))}
      </div>
    </section>
  )
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`
