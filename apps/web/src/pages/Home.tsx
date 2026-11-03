import { useEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import { Link } from 'react-router-dom'
import { ChevronRight, Pin, Plug, Plus, Search, WifiOff } from 'lucide-react'
import { useStore } from '../lib/store.ts'
import { agentPanes, allPanes, type PaneRef } from '../lib/overview.ts'
import { nextWake, sortPanes } from '../lib/organize.ts'
import { ActivityFeed, LimitsCard, MachinesTable, SessionCard } from '../components/Overview.tsx'
import { Logo } from '../components/AppShell.tsx'
import { Button, Empty, Segmented, SectionLabel } from '../components/ui/primitives.tsx'
import { api } from '../lib/api.ts'
import { INTEGRATION_TEXT } from '../components/session/IntegrationNote.tsx'
import { toast } from '../lib/store.ts'
import { isBusy, type AgentPaneInfo } from '@inyon/protocol'
import { MachineSheet } from '../components/Sheets.tsx'
import { isPhone } from '../lib/phone.ts'
import { isHosted } from '../lib/base.ts'
import { InstallCard } from '../components/InstallCard.tsx'
import { PullRefresh } from '../components/ui/PullRefresh.tsx'
import { refreshAll } from '../lib/refresh.ts'
import { groupPanes, loadFolded, saveFolded } from '../lib/groups.ts'
import { FilterInput } from '../components/ui/FilterInput.tsx'
import { AnimatedList } from '../components/ui/AnimatedList.tsx'
import { PhoneSheet } from '../components/PhoneSheet.tsx'
import { Smartphone } from 'lucide-react'
import { CompanionBar } from '../components/CompanionAgent.tsx'
import { MachineIcon, machineKindOf } from '../components/ui/MachineIcon.tsx'
import { openConnectHelp } from '../components/ConnectSheet.tsx'

const GRID = 'grid grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))] sm:gap-4'
const paneKey = (r: PaneRef) => r.host.config.id + r.pane.id
/** the parked and snoozed start folded: this is kept with the folded groups, set when it's open */
const PARKED_OPEN = 'parked:open'

/** Past this many agent sessions the overview gets a search box. */
const SEARCH_FROM = 6

/** working, or idle with shells or tasks still running */
const busy = (a: AgentPaneInfo) => isBusy(a)

export function Home() {
  const hosts = useStore((s) => s.hosts)
  const loaded = useStore((s) => s.hostsLoaded)
  const connected = useStore((s) => s.connected)
  const [filter, setFilter] = useState<'all' | 'working' | 'idle'>('all')
  const [adding, setAdding] = useState(false)
  const [reloading, setReloading] = useState(false)
  const reload = () => {
    setReloading(true)
    // long enough to see the turn, however fast the hub answers
    Promise.allSettled([refreshAll(), new Promise((r) => setTimeout(r, 600))]).then(() => setReloading(false))
  }
  // the companion has its own line above the sessions
  const panes = agentPanes(hosts).filter((r) => !r.pane.companion)
  const waiting = panes.filter((r) => r.pane.agent!.state === 'waiting')
  const [q, setQ] = useState('')
  const [folded, setFolded] = useState(loadFolded)
  const homeFolded = useMemo(() => new Set([...folded].filter((k) => k.startsWith('home:')).map((k) => k.slice(5))), [folded])
  const toggle = (key: string) =>
    setFolded((f) => {
      const n = new Set(f)
      const k = `home:${key}`
      if (n.has(k)) n.delete(k)
      else n.add(k)
      saveFolded(n)
      return n
    })
  // the pinned first (from 0.24, kept by the hub so every device has the same), then every other
  // agent session grouped by machine, each in a place that doesn't change, then the parked and
  // snoozed, folded away. One that asks something answers from its own card. A search looks
  // through every machine, tags and notes included.
  const organize = useStore((s) => s.organize)
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    // a snooze that ends brings its session back right then
    const at = nextWake(organize, now)
    if (at === undefined) return
    const t = setTimeout(() => setNow(Date.now()), Math.min(at - Date.now() + 250, 2 ** 31 - 1))
    return () => clearTimeout(t)
  }, [organize, now])
  const sorted = useMemo(
    () => sortPanes(allPanes(hosts).filter((r) => !!r.pane.agent && !r.pane.companion && (filter === 'all' || (filter === 'working' ? busy(r.pane.agent) : !busy(r.pane.agent)))), organize, { now, query: q }),
    [hosts, q, filter, organize, now],
  )
  const groups = useMemo(() => {
    const rest = new Set(sorted.sessions.map(paneKey))
    return groupPanes(hosts, { keep: (r) => rest.has(paneKey(r)) })
  }, [hosts, sorted])
  const shownAny = groups.length + sorted.pinned.length + sorted.folded.length > 0
  const parkedOpen = !!q || homeFolded.has(PARKED_OPEN)
  // Show finds a session that needs you even in the folded parked and snoozed: it opens them
  // first, then brings the card into view once it's on screen
  const showAfterOpen = useRef(false)
  const show = () => {
    if (showNext() || parkedOpen || !sorted.folded.some((r) => r.pane.agent?.state === 'waiting')) return
    showAfterOpen.current = true
    toggle(PARKED_OPEN)
  }
  useEffect(() => {
    if (!parkedOpen || !showAfterOpen.current) return
    showAfterOpen.current = false
    showNext()
  }, [parkedOpen])
  const working = panes.filter((r) => busy(r.pane.agent!)).length
  // the status line tells the two apart: a turn in progress, and work left running behind an idle prompt
  const inFront = panes.filter((r) => r.pane.agent!.state === 'working').length
  const behind = working - inFront
  // a card being turned on stays, with its spinner, until the machine answers: the hub may report
  // the new state before the request returns, and a card that just vanished says nothing
  // (with the state it had when tapped, so its text doesn't change under the spinner)
  const [turning, setTurning] = useState<ReadonlyMap<string, string>>(() => new Map())
  const needsIntegration = hosts.filter(
    (h) => turning.has(h.config.id) || (h.status === 'online' && (h.integration === 'missing' || h.integration === 'removed' || h.integration === 'outdated') && h.agents.includes('claude')),
  )
  const panesTotal = hosts.reduce((n, h) => n + h.sessions.reduce((m, s) => m + s.windows.reduce((k, w) => k + w.panes.length, 0), 0), 0)

  const enable = async (id: string, state: string) => {
    const update = state === 'outdated'
    setTurning((m) => new Map(m).set(id, state))
    try {
      await api.integration(id, true)
      toast({ title: update ? 'Claude integration updated' : 'Claude integration on', body: 'Restart running Claude sessions to get live limits and exact matching.', tone: 'success' })
    } catch (e) {
      toast({ title: update ? 'Could not update the integration' : 'Could not turn on the integration', body: (e as Error).message, tone: 'error' })
    } finally {
      setTurning((m) => {
        const n = new Map(m)
        n.delete(id)
        return n
      })
    }
  }

  return (
    <PullRefresh onRefresh={refreshAll} className="min-h-0 flex-1 overflow-y-auto">
      <header className="pt-safe sticky top-0 z-20 border-b border-line bg-bg">
        <div className="flex h-[52px] items-center gap-3 px-4 lg:px-8">
          <span className="md:hidden">
            <Logo target />
          </span>
          <span className="font-mono text-[12px] text-fg-3 max-md:hidden">
            {hosts.length} {isPhone() ? 'computer' : 'machine'}{hosts.length === 1 ? '' : 's'} · {panes.length} agent session{panes.length === 1 ? '' : 's'} · {panesTotal} pane{panesTotal === 1 ? '' : 's'}
          </span>
          {!connected && (
            // the hub this page came from: tapping it asks what's wrong
            <button type="button" onClick={() => !isPhone() && openConnectHelp('self')} className="ml-auto flex h-8 items-center gap-1.5 rounded-full px-2 text-[12px] text-warn hover:bg-hover" title={isPhone() ? undefined : "Can't connect?"}>
              <WifiOff className="size-3.5" /> Reconnecting
            </button>
          )}
          {/* with a mouse there is no pull to refresh: the mark's line reels here instead */}
          <button
            type="button"
            onClick={reload}
            aria-label="Refresh"
            title="Refresh"
            className={clsx('flex size-8 items-center justify-center rounded-full text-fg-2 hover:bg-hover max-md:hidden', connected && 'ml-auto')}
          >
            <span className={clsx('flex', reloading && 'mark-reeling')}>
              <Logo className="size-[18px]" />
            </span>
          </button>
          <button
            type="button"
            onClick={() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', metaKey: true }))}
            aria-label="Search"
            className="ml-auto flex size-8 items-center justify-center rounded-full text-fg-2 hover:bg-hover relative after:absolute after:-inset-1 md:hidden"
          >
            <Search className="size-[18px]" />
          </button>
        </div>
      </header>

      <div className="mx-auto flex w-full max-w-[1280px] flex-wrap items-start gap-8 px-4 pt-4 pb-16 md:pt-8 lg:px-8 lg:pt-12">
        <div className="flex min-w-0 flex-[999_1_560px] flex-col gap-5 md:gap-9">
          {!loaded ? (
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className="skeleton h-[170px] rounded-3xl" />
              ))}
            </div>
          ) : !hosts.length ? (
            <div className="rounded-3xl border border-dashed border-line-strong">
              <Empty
                icon={<Logo className="size-7 mark-drawn" />}
                title={isPhone() ? 'Connect your computer' : 'No machines yet'}
                action={
                  <Button variant="primary" onClick={() => setAdding(true)}>
                    <Plus className="size-3.5" /> {isPhone() ? 'Add a computer' : 'Add a machine'}
                  </Button>
                }
              >
                {isHosted()
                  ? 'See the coding agents on your laptop or server and talk to them from here. Run one command on it, then scan the code it shows with your phone\u2019s camera, or type it here.'
                  : isPhone()
                  ? 'See the coding agents on your laptop or server and talk to them from here. Run one command on it, then scan the code it shows.'
                  : 'Pair a machine or add one over SSH to see the coding agents running in tmux on it.'}
              </Empty>
            </div>
          ) : (
            <>
              {isHosted() && <InstallCard />}
              <div>
              <h1 className="text-[1.75rem] leading-[0.95] font-semibold tracking-[-0.04em] md:text-[44px] md:tracking-[-0.045em]">Overview</h1>
              <p className="mt-1.5 flex items-center gap-2 text-[14px] text-fg-2 md:mt-3 md:text-[15px]">
                <span className="min-w-0 truncate">
                {waiting.length
                  ? `${waiting.length} session${waiting.length > 1 ? 's need' : ' needs'} you.`
                  : inFront && behind
                    ? `${inFront} working, ${behind} in the background.`
                    : inFront
                      ? `${inFront} session${inFront > 1 ? 's are' : ' is'} working.`
                    : behind
                      ? `${behind} session${behind > 1 ? 's have' : ' has'} work running in the background.`
                    : panes.length
                      ? 'All quiet. Every session is idle.'
                      : 'No agents are running right now.'}
                </span>
                {waiting.length > 0 && (
                  <button type="button" onClick={show} className="-my-1 shrink-0 rounded-full px-2 py-1 text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg">
                    Show
                  </button>
                )}
              </p>
              </div>

              <CompanionBar />

              {!isPhone() && hosts.length > 0 && <PhoneCard />}

              {needsIntegration.map((h) => {
                const busy = turning.get(h.config.id)
                const state = busy ?? h.integration
                return (
                  <div key={h.config.id} className="flex items-center gap-3 rounded-3xl border border-line bg-elev px-5 py-3.5 shadow-card">
                    <Plug className="size-4 shrink-0 text-fg-3" />
                    <p className="min-w-0 flex-1 text-[13px] text-fg-2">
                      <span className="font-medium text-fg">{h.config.name}:</span>{' '}
                      {state === 'missing' ? 'turn on the integration for live usage limits, context and cost, instant updates and exact session matching.' : INTEGRATION_TEXT[state]}
                    </p>
                    <Button size="sm" variant="primary" loading={!!busy} onClick={() => enable(h.config.id, state)}>
                      {state === 'outdated' ? (busy ? 'Updating' : 'Update') : busy ? 'Turning on' : 'Turn on'}
                    </Button>
                  </div>
                )
              })}

              <section>
                <SectionLabel
                  right={
                    panes.length > 3 && (
                      <Segmented
                        size="sm"
                        value={filter}
                        onChange={setFilter}
                        options={[
                          { v: 'all', label: 'All' },
                          { v: 'working', label: 'Working' },
                          { v: 'idle', label: 'Idle' },
                        ]}
                      />
                    )
                  }
                >
                  Agent sessions
                </SectionLabel>
                {panes.length > SEARCH_FROM && <FilterInput value={q} onChange={setQ} placeholder={`Search ${panes.length} sessions`} className="mb-3" />}
                {sorted.pinned.length > 0 && (
                  <div className={clsx(groups.length && 'mb-5')}>
                    <div className="mb-2 flex h-6 items-center gap-2 text-[13px] font-medium text-fg-2">
                      <Pin className="size-3.5 text-fg-3" /> Pinned
                    </div>
                    <AnimatedList className={GRID} items={sorted.pinned.map((r) => ({ key: paneKey(r), node: <SessionCard r={r} showHost={hosts.length > 1} /> }))} />
                  </div>
                )}
                {!shownAny ? (
                  <div className="rounded-3xl border border-dashed border-line-strong">
                    <Empty title={panes.length ? 'Nothing here' : 'No agent sessions'}>
                      {panes.length ? (q ? `No session matches “${q}”.` : 'Try another filter.') : `Start Claude Code, Codex or another agent in tmux on any ${isPhone() ? 'computer' : 'machine'}, or ${isPhone() ? 'tap + ' : 'press N '}to start one from here.`}
                    </Empty>
                  </div>
                ) : !groups.length ? null : (
                  hosts.length > 1 ? (
                    <div className="flex flex-col gap-2">
                      {groups.map((g) => {
                        const open = !!q || !homeFolded.has(g.key)
                        return (
                          <div key={g.key}>
                            <button type="button" onClick={() => toggle(g.key)} aria-expanded={open} className="-mx-2 flex h-10 w-[calc(100%+1rem)] items-center gap-2 rounded-full px-2 text-left hover:bg-hover/60">
                              <ChevronRight className={clsx('size-4 shrink-0 text-fg-3 transition-transform', open && 'rotate-90')} />
                              <MachineIcon kind={machineKindOf(g.host)} size={16} className={clsx('shrink-0', g.host.status === 'online' ? 'text-fg-2' : 'text-fg-3')} />
                              <span className={clsx('truncate text-[14px] font-medium', g.host.status !== 'online' && 'text-fg-2')}>{g.host.config.name}</span>
                              {g.host.status !== 'online' && <span className="shrink-0 text-[12px] text-fg-3">{g.host.status === 'error' ? 'Offline' : 'Connecting'}</span>}
                              <span className="tabular ml-auto shrink-0 font-mono text-[12px] text-fg-3">{g.panes}</span>
                            </button>
                            <div className="expand" data-open={open}>
                              <div inert={!open}>
                                <AnimatedList
                                  className="grid grid-cols-1 gap-3 pt-2 pb-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))] sm:gap-4"
                                  items={g.sessions.flatMap((sg) => sg.panes).map((r) => ({ key: paneKey(r), node: <SessionCard r={r} showHost={false} /> }))}
                                />
                              </div>
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  ) : (
                    <AnimatedList className={GRID} items={groups.flatMap((g) => g.sessions.flatMap((sg) => sg.panes)).map((r) => ({ key: paneKey(r), node: <SessionCard r={r} /> }))} />
                  )
                )}
                {sorted.folded.length > 0 && (
                  <div className="mt-3">
                    <button type="button" onClick={() => toggle(PARKED_OPEN)} aria-expanded={parkedOpen} className="-mx-2 flex h-10 w-[calc(100%+1rem)] items-center gap-2 rounded-full px-2 text-left hover:bg-hover/60">
                      <ChevronRight className={clsx('size-4 shrink-0 text-fg-3 transition-transform', parkedOpen && 'rotate-90')} />
                      <span className="truncate text-[14px] font-medium text-fg-2">Parked and snoozed</span>
                      <span className="tabular ml-auto shrink-0 font-mono text-[12px] text-fg-3">{sorted.folded.length}</span>
                    </button>
                    <div className="expand" data-open={parkedOpen}>
                      <div inert={!parkedOpen}>
                        <AnimatedList className={clsx(GRID, 'pt-2 pb-3')} items={sorted.folded.map((r) => ({ key: paneKey(r), node: <SessionCard r={r} showHost={hosts.length > 1} /> }))} />
                      </div>
                    </div>
                  </div>
                )}
              </section>

              <section>
                <SectionLabel right={<Link to="/machines" className="-my-2 px-1 py-2 text-[13px] text-fg-2 hover:text-fg">Manage →</Link>}>{isPhone() ? 'Computers' : 'Machines'}</SectionLabel>
                <MachinesTable hosts={hosts} />
              </section>
            </>
          )}
        </div>
        <MachineSheet open={adding} onClose={() => setAdding(false)} />
        <aside className="flex w-full flex-[1_1_280px] flex-col gap-4 lg:max-w-[340px] lg:pt-[92px]">
          <LimitsCard hosts={hosts} />
          <ActivityFeed />
        </aside>
      </div>
    </PullRefresh>
  )
}

/** Brings the next session that needs you into view, in the order they're shown; each press moves on. */
let shown = -1
function showNext() {
  // a card in a folded machine group isn't on screen
  const cards = [...document.querySelectorAll<HTMLElement>('[data-waiting]')].filter((c) => !c.closest('[inert]'))
  if (!cards.length) return false
  shown = (shown + 1) % cards.length
  const card = cards[shown]!
  card.scrollIntoView?.({ block: 'center', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
  card.querySelector<HTMLElement>('[data-answer]')?.focus({ preventScroll: true })
  return true
}

const PHONE_CARD_KEY = 'inyon.phoneCard.dismissed'

/** Until a phone is paired (or this is dismissed): the hub's one-line invitation to open it on a phone. */
function PhoneCard() {
  const [show, setShow] = useState(false)
  const [open, setOpen] = useState(false)
  useEffect(() => {
    try {
      if (localStorage.getItem(PHONE_CARD_KEY)) return
    } catch {}
    api.phones().then((phones) => setShow(Array.isArray(phones) && !phones.length), () => {})
  }, [])
  const dismiss = () => {
    try {
      localStorage.setItem(PHONE_CARD_KEY, '1')
    } catch {}
    setShow(false)
  }
  if (!show) return null
  return (
    <>
      <div className="flex items-center gap-3 rounded-3xl border border-line bg-elev px-5 py-3.5 shadow-card">
        <Smartphone className="size-4 shrink-0 text-fg-3" />
        <p className="min-w-0 flex-1 text-[13px] text-fg-2">
          <span className="font-medium text-fg">Open this hub on your phone.</span> The Inyon app scans a code and reaches every machine here itself.
        </p>
        <Button size="sm" variant="ghost" onClick={dismiss}>Later</Button>
        <Button size="sm" variant="primary" onClick={() => setOpen(true)}>Show the code</Button>
      </div>
      <PhoneSheet open={open} onClose={() => (setOpen(false), api.phones().then((p) => Array.isArray(p) && p.length && dismiss(), () => {}))} />
    </>
  )
}
