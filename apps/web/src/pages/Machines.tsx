import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import clsx from 'clsx'
import { AlertCircle, ChevronLeft, ChevronRight, FileCode2, HardDrive, Pencil, Plug, Plus, RotateCcw, Save, Smartphone, SquareTerminal, Trash2 } from 'lucide-react'
import { agentDef, type AgentKind, type HostState } from '@inyon/protocol'
import { api, type ConfigFile, type UploadUsage } from '../lib/api.ts'
import { toast, useStore } from '../lib/store.ts'
import { paneHref, paneTitle } from '../lib/overview.ts'
import { RESPONSE_TITLE, bytes, responseTime, shortPath, timeAgo } from '../lib/format.ts'
import { INTEGRATION_TEXT } from '../components/session/IntegrationNote.tsx'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Empty, Pulse, SectionLabel, Segmented, Waiting } from '../components/ui/primitives.tsx'
import { HostDot, StateDot, hostLine, shownState, sshAddress } from '../components/ui/status.tsx'
import { MachineSheet, NewSessionSheet, PairMachine } from '../components/Sheets.tsx'
import { PhoneSheet } from '../components/PhoneSheet.tsx'
import { Logo } from '../components/AppShell.tsx'
import { SetupPanel } from '../components/SetupPanel.tsx'
import { SignInSheet } from '../components/SignInSheet.tsx'
import { isPhone } from '../lib/phone.ts'
import { forgetPairFragment, pairFragment } from '../lib/native.ts'
import { listHubs } from '../lib/hubs.ts'
import { PullRefresh } from '../components/ui/PullRefresh.tsx'
import { refreshAll } from '../lib/refresh.ts'
import { AgentBadges } from '../components/Overview.tsx'
import { MachineIcon, machineKindOf, onlineFirst } from '../components/ui/MachineIcon.tsx'
import { FilterInput } from '../components/ui/FilterInput.tsx'
import { matchesQuery } from '../lib/groups.ts'
import { confirmAction } from '../components/Confirm.tsx'
import { NodeUpdate, nodeUpdateLabel } from '../components/NodeUpdate.tsx'
import { contextMenu } from '../components/ui/ContextMenu.tsx'
import { openConnectHelp, targetFor } from '../components/ConnectSheet.tsx'
import { machineMenu, paneMenu, removeMachine } from '../components/menus.tsx'
import { MachinePages } from '../components/pages/shared.tsx'
import { TempFiles } from './Temp.tsx'

function PageHeader({ title, back, right }: { title: React.ReactNode; back?: string; right?: React.ReactNode }) {
  return (
    <header className="pt-safe sticky top-0 z-20 border-b border-line bg-bg">
      <div className="flex h-[52px] items-center gap-2 px-3 lg:px-7">
        {back ? (
          <Link to={back} aria-label="Back" className="inline-flex size-8 items-center justify-center rounded-full text-fg-2 hover:bg-hover relative after:absolute after:-inset-1">
            <ChevronLeft className="size-5" />
          </Link>
        ) : (
          <span className="md:hidden">
            <Logo />
          </span>
        )}
        <h1 className="min-w-0 flex-1 truncate px-1 text-[15px] font-semibold tracking-tight">{title}</h1>
        {right}
      </div>
    </header>
  )
}

/** Past this many machines the list gets a search box. */
const SEARCH_FROM = 8

/**
 * "studio · 2 sessions · 2 agents · responds in 38 ms", wrapping only between items: each dot
 * stays with the item before it and each count with its word (no-break spaces), so no line starts
 * with a dot. The time is how long the machine took to report its sessions, not a network round trip.
 */
export function metaLine(where: string, sessions: number, agents: number, ms?: number) {
  const nb = (t: string) => t.replace(/ /g, '\u00a0')
  const parts = [where, nb(`${sessions} session${sessions === 1 ? '' : 's'}`)]
  if (agents) parts.push(nb(`${agents} agent${agents === 1 ? '' : 's'}`))
  if (ms !== undefined) parts.push(nb(`responds in ${responseTime(ms)}`))
  return parts.join('\u00a0· ')
}

/** One machine in the list: what it is, whether it answers, and what runs there. */
function MachineRow({ h, hosts, i }: { h: HostState; hosts: HostState[]; i: number }) {
  const nav = useNavigate()
  const where = h.config.kind === 'local' ? 'This hub' : h.config.kind === 'node' ? h.hostname ?? 'Paired machine' : h.config.kind === 'link' ? h.hostname ?? 'Paired' : sshAddress(h.config, hosts)
  const panes = h.sessions.reduce((n, s) => n + s.windows.reduce((m, w) => m + w.panes.filter((p) => p.agent).length, 0), 0)
  return (
    <Link style={{ '--i': i } as React.CSSProperties} to={`/machines/${encodeURIComponent(h.config.id)}`} {...contextMenu(() => machineMenu(h, nav), { touch: true })} className="flex min-h-[56px] items-center gap-3 px-4 py-2.5 hover:bg-hover/60 active:bg-hover">
      <span className={clsx('relative flex size-9 shrink-0 items-center justify-center rounded-xl border border-line bg-sunken', h.status === 'online' ? 'text-fg-2' : 'text-fg-3')}>
        <MachineIcon kind={machineKindOf(h)} size={18} />
        <span className="absolute -right-0.5 -bottom-0.5 flex size-[11px] items-center justify-center rounded-full bg-elev">
          {h.status === 'online' || h.status === 'error' ? <HostDot status={h.status} /> : <span className="size-[7px] rounded-full border border-fg-3" />}
        </span>
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className={clsx('truncate text-[14px] font-medium', h.status !== 'online' && 'text-fg-2')}>{h.config.name}</span>
          {nodeUpdateLabel(h) && <span className="inline-flex h-5 shrink-0 items-center rounded-full border border-line bg-white/45 px-2 font-mono text-[12px] text-fg-2 dark:bg-white/[0.03]">{nodeUpdateLabel(h)}</span>}
        </div>
        {/* two lines at most, so the counts stay readable on a phone */}
        <div className="line-clamp-2 font-mono text-[12px] break-words text-fg-3">
          {waitingToPair(h) ? 'Waiting for its pairing command' : hostLine(h, metaLine(where, h.sessions.length, panes, h.latencyMs))}
        </div>
      </div>
      {/* on a phone the address needs the room more; the machine page lists the agents */}
      <span className="hidden sm:contents">
        <AgentBadges host={h} size={18} />
      </span>
      <ChevronRight className="size-4 shrink-0 text-fg-3" />
    </Link>
  )
}

export function MachinesPage() {
  const hosts = useStore((s) => s.hosts)
  const loaded = useStore((s) => s.hostsLoaded) || hosts.length > 0
  // nothing listed, but a hub is paired: the phone can't reach it right now (off its VPN, say)
  const hubs = loaded && !hosts.length ? listHubs() : []
  const [q, setQ] = useState('')
  const shown = useMemo(() => onlineFirst(hosts).filter((h) => !q || matchesQuery(`${h.config.name} ${h.hostname ?? ''} ${h.machineKind ?? ''} ${h.os ?? ''} ${h.agents.join(' ')}`, q)), [hosts, q])
  // a pairing link opened from outside (a QR code scanned with the camera) lands here
  const [params, setParams] = useSearchParams()
  // a hub's code goes the same way; the sheet tells the two apart
  const pairLink = params.get('pair') ?? params.get('hub') ?? pairFragment()
  // "Pair again" from Can't connect? opens the same sheet
  const adding = params.has('add')
  const [sheet, setSheet] = useState(!!pairLink || adding)
  const [phone, setPhone] = useState(false)
  // a code scanned while this page is already open
  useEffect(() => {
    if (pairLink || adding) setSheet(true)
  }, [pairLink, adding])
  const close = () => {
    setSheet(false)
    forgetPairFragment()
    if (params.get('pair') || params.get('hub') || adding) setParams({}, { replace: true })
  }
  return (
    <PullRefresh onRefresh={refreshAll} className="min-h-0 flex-1 overflow-y-auto">
      <PageHeader
        title={isPhone() ? 'Computers' : 'Machines'}
        right={
          <span className="flex items-center gap-2">
            {!isPhone() && (
              <Button size="sm" variant="ghost" onClick={() => setPhone(true)}>
                <Smartphone className="size-3.5" /> Phone
              </Button>
            )}
            <Button size="sm" variant="primary" onClick={() => setSheet(true)}><Plus className="size-3.5" /> {isPhone() ? 'Add' : 'Add machine'}</Button>
          </span>
        }
      />
      <div className="mx-auto max-w-[880px] px-4 py-6 lg:px-7">
        {hosts.length > SEARCH_FROM && <FilterInput value={q} onChange={setQ} placeholder={`Search ${hosts.length} ${isPhone() ? 'computers' : 'machines'}`} className="mb-3" />}
        {!loaded ? (
          // rows in the list's shape, until the first answer
          <div role="status" aria-label={`Loading ${isPhone() ? 'computers' : 'machines'}`} className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
            {[0, 1, 2].map((i) => (
              <div key={i} className="flex min-h-[56px] items-center gap-3 px-4 py-2.5">
                <div className="skeleton size-9 rounded-xl" />
                <div className="flex-1">
                  <div className="skeleton h-3.5 w-28 rounded" />
                  <div className="skeleton mt-2 h-3 w-44 rounded" />
                </div>
              </div>
            ))}
          </div>
        ) : !hosts.length ? (
          <div className="rounded-3xl border border-dashed border-line-strong">
            <Empty
              icon={<Logo className="size-7 mark-drawn" />}
              title={hubs.length ? `Connecting to ${hubs.map((h) => h.name).join(' and ')}…` : isPhone() ? 'Connect your computer' : 'No machines yet'}
              action={
                <Button variant="primary" onClick={() => setSheet(true)}>
                  <Plus className="size-3.5" /> {isPhone() ? 'Add a computer' : 'Add a machine'}
                </Button>
              }
            >
              {hubs.length
                ? `${hubs.length === 1 ? 'Its computers appear as soon as it answers.' : 'Their computers appear as soon as they answer.'} On a VPN, make sure this ${isPhone() ? 'phone' : 'device'} is connected to it too.`
                : isPhone()
                ? 'See the coding agents on your laptop or server and talk to them from here. Run one command on it, then scan the code it shows.'
                : 'Pair a machine or add one over SSH to see the coding agents running in tmux on it.'}
            </Empty>
          </div>
        ) : (
          shown.length > 0 && (
            <div className="stagger divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
              {shown.map((h, i) => (
                <MachineRow key={h.config.id} h={h} hosts={hosts} i={i} />
              ))}
            </div>
          )
        )}
        {q && !shown.length && <p className="px-1 py-6 text-center text-[13px] text-fg-3">No {isPhone() ? 'computer' : 'machine'} matches “{q}”</p>}
      </div>
      <MachineSheet open={sheet} onClose={close} link={pairLink} />
      {!isPhone() && <PhoneSheet open={phone} onClose={() => setPhone(false)} />}
    </PullRefresh>
  )
}

/** A machine added with a pairing command that hasn't run it yet. */
const waitingToPair = (h: HostState) => h.config.kind === 'node' && !h.config.pairedAt && h.status !== 'online'

type Tab = 'sessions' | 'setup' | 'config' | 'uploads' | 'temp' | 'integration'

export function MachinePage() {
  const id = decodeURIComponent(useParams().host!)
  const host = useStore((s) => s.hosts.find((h) => h.config.id === id))
  const via = useStore((s) => (host?.config.via ? s.hosts.find((h) => h.config.id === host.config.via)?.config.name ?? 'a removed computer' : undefined))
  const needsSetup = !!host && host.status === 'online' && (host.tmux !== 'ok' || !host.agents.length)
  const [tab, setTab] = useState<Tab>(needsSetup ? 'setup' : 'sessions')
  const [edit, setEdit] = useState(false)
  const [repair, setRepair] = useState(false)
  const [newOpen, setNewOpen] = useState(false)
  const nav = useNavigate()
  // the machines haven't arrived yet (or this one is gone): the page's shape, sessions as rows
  if (!host) return <div className="mx-auto w-full max-w-[1100px] flex-1 px-4 py-5 lg:px-7"><Waiting shape="rows" label="Loading this machine" /></div>
  const remove = () => removeMachine(host, () => nav('/machines'))
  // a joined machine that never ran its pairing command has its own instructions below
  const checkable = !waitingToPair(host) && !!targetFor(host.config.id, [host])
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <PageHeader
        back="/machines"
        title={
          <span className="flex items-center gap-2">
            <MachineIcon kind={machineKindOf(host)} size={17} className="shrink-0 text-fg-2" /> {host.config.name} <HostDot status={host.status} />
          </span>
        }
        right={
          <div className="flex gap-1.5">
            {/* on a phone the button is its icon, so the machine's name keeps the room */}
            {!waitingToPair(host) && (
              <Button size="sm" onClick={() => setNewOpen(true)} aria-label="New session" title="New session">
                <Plus className="size-3.5" /> <span className="max-sm:hidden">New session</span>
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => setEdit(true)} aria-label="Rename or edit"><Pencil className="size-3.5" /> <span className="hidden sm:inline">{host.config.kind === 'ssh' ? 'Edit' : 'Rename'}</span></Button>
            {host.config.kind !== 'local' && <Button size="sm" variant="ghost" onClick={remove} aria-label="Remove"><Trash2 className="size-3.5" /></Button>}
          </div>
        }
      />
      <div className="mx-auto max-w-[1100px] px-4 py-5 lg:px-7">
        <p className="mb-4 text-[13px] text-fg-3">
          {host.config.kind === 'local'
            ? 'This hub'
            : host.config.kind === 'node'
              ? waitingToPair(host) ? 'Not paired yet' : `Paired machine${host.hostname ? ` · ${host.hostname}` : ''}`
              : host.config.kind === 'link'
                ? `Paired${host.hostname ? ` · ${host.hostname}` : ''}`
                : `ssh ${host.config.target}${host.config.port ? ` -p ${host.config.port}` : ''}${via ? ` from ${via}` : ''}`}
          {host.nodeVersion && ` · inyon ${host.nodeVersion}`}
          {host.os && ` · ${host.os}`}
          {host.latencyMs !== undefined && <span title={RESPONSE_TITLE}>{` · responds in ${responseTime(host.latencyMs)}`}</span>}
        </p>
        <NodeUpdate host={host} />
        {/* the machine's own warning about how it reaches the hub (plain http, added before 0.20) */}
        {host.nodeWarning && (
          <div className="animate-slide-up mb-4 flex items-start gap-2 rounded-2xl bg-warn-soft px-3.5 py-3 text-[13px] text-warn">
            <AlertCircle className="mt-0.5 size-4 shrink-0" />
            <span className="min-w-0 flex-1 break-words">{host.nodeWarning}</span>
          </div>
        )}
        {host.status !== 'online' && (host.error || checkable) && (
          <div className={clsx('animate-slide-up mb-4 flex flex-wrap items-start gap-2 rounded-2xl px-3.5 py-3 text-[13px]', host.status === 'error' ? 'bg-danger-soft text-danger' : 'bg-sunken text-fg-2')}>
            {host.status === 'error' ? <AlertCircle className="mt-0.5 size-4 shrink-0" /> : <span className="mt-0.5 flex size-4 shrink-0 items-center justify-center"><Pulse /></span>}
            <span className="min-w-0 flex-[1_1_200px] break-words">{host.error ?? 'Not connected right now.'}</span>
            <span className="flex gap-1.5">
              {/* unpaired on the computer: scanning a fresh code fixes it, in place */}
              {host.config.kind === 'link' && host.error && /paired/i.test(host.error) && (
                <Button size="xs" variant="secondary" className="-my-0.5" onClick={() => setRepair(true)}>Pair again</Button>
              )}
              {checkable && (
                <Button size="xs" variant="secondary" className="-my-0.5" onClick={() => openConnectHelp(host.config.id)}>Can't connect?</Button>
              )}
            </span>
          </div>
        )}
        {waitingToPair(host) ? (
          <div className="max-w-[560px] rounded-2xl border border-line p-4">
            <PairMachine existing={host.config.id} />
          </div>
        ) : (
        <>
        <div className="mb-5">
          <div className="no-scrollbar overflow-x-auto">
            <Segmented value={tab} onChange={setTab} options={[{ v: 'sessions', label: 'Sessions' }, { v: 'setup', label: needsSetup ? 'Setup ·' : 'Setup' }, { v: 'config', label: 'Config' }, { v: 'uploads', label: 'Uploads' }, { v: 'temp', label: 'Temp files' }, { v: 'integration', label: 'Integration' }]} />
          </div>
        </div>
        <div key={tab} className="animate-fade-in">
          {tab === 'sessions' && <SessionsTab host={host} onNew={() => setNewOpen(true)} />}
          {tab === 'setup' && <SetupPanel host={host} />}
          {tab === 'config' && <ConfigTab host={host} />}
          {tab === 'uploads' && <UploadsCard key={host.config.id} host={host} />}
          {tab === 'temp' && <TempFiles key={host.config.id} host={host} />}
          {tab === 'integration' && <IntegrationCard host={host} />}
        </div>
        <MachinePages host={host.config.id} />
        </>
        )}
      </div>
      <MachineSheet open={edit} edit={host.config} onClose={() => setEdit(false)} />
      <MachineSheet open={repair} onClose={() => setRepair(false)} />
      <NewSessionSheet open={newOpen} hostId={host.config.id} onClose={() => setNewOpen(false)} />
    </div>
  )
}

function SessionsTab({ host, onNew }: { host: HostState; onNew: () => void }) {
  const nav = useNavigate()
  if (host.status !== 'online') return <Empty title="Not connected">Sessions appear once the machine is reachable.</Empty>
  if (!host.sessions.length) {
    return (
      <Empty icon={<SquareTerminal className="size-5" />} title="No tmux sessions" action={<Button onClick={onNew}><Plus className="size-3.5" /> New session</Button>}>
        Start one from here, or in tmux on the {isPhone() ? 'computer' : 'machine'}.
      </Empty>
    )
  }
  return (
    <div className="space-y-4">
      {host.sessions.map((s) => (
        <section key={s.id} className="overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
          <div className="flex items-center gap-2 border-b border-line px-4 py-2.5 text-[12.5px]">
            <span className="font-semibold">{s.name}</span>
            {s.attached > 0 && <span className="rounded-full bg-sunken px-1.5 py-px text-[12px] text-fg-3">attached</span>}
            <span className="ml-auto text-fg-3">{s.windows.length} window{s.windows.length === 1 ? '' : 's'}</span>
          </div>
          {s.windows.flatMap((w) =>
            w.panes.map((p) => {
              const r = { host, session: s, window: w, pane: p }
              return (
                <Link key={p.id} to={paneHref(host.config.id, p.id)} {...contextMenu(() => paneMenu(r, nav), { touch: true })} className="flex items-center gap-3 border-b border-line/60 px-4 py-2.5 last:border-0 hover:bg-hover/50">
                  {p.agent ? <AgentLogo kind={p.agent.kind} size={20} /> : <SquareTerminal className="size-4 text-fg-3" />}
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13.5px] font-medium">
                      <span className="mr-2 font-mono text-[12px] text-fg-3">{w.index}.{p.index}</span>
                      {p.agent ? paneTitle(r) : p.command}
                    </div>
                    <div className="truncate text-[12px] text-fg-3">{shortPath(p.cwd, 50)}</div>
                  </div>
                  {p.agent && <StateDot state={shownState(p.agent, host)} />}
                  <ChevronRight className="size-4 text-fg-3" />
                </Link>
              )
            }),
          )}
        </section>
      ))}
    </div>
  )
}

/** Agent config files on the machine: list, edit with validation, save with backup. */
function ConfigTab({ host }: { host: HostState }) {
  const [files, setFiles] = useState<ConfigFile[] | null>(null)
  const [sel, setSel] = useState<string | null>(null)
  const [orig, setOrig] = useState('')
  const [text, setText] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const id = host.config.id

  useEffect(() => {
    api.configs(id).then((f) => {
      setFiles(f)
      if (!sel) setSel(f.find((x) => x.exists)?.path ?? f[0]?.path ?? null)
    }).catch((e) => toast({ title: 'Could not list config files', body: e.message, tone: 'error' }))
  }, [id])

  useEffect(() => {
    if (!sel) return
    setLoading(true)
    api.readConfig(id, sel).then((r) => (setOrig(r.content), setText(r.content))).catch((e) => toast({ title: 'Could not read', body: e.message, tone: 'error' })).finally(() => setLoading(false))
  }, [sel])

  const cur = files?.find((f) => f.path === sel)
  const jsonError = useMemo(() => {
    if (cur?.format !== 'json' || !text.trim()) return ''
    try {
      JSON.parse(text)
      return ''
    } catch (e) {
      return (e as Error).message
    }
  }, [text, cur?.format])
  const dirty = text !== orig

  const save = async () => {
    if (!sel) return
    setSaving(true)
    try {
      await api.writeConfig(id, sel, text)
      setOrig(text)
      toast({ title: 'Saved', body: 'A backup of the previous version was kept next to the file. Running sessions pick changes up after a restart.', tone: 'success' })
      api.configs(id).then(setFiles)
    } catch (e) {
      toast({ title: 'Could not save', body: (e as Error).message, tone: 'error' })
    } finally {
      setSaving(false)
    }
  }

  if (!files) return <Waiting shape="rows" label={`Looking for config files on ${host.config.name}`} />
  if (!files.length) return <Empty icon={<FileCode2 className="size-5" />} title="No agent config files">Install an agent on this machine and its config files show up here.</Empty>
  const groups = [...new Set(files.map((f) => f.agent))]
  return (
    <div className="flex flex-wrap gap-4">
      <nav className="w-full shrink-0 space-y-3 md:w-[280px]">
        {groups.map((g) => (
          <div key={g}>
            <div className="mb-1 flex items-center gap-2 px-1.5 text-[12px] font-semibold">
              <AgentLogo kind={g} size={16} /> {agentDef(g).name}
            </div>
            {files.filter((f) => f.agent === g).map((f) => (
              <button
                key={f.path}
                type="button"
                onClick={async () => (dirty && !(await confirmAction({ title: 'Discard unsaved changes?', yes: 'Discard', danger: true })) ? null : setSel(f.path))}
                className={clsx('flex w-full items-center gap-2 rounded-xl px-2.5 py-1.5 text-left text-[12.5px]', sel === f.path ? 'bg-hover' : 'hover:bg-hover/60')}
              >
                <span className={clsx('min-w-0 flex-1 truncate', !f.exists && 'text-fg-3')}>{f.label}</span>
                <span className="shrink-0 text-[12px] text-fg-3">{f.exists ? bytes(f.size) : 'new'}</span>
              </button>
            ))}
          </div>
        ))}
      </nav>
      <div className="min-w-0 flex-[999_1_480px]">
        <div className="overflow-hidden rounded-2xl border border-line bg-elev">
          <div className="flex items-center gap-2 border-b border-line px-3.5 py-2.5">
            <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{sel?.replace(/^\/home\/[^/]+/, '~')}</span>
            {cur?.mtime && <span className="hidden shrink-0 text-[12px] text-fg-3 sm:inline">changed {timeAgo(cur.mtime * 1000)}</span>}
          </div>
          {loading ? (
            <Waiting shape="lines" count={8} label={`Reading ${cur?.label ?? 'the file'}`} className="bg-code px-3.5 py-4" />
          ) : (
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              spellCheck={false}
              className="block h-[52dvh] w-full resize-none bg-code px-3.5 py-3 font-mono text-[12.5px] leading-[1.65] text-fg"
              placeholder={cur?.exists ? '' : 'This file does not exist yet. Saving creates it.'}
            />
          )}
          <div className="flex items-center gap-2 border-t border-line px-3.5 py-2.5">
            <span className={clsx('min-w-0 flex-1 truncate text-[12px]', jsonError ? 'text-danger' : dirty ? 'text-warn' : 'text-fg-3')}>
              {jsonError ? `Invalid JSON: ${jsonError}` : dirty ? 'Unsaved changes' : cur?.format === 'json' ? 'Valid JSON' : 'Saved'}
            </span>
            <Button size="sm" variant="ghost" disabled={!dirty} onClick={() => setText(orig)}><RotateCcw className="size-3.5" /> Discard</Button>
            <Button size="sm" variant="primary" disabled={!dirty || !!jsonError} loading={saving} onClick={save}><Save className="size-3.5" /> Save</Button>
          </div>
        </div>
        <p className="mt-2 text-[12px] text-fg-3">Every save keeps the previous version as a .inyon-bak file next to it (the last five).</p>
      </div>
    </div>
  )
}

/** The age the clean button cleans: the hub's retention setting, which its count of expired files uses; 7 days when the hub doesn't say or keeps uploads forever. */
export const cleanDays = (u: UploadUsage) => (u.days && u.days > 0 ? u.days : 7)

export function UploadsCard({ host, compact }: { host: HostState; compact?: boolean }) {
  const [usage, setUsage] = useState<UploadUsage | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  useEffect(() => {
    if (host.status === 'online') api.uploadUsage(host.config.id).then(setUsage).catch(() => {})
  }, [host.config.id, host.status])
  const clean = async (days: number) => {
    if (days === 0 && !(await confirmAction({ title: 'Delete every uploaded file?', body: `All files sent from the app to ${host.config.name} are removed.`, yes: 'Delete all', danger: true }))) return
    setBusy(String(days))
    try {
      setUsage(await api.cleanUploads(host.config.id, days))
      toast({ title: 'Uploads cleaned', tone: 'success' })
    } catch (e) {
      toast({ title: 'Could not clean', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }
  return (
    <div className={clsx('rounded-3xl border border-line bg-elev shadow-card p-4', compact && 'p-3.5')}>
      <div className="flex items-center gap-2.5">
        <HardDrive className="size-4 text-fg-3" />
        <span className="text-[13.5px] font-semibold">{host.config.name}</span>
        <span className="tabular ml-auto text-[12.5px] text-fg-2">{usage ? `${bytes(usage.bytes)} · ${usage.files} files` : host.status === 'online' ? '…' : 'offline'}</span>
      </div>
      {usage && (
        <>
          <div className="mt-1 truncate font-mono text-[12px] text-fg-3">{usage.dir.replace(/^\/home\/[^/]+/, '~')}</div>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" onClick={() => clean(cleanDays(usage))} loading={busy === String(cleanDays(usage))} disabled={!usage.files}>Clean older than {cleanDays(usage)} day{cleanDays(usage) === 1 ? '' : 's'}{usage.expired && usage.days ? ` (${usage.expired})` : ''}</Button>
            <Button size="sm" variant="danger" onClick={() => clean(0)} loading={busy === '0'} disabled={!usage.files}>Delete all</Button>
          </div>
        </>
      )}
    </div>
  )
}

function IntegrationCard({ host }: { host: HostState }) {
  const [busy, setBusy] = useState(false)
  const [signIn, setSignIn] = useState<AgentKind | null>(null)
  const on = host.integration === 'installed'
  const toggle = async () => {
    setBusy(true)
    try {
      await api.integration(host.config.id, !on)
      toast({ title: on ? 'Integration turned off' : 'Integration on', body: on ? undefined : 'Restart running Claude sessions to pick it up.', tone: 'success' })
    } catch (e) {
      toast({ title: 'Failed', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="max-w-2xl space-y-4">
      <div className="rounded-3xl border border-line bg-elev shadow-card p-5">
        <div className="flex items-center gap-3">
          <Plug className={clsx('size-5', on ? 'text-fg' : 'text-fg-3')} />
          <div className="flex-1">
            <div className="text-[15px] font-semibold">Claude Code integration</div>
            <div className="text-[12.5px] text-fg-3">{on ? 'On' : host.integration === 'missing' ? 'Off' : host.integration === 'unknown' ? 'Unknown' : INTEGRATION_TEXT[host.integration]}</div>
          </div>
          <Button variant={on ? 'danger' : 'primary'} onClick={toggle} loading={busy} disabled={host.status !== 'online'}>
            {on ? 'Turn off' : host.integration === 'outdated' ? 'Update' : 'Turn on'}
          </Button>
        </div>
        <ul className="mt-4 space-y-2 text-[13px] leading-relaxed text-fg-2">
          <li>• Adds a tiny hook that records which tmux pane runs which session, for exact matching and instant updates.</li>
          <li>• Wraps your status line so Inyon sees model, effort, context window, cost and 5-hour/weekly limits. Your status line still shows exactly as before.</li>
          <li>• Backs up <span className="font-mono text-[12px]">~/.claude/settings.json</span> first. Turning it off restores your original status line.</li>
        </ul>
      </div>
      <SectionLabel>Agents installed on this machine</SectionLabel>
      <div className="flex flex-wrap gap-2">
        {host.agents.length ? host.agents.map((k) => {
          const signedIn = host.signedIn?.includes(k)
          return (
            <span key={k} className="flex items-center gap-2 rounded-full border border-line bg-elev py-1 pr-1 pl-3 font-mono text-[12px]">
              <AgentLogo kind={k} size={18} /> {agentDef(k).name}
              {host.signedIn && (signedIn ? (
                <span className="pr-1.5 text-[12px] text-fg-3">· signed in</span>
              ) : (
                <Button size="xs" variant="primary" onClick={() => setSignIn(k)} disabled={host.status !== 'online'}>Sign in</Button>
              ))}
            </span>
          )
        }) : <span className="text-[13px] text-fg-3">None on PATH</span>}
      </div>
      <SignInSheet hostId={host.config.id} hostName={host.config.name} agent={signIn} onClose={() => setSignIn(null)} />
    </div>
  )
}
