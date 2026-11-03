import { Suspense, useEffect, useMemo, useRef, useState } from 'react'
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { LayoutGrid, Monitor, PanelsTopLeft, Plus, Search, Settings, WifiOff } from 'lucide-react'
import { useStore } from '../lib/store.ts'
import type { HostState } from '@inyon/protocol'
import { agentPanes, allPanes, paneHref, paneTitle, terminalName } from '../lib/overview.ts'
import { SessionTree } from './SessionTree.tsx'
import { CompanionNavItem } from './CompanionAgent.tsx'
import { AgentLogo } from '../lib/skin.tsx'
import { MachineSheet, NewSessionSheet } from './Sheets.tsx'
import { isPhone } from '../lib/phone.ts'
import { keyLabel, shortcut } from '../lib/keys.ts'
import { usePageMenu } from './ui/ContextMenu.tsx'
import { ActionSheets } from './menus.tsx'
import { Waiting } from './ui/primitives.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'
import { isChanged, loadPages, usePages } from '../lib/pages.ts'
import { onHubs } from '../lib/hubs.ts'
import { PageSheets } from './pages/shared.tsx'
import { ConnectHelpHost, openConnectHelp } from './ConnectSheet.tsx'

// the phone app connects to servers itself; the hub talks about its machines
const machinesLabel = isPhone() ? 'Computers' : 'Machines'

/** Inyon's mark: a Korean shield kite on its line (same art as public/icon.svg). The tile lifts a step on graphite. */
export function Logo({ className, target }: { className?: string; target?: boolean }) {
  // while the hub is out of reach the line draws itself, over and over, until it is back
  const connecting = useStore((s) => s.authed === true && !s.connected)
  // the intro's mark flies onto a target and the target stays hidden until it lands
  return (
    <svg viewBox="0 0 64 64" className={clsx('shrink-0', connecting && 'mark-connecting', className ?? 'size-[26px]')} data-intro-target={target || undefined} aria-hidden>
      <path d={SQUIRCLE} fill="var(--logo-tile)" />
      <path d={SQUIRCLE} transform="translate(.5 .5) scale(.984)" fill="none" stroke="#fff" strokeOpacity=".08" />
      <g fill="#F1F0EC">
        <path transform="rotate(-12 41 23)" fillRule="evenodd" d="M33.5 9H48.5A3.5 3.5 0 0 1 52 12.5V33.5A3.5 3.5 0 0 1 48.5 37H33.5A3.5 3.5 0 0 1 30 33.5V12.5A3.5 3.5 0 0 1 33.5 9ZM41 18.4A4.6 4.6 0 1 0 41 27.6A4.6 4.6 0 1 0 41 18.4Z" />
        <path className="mark-line" d="M38.5 36.5C35 48 25 42 17.5 49.5" fill="none" stroke="#F1F0EC" strokeWidth="3.8" strokeLinecap="round" />
        <circle cx="15.5" cy="51" r="4" />
      </g>
    </svg>
  )
}
const SQUIRCLE = 'M25 0H39C60 0 64 4 64 25V39C64 60 60 64 39 64H25C4 64 0 60 0 39V25C0 4 4 0 25 0Z'

/** Quick switcher: type to jump to any session, terminal or machine. */
const NO_HOSTS: HostState[] = []
function QuickSwitch({ open, onClose }: { open: boolean; onClose: () => void }) {
  // it stays a moment after closing, fading out, as a sheet does
  const [shown, setShown] = useState(open)
  useEffect(() => {
    if (open) return setShown(true)
    const t = setTimeout(() => setShown(false), LEAVE_MS)
    return () => clearTimeout(t)
  }, [open])
  // closed, it has nothing to do with an overview update
  const hosts = useStore((s) => (shown ? s.hosts : NO_HOSTS))
  const nav = useNavigate()
  const [q, setQ] = useState('')
  const [i, setI] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  const items = useMemo(() => {
    const panes = allPanes(hosts).map((r) => ({
      key: r.host.config.id + r.pane.id,
      label: r.pane.agent ? paneTitle(r) : `${terminalName(r) ?? `${r.session.name}:${r.window.index}`} ${r.pane.command}`,
      sub: `${r.host.config.name} · ${r.pane.cwd.replace(/^\/home\/[^/]+/, '~')}`,
      agent: r.pane.agent?.kind,
      terminal: !r.pane.agent,
      href: paneHref(r.host.config.id, r.pane.id),
    }))
    const t = q.toLowerCase()
    return panes.filter((p) => !t || `${p.label} ${p.sub}`.toLowerCase().includes(t)).slice(0, 12)
  }, [hosts, q])
  useEffect(() => {
    if (open) {
      setQ('')
      setI(0)
      setTimeout(() => input.current?.focus(), 10)
    }
  }, [open])
  if (!shown) return null
  const go = (href: string) => {
    nav(href)
    onClose()
  }
  return (
    <div className={clsx('fixed inset-0 z-[80] flex items-start justify-center bg-black/45 px-4 pt-[12vh]', open ? 'animate-fade-in' : 'animate-fade-out')} onClick={onClose}>
      <div className={clsx('w-full max-w-lg overflow-hidden rounded-3xl border border-line bg-elev shadow-float', open ? 'animate-pop' : 'animate-fade-out')} onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2.5 border-b border-line px-4">
          <Search className="size-4 text-fg-3" />
          <input
            ref={input}
            value={q}
            onChange={(e) => (setQ(e.target.value), setI(0))}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose()
              if (e.key === 'ArrowDown') (e.preventDefault(), setI((n) => Math.min(items.length - 1, n + 1)))
              if (e.key === 'ArrowUp') (e.preventDefault(), setI((n) => Math.max(0, n - 1)))
              if (e.key === 'Enter' && items[i]) go(items[i].href)
            }}
            placeholder="Jump to a session or terminal…"
            className="h-12 flex-1 bg-transparent text-[15px] placeholder:text-fg-3"
          />
          <kbd className="rounded-full border border-line px-1.5 font-mono text-[12px] text-fg-3 pointer-coarse:hidden">esc</kbd>
        </div>
        <div className="max-h-[50vh] overflow-y-auto p-1.5">
          {items.map((it, n) => (
            <button key={it.key} type="button" onMouseEnter={() => setI(n)} onClick={() => go(it.href)} className={clsx('flex w-full items-center gap-3 rounded-2xl px-3 py-2 text-left', n === i && 'bg-hover')}>
              {it.terminal ? <AgentLogo size={22} /> : <AgentLogo kind={it.agent} size={22} />}
              <div className="min-w-0">
                <div className="truncate text-[13.5px] font-medium">{it.label}</div>
                <div className="truncate text-[12px] text-fg-3">{it.sub}</div>
              </div>
            </button>
          ))}
          {!items.length && <div className="px-3 py-6 text-center text-[13px] text-fg-3">Nothing matches</div>}
        </div>
      </div>
    </div>
  )
}

function Sidebar({ onNew, onSearch }: { onNew: () => void; onSearch: () => void }) {
  const hosts = useStore((s) => s.hosts)
  const connected = useStore((s) => s.connected)
  const online = hosts.filter((h) => h.status === 'online').length
  const changed = usePages(changedCount)
  const total = usePages((s) => s.pages?.length ?? 0)
  const item = (active: boolean) =>
    clsx(
      'relative flex h-9 items-center gap-2.5 rounded-full px-3 text-[13.5px] transition-colors',
      active
        ? 'bg-elev font-medium text-fg shadow-card ring-1 ring-line'
        : 'text-fg-2 hover:bg-hover/70 hover:text-fg',
    )

  return (
    <aside className="pt-safe hidden w-[240px] shrink-0 flex-col border-r border-line bg-sidebar md:flex lg:w-[264px]">
      <div className="flex h-[60px] items-center gap-2.5 px-5">
        <Logo target />
        <span className="text-[17px] font-semibold tracking-[-0.04em]">Inyon</span>
        {!connected && (
          <button type="button" onClick={() => !isPhone() && openConnectHelp('self')} className="relative ml-auto flex size-8 items-center justify-center rounded-full text-warn hover:bg-hover" aria-label="Reconnecting" title={isPhone() ? 'Reconnecting' : "Reconnecting. Can't connect?"}>
            <WifiOff className="size-4" />
          </button>
        )}
      </div>
      <div className="space-y-1 px-3">
        <button type="button" onClick={onSearch} className="mb-2 flex h-9 w-full items-center gap-2 rounded-full border border-line bg-elev px-3 text-[13px] text-fg-3 shadow-card hover:border-line-strong">
          <Search className="size-3.5" />
          Search or jump to…
          <kbd className="ml-auto rounded-full border border-line px-2 font-mono text-[12px] pointer-coarse:hidden">{keyLabel('cmd', 'K')}</kbd>
        </button>
        <button type="button" onClick={onNew} className={item(false)}>
          <Plus className="size-4" /> New session
          <kbd className="ml-auto font-mono text-[12px] text-fg-3 pointer-coarse:hidden">{keyLabel('alt', 'N')}</kbd>
        </button>
        <NavLink to="/" end className={({ isActive }) => item(isActive)}>
          <LayoutGrid className="size-4" /> Overview
        </NavLink>
        <CompanionNavItem className={item} />
      </div>
      <nav className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        <SessionTree hosts={hosts} />
      </nav>
      <div className="space-y-1 border-t border-line p-3">
        <NavLink to="/machines" className={({ isActive }) => item(isActive)}>
          <Monitor className="size-4" /> {machinesLabel}
          {hosts.length > 0 && (
            <span className="tabular ml-auto font-mono text-[12px] text-fg-3" title={`${online} of ${hosts.length} online`}>
              {online === hosts.length ? hosts.length : `${online}/${hosts.length}`}
            </span>
          )}
        </NavLink>
        <NavLink to="/pages" className={({ isActive }) => item(isActive)}>
          <PanelsTopLeft className="size-4" /> Pages
          {changed > 0 ? (
            <span className="tabular animate-pop ml-auto flex h-[18px] min-w-[18px] items-center justify-center rounded-full bg-fg px-1.5 font-mono text-[12px] leading-none font-semibold text-bg" title={`${changed} changed since you last opened ${changed === 1 ? 'it' : 'them'}`}>
              {changed}
            </span>
          ) : (
            total > 0 && <span className="tabular ml-auto font-mono text-[12px] text-fg-3">{total}</span>
          )}
        </NavLink>
        <NavLink to="/settings" className={({ isActive }) => item(isActive)}>
          <Settings className="size-4" /> Settings
        </NavLink>
      </div>
    </aside>
  )
}

function TabBar({ onNew }: { onNew: () => void }) {
  const waiting = useStore((s) => agentPanes(s.hosts).filter((r) => r.pane.agent!.state === 'waiting').length)
  const changed = usePages(changedCount)
  const tab = ({ isActive }: { isActive: boolean }) => clsx('flex min-w-0 flex-col items-center gap-0.5 py-1.5 text-[12px] font-medium', isActive ? 'text-fg' : 'text-fg-3')
  return (
    <nav className="pb-safe grid shrink-0 grid-cols-5 items-center border-t border-line bg-bg px-1 pt-1 md:hidden">
      <NavLink to="/" end className={tab}>
        <span className="relative">
          <LayoutGrid className="size-[21px]" />
          {waiting > 0 && <span className="absolute -top-1 -right-2 flex h-4 min-w-4 items-center justify-center rounded-full bg-warn px-1 text-[12px] leading-none font-semibold text-black">{waiting}</span>}
        </span>
        Overview
      </NavLink>
      <NavLink to="/pages" className={tab}>
        <span className="relative">
          <PanelsTopLeft className="size-[21px]" />
          {changed > 0 && (
            <span aria-label={`${changed} changed`} className="tabular animate-pop absolute -top-1 -right-2.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-fg px-1 font-mono text-[12px] leading-none font-semibold text-bg ring-2 ring-bg">
              {changed}
            </span>
          )}
        </span>
        Pages
      </NavLink>
      <button type="button" onClick={onNew} aria-label="New session" className="flex justify-center">
        <span className="press flex h-10 w-14 items-center justify-center rounded-full bg-fg text-bg">
          <Plus className="size-5" strokeWidth={2.4} />
        </span>
      </button>
      <NavLink to="/machines" className={tab}>
        <Monitor className="size-[21px]" />
        {machinesLabel}
      </NavLink>
      <NavLink to="/settings" className={tab}>
        <Settings className="size-[21px]" />
        Settings
      </NavLink>
    </nav>
  )
}

/**
 * Which page is showing, for the page-in animation. Sessions share one key: moving between them, or
 * between a session's chat and terminal, keeps the page mounted (and its scroll and state).
 */
export const pageKey = (path: string) => (path.startsWith('/m/') ? 'session' : /^\/pages(\/|$)/.test(path) ? 'pages' : path.replace(/\/+$/, '') || '/')

/** Pages that changed since they were last opened, for the badges. */
const changedCount = (s: ReturnType<typeof usePages.getState>) => s.pages?.filter((p) => isChanged(p, s.seen)).length ?? 0

/** Keeps the list of pages fresh while the app is open: every 30 s in view, and when it comes back. */
function usePagesFeed() {
  useEffect(() => {
    void loadPages()
    const visible = () => document.visibilityState === 'visible'
    const t = setInterval(() => visible() && void loadPages(), 30_000)
    const back = () => visible() && void loadPages()
    document.addEventListener('visibilitychange', back)
    const off = onHubs(() => void loadPages())
    return () => {
      clearInterval(t)
      document.removeEventListener('visibilitychange', back)
      off()
    }
  }, [])
}

export function AppShell() {
  const [newOpen, setNewOpen] = useState(false)
  const [search, setSearch] = useState(false)
  const loc = useLocation()
  const inSession = /\/p\//.test(loc.pathname)
  // a page open on a phone takes the whole screen
  const inPage = /^\/pages\/[^/]+\/[^/]+/.test(loc.pathname)
  usePagesFeed()

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const what = shortcut(e)
      if (!what) return
      e.preventDefault()
      if (what === 'search') setSearch(true)
      else setNewOpen(true)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const online = useStore((s) => s.hosts.some((h) => h.status === 'online'))
  // a right-click on nothing in particular offers the same two things as the shortcuts
  usePageMenu(() => [
    { key: 'search', label: 'Search…', icon: <Search className="size-4" />, keys: keyLabel('cmd', 'K'), onSelect: () => setSearch(true) },
    { key: 'new', label: online ? 'New session…' : 'Add a machine…', icon: <Plus className="size-4" />, keys: keyLabel('alt', 'N'), onSelect: () => setNewOpen(true) },
  ])

  return (
    <div className="flex h-dvh overflow-hidden">
      <Sidebar onNew={() => setNewOpen(true)} onSearch={() => setSearch(true)} />
      <main className="flex min-w-0 flex-1 flex-col">
        {/* each page settles in: a short fade and an 8px rise */}
        <div key={pageKey(loc.pathname)} className="animate-page flex min-h-0 flex-1 flex-col">
          <Suspense fallback={<Waiting label="Opening" shape="lines" className="mx-auto w-full max-w-[760px] p-5" />}>
            <Outlet />
          </Suspense>
        </div>
        {!inSession && !inPage && <TabBar onNew={() => setNewOpen(true)} />}
      </main>
      {/* nothing to start a session on yet: + adds a machine instead of an empty form */}
      {online ? <NewSessionSheet open={newOpen} onClose={() => setNewOpen(false)} /> : <MachineSheet open={newOpen} onClose={() => setNewOpen(false)} />}
      <QuickSwitch open={search} onClose={() => setSearch(false)} />
      <ActionSheets />
      <PageSheets />
      <ConnectHelpHost />
    </div>
  )
}
