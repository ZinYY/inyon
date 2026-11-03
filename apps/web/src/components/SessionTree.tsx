import { memo, useMemo, useState, type ReactNode } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { ChevronRight, SquareTerminal } from 'lucide-react'
import { isBusy, type HostState } from '@inyon/protocol'
import { paneHref, paneTitle, terminalName, type PaneRef } from '../lib/overview.ts'
import { groupPanes, loadFolded, saveFolded, tmuxTarget } from '../lib/groups.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { StateDot, shownState } from './ui/status.tsx'
import { MachineIcon, machineKindOf } from './ui/MachineIcon.tsx'
import { FilterInput } from './ui/FilterInput.tsx'
import { contextMenu, type MenuItems } from './ui/ContextMenu.tsx'
import { machineMenu, paneMenu, tmuxSessionMenu } from './menus.tsx'

const notCompanion = (r: PaneRef) => !r.pane.companion

/** Past this many panes the tree gets a search box. */
export const TREE_SEARCH_FROM = 8

/**
 * Every pane in the sidebar, grouped as tmux holds them: machine, then tmux session, then the
 * session's windows in order. Groups fold; a search looks through all of them and opens what matches.
 * With one machine its level is left out.
 */
export function SessionTree({ hosts }: { hosts: HostState[] }) {
  const loc = useLocation()
  const nav = useNavigate()
  const [q, setQ] = useState('')
  const [folded, setFolded] = useState(loadFolded)
  // the companion has its own place under Overview
  const all = useMemo(() => groupPanes(hosts, { keep: notCompanion }), [hosts])
  const groups = useMemo(() => (q ? groupPanes(hosts, { query: q, keep: notCompanion }) : all), [hosts, q, all])
  const total = all.reduce((n, g) => n + g.panes, 0)
  if (!total) return null
  const oneHost = hosts.length <= 1
  const open = (key: string) => !!q || !folded.has(key)
  const toggle = (key: string) =>
    setFolded((f) => {
      const n = new Set(f)
      if (n.has(key)) n.delete(key)
      else n.add(key)
      saveFolded(n)
      return n
    })

  return (
    <div className="pt-5">
      <div className="flex items-center px-3 pb-2 text-[12px] font-medium text-fg-3">
        <span className="flex-1">Sessions</span>
        <span className="tabular font-mono">{total}</span>
      </div>
      {total > TREE_SEARCH_FROM && <FilterInput value={q} onChange={setQ} placeholder="Filter sessions" className="mb-2" />}
      {groups.map((g) => {
        const sessions = g.sessions.map((s) => (
          <Group
            key={s.key}
            open={open(`s:${s.key}`)}
            onToggle={() => toggle(`s:${s.key}`)}
            depth={oneHost ? 0 : 1}
            menu={() => tmuxSessionMenu(s.panes[0])}
            label={<span className="truncate font-mono text-[12.5px]">{s.session.name}</span>}
            count={s.panes.length}
            waiting={s.waiting}
          >
            {s.panes.map((r) => (
              <TreeRow key={r.pane.id} r={r} oneHost={oneHost} active={loc.pathname === paneHref(r.host.config.id, r.pane.id)} />
            ))}
          </Group>
        ))
        if (oneHost) return sessions
        return (
          <Group
            key={g.key}
            open={open(`h:${g.key}`)}
            onToggle={() => toggle(`h:${g.key}`)}
            depth={0}
            menu={() => machineMenu(g.host, nav)}
            label={
              <>
                <MachineIcon kind={machineKindOf(g.host)} size={15} className={clsx('shrink-0', g.host.status === 'online' ? 'text-fg-2' : 'text-fg-3')} />
                <span className={clsx('truncate font-medium', g.host.status !== 'online' && 'text-fg-3')}>{g.host.config.name}</span>
              </>
            }
            count={g.panes}
            waiting={g.waiting}
          >
            {sessions}
          </Group>
        )
      })}
      {q && !groups.length && <p className="px-3 py-4 text-[13px] text-fg-3">Nothing matches</p>}
    </div>
  )
}

/** One pane in the tree. Memoised on its pane, so an update elsewhere leaves it alone. */
const TreeRow = memo(
  function TreeRow({ r, oneHost, active }: { r: PaneRef; oneHost: boolean; active: boolean }) {
    const href = paneHref(r.host.config.id, r.pane.id)
    const nav = useNavigate()
    return (
      <Link
        to={href}
        {...contextMenu(() => paneMenu(r, nav), { touch: true })}
        title={`${r.host.config.name} · ${tmuxTarget(r)}`}
        className={clsx(
          'relative flex h-8 items-center gap-2 rounded-full pr-3 text-[13px] transition-colors',
          oneHost ? 'pl-7' : 'pl-10',
          active ? 'bg-elev font-medium text-fg shadow-card ring-1 ring-line' : 'text-fg-2 hover:bg-hover/70 hover:text-fg',
        )}
      >
        {r.pane.agent ? <AgentLogo kind={r.pane.agent.kind} size={16} active={isBusy(r.pane.agent)} /> : <SquareTerminal className="size-4 shrink-0 text-fg-3" />}
        <span className="min-w-0 flex-1 truncate">{r.pane.agent ? paneTitle(r) : (terminalName(r) ?? r.pane.command)}</span>
        <span className="shrink-0 font-mono text-[12px] text-fg-3">:{r.window.index}</span>
        {r.pane.agent && <StateDot state={shownState(r.pane.agent, r.host)} />}
      </Link>
    )
  },
  (a, b) =>
    a.active === b.active && a.oneHost === b.oneHost && a.r.pane === b.r.pane && a.r.window === b.r.window && a.r.session === b.r.session &&
    a.r.host.config === b.r.host.config && a.r.host.status === b.r.host.status && a.r.host.hostname === b.r.host.hostname,
)

/** A foldable heading and what's under it. Folded, it still shows that something waits on you. */
function Group({ open, onToggle, depth, label, count, waiting, menu, children }: { open: boolean; onToggle: () => void; depth: 0 | 1; label: ReactNode; count: number; waiting: number; menu?: () => MenuItems; children: ReactNode }) {
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        {...(menu && contextMenu(menu, { touch: true }))}
        aria-expanded={open}
        className={clsx('flex h-8 w-full items-center gap-1.5 rounded-full pr-3 text-left text-[13px] text-fg-2 hover:bg-hover/70 hover:text-fg', depth ? 'pl-5' : 'pl-2')}
      >
        <ChevronRight className={clsx('size-3.5 shrink-0 text-fg-3 transition-transform', open && 'rotate-90')} />
        <span className="flex min-w-0 flex-1 items-center gap-2">{label}</span>
        {waiting > 0 && !open && <span className="size-1.5 shrink-0 rounded-full bg-warn" aria-label={`${waiting} waiting for you`} />}
        <span className="tabular shrink-0 font-mono text-[12px] text-fg-3">{count}</span>
      </button>
      <div className="expand" data-open={open}>
        <div inert={!open}>{children}</div>
      </div>
    </div>
  )
}
