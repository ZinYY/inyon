import { useEffect, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import clsx from 'clsx'
import {
  BookOpen, ChevronDown, ChevronLeft, ChevronRight, CircleArrowUp, CloudOff, Copy, Ellipsis, ExternalLink, FileText, FolderOpen, FolderPlus, LayoutTemplate, Link2Off, PanelsTopLeft,
  NotebookText, RotateCw, Search, ShieldCheck, Smartphone, Table2, Tablet, Monitor, Trash2,
} from 'lucide-react'
import { useStore } from '../lib/store.ts'
import { ApiError } from '../lib/api.ts'
import { copyText } from '../lib/clipboard.ts'
import { toast } from '../lib/store.ts'
import { useBackCloses } from '../lib/back.ts'
import { listHubs } from '../lib/hubs.ts'
import {
  DEVICES, PART_LABEL, changedAt, fitScale, frameSrc, noteSrc, noteTitle, groupName, groupPages, hubForgot, hubIdOf, hubUnreachable, isBridged, isChanged, leftText, loadPages, markSeen, matchesPage, pageBase, pageHref, pageId,
  usePages, type Device, type Page, type Part,
} from '../lib/pages.ts'
import { Logo } from '../components/AppShell.tsx'
import { Button, Empty, IconButton, Menu, Segmented, Waiting, type MenuItem } from '../components/ui/primitives.tsx'
import { contextMenu } from '../components/ui/ContextMenu.tsx'
import { FilterInput } from '../components/ui/FilterInput.tsx'
import { PullRefresh } from '../components/ui/PullRefresh.tsx'
import { MachineIcon, machineKindOf } from '../components/ui/MachineIcon.tsx'
import {
  ADD_COMMAND, BROWSER_HINT, ChangedDot, PAGES_DOCS, PageFrame, ProgressBar, SANDBOX_NOTE, copyFolder, openInBrowser, openPageSheet, pageLabel, pageMenu, removePage, renewLink,
  revokeLinks, updatedLine, useMachines, usePageLink, useRunningNames, useSiteParts, type PageMachine,
} from '../components/pages/shared.tsx'

/** Two panes from this width up; below it, the list and a full-screen viewer. */
const WIDE = '(min-width: 768px)'
function useWide() {
  const [wide, setWide] = useState(() => typeof matchMedia === 'function' && matchMedia(WIDE).matches)
  useEffect(() => {
    const mq = matchMedia(WIDE)
    const on = () => setWide(mq.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  return wide
}

/** `/pages` and `/pages/:host/:slug`. */
export function PagesPage() {
  const params = useParams()
  const host = params.host ? decodeURIComponent(params.host) : undefined
  const id = host && params.slug ? pageId(host, params.slug) : undefined
  const wide = useWide()
  if (wide) return <PagesDesktop selected={id} />
  // keyed by page: picking another page from the viewer starts it afresh
  return id ? <PhoneViewer key={id} id={id} /> : <PhoneList />
}

// ---------- shared by the phone and desktop viewers ----------

function useViewer(page: Page | undefined) {
  const { link, error, retry } = usePageLink(page)
  const parts = useSiteParts(page, link)
  const [want, setPart] = useState<Part>('plan')
  // a note from the site's research folder shown instead of a part, while it's still listed
  const [wantNote, setNote] = useState<string>()
  useEffect(() => {
    setPart('plan')
    setNote(undefined)
  }, [page?.id])
  const part = parts.includes(want) ? want : 'plan'
  const note = wantNote && page?.notes?.includes(wantNote) ? wantNote : undefined
  const [n, setN] = useState(0)
  const theme = useStore((s) => s.resolvedTheme)
  const themeNow = useRef(theme)
  themeNow.current = theme
  // the theme isn't part of the address's identity: a theme change is a message, not a reload
  const src = useMemo(
    () => (page && link ? (note ? noteSrc(pageBase(page.host), link.url, note, themeNow.current) : frameSrc(pageBase(page.host), link.url, part, themeNow.current)) : undefined),
    [page?.host, link?.url, part, note],
  )
  // a new link every time: the one on screen may have been revoked from another device. The new
  // address reloads the frame; when no link comes (the hub is out of reach) the old one is tried again
  const reload = () => void retry().then((renewed) => renewed || setN((x) => x + 1))
  // picking a part always loads it, also the one the control shows when the page moved on by its own links
  const pickPart = (p: Part) => {
    setPart(p)
    setNote(undefined)
    setN((x) => x + 1)
  }
  const pickNote = (name: string) => {
    setNote(name)
    setN((x) => x + 1)
  }
  // looking at a page clears its changed mark, also when it changes while it's open
  const changed = page ? changedAt(page) : 0
  useEffect(() => {
    if (page) markSeen(page)
  }, [page?.id, changed])
  // the page's hub didn't answer the last poll: a frame already showing stays, marked out of date
  const stale = usePages((s) => !!page && hubUnreachable(page.host, s.unreachable))
  // a page on a computer paired with the phone: the app's viewer renders it from the link's files,
  // and there is no address to open outside the app
  const bridged = !!page && isBridged(page.host)
  const bridge = bridged && link ? link.url : undefined
  return { link, error, retry, parts, part, setPart: pickPart, note, setNote: pickNote, src, frameKey: `${src}#${n}`, reload, stale, bridged, bridge }
}

type Viewer = ReturnType<typeof useViewer>

/** The site's research notes, and the way back to the page while one is open. */
function notesMenu(page: Page, v: Viewer): MenuItem[] {
  return [
    ...(v.note ? [{ key: 'back', label: `Back to ${PART_LABEL[v.part]}`, icon: PART_ICON[v.part], onSelect: () => v.setPart(v.part) }] : []),
    ...(page.notes ?? []).map((name, i) => ({ key: `note:${name}`, label: noteTitle(name), icon: <FileText className="size-4" />, selected: v.note === name, separated: !!v.note && i === 0, disabled: !v.link, onSelect: () => v.setNote(name) })),
  ]
}

const PART_ICON: Record<Part, React.ReactNode> = { plan: <FileText className="size-4" />, mockups: <LayoutTemplate className="size-4" />, matrix: <Table2 className="size-4" /> }

/** What the stage shows while there's no frame: opening, gone, or out of reach. */
function FrameState({ page, machine, error, onRetry, onBack }: { page: Page; machine: PageMachine; error?: Error; onRetry: () => void; onBack?: () => void }) {
  if (!machine.online)
    return (
      <StateCard icon={<CloudOff className="size-5 text-danger" />} title={`${machine.name} is offline`}>
        “{page.name}” opens again as soon as it's back.
        <div className="mt-4 flex flex-wrap gap-2">
          <Button size="sm" onClick={onRetry}><RotateCw className="size-3.5" /> Try again</Button>
          <Button size="sm" variant="ghost" onClick={() => void copyFolder(page)}><Copy className="size-3.5" /> Copy folder path</Button>
        </div>
      </StateCard>
    )
  if (!error) return <Waiting shape="panel" count={2} label={`Opening “${page.name}”`} className="w-full max-w-[560px] self-start p-5" />
  const gone = error instanceof ApiError && error.status === 404
  return (
    <StateCard icon={gone ? <FolderOpen className="size-5" /> : <CloudOff className="size-5" />} title={gone ? 'This page is gone' : `Couldn't open “${page.name}”`}>
      {gone ? `It was taken off the list on ${machine.name}.` : error.message}
      <div className="mt-4 flex flex-wrap gap-2">
        {gone ? onBack && <Button size="sm" onClick={onBack}><ChevronLeft className="size-3.5" /> Back to Pages</Button> : <Button size="sm" onClick={onRetry}><RotateCw className="size-3.5" /> Try again</Button>}
      </div>
    </StateCard>
  )
}

/** Said over a page whose hub didn't answer the last time: what's on screen may be behind. */
function StaleLine({ name, className }: { name: string; className?: string }) {
  return (
    <div role="status" className={clsx('flex min-w-0 items-center gap-1.5 font-mono text-[12px] text-danger', className)}>
      <CloudOff className="size-3.5 shrink-0" />
      <span className="truncate">Can't reach {name} now: this may be out of date</span>
    </div>
  )
}

/** A paired hub's name by its id, for a hub that may have been forgotten here since. */
const hubName = (id: string | undefined) => listHubs().find((h) => h.id === id)?.name ?? 'The hub'

/**
 * Lines over the list: each paired hub that refused this phone (forgotten there, so its pages are
 * gone until it pairs again), and each computer paired with the phone whose inyon is too old to
 * serve its pages.
 */
function NoticeLines({ className }: { className?: string }) {
  const forgotten = usePages((s) => s.forgotten)
  const outdated = usePages((s) => s.outdated)
  const machine = useMachines()
  return (
    <>
      {forgotten.map((id) => (
        <p key={id} role="status" className={clsx('flex items-start gap-1.5 text-[13px] text-fg-2', className)}>
          <Link2Off className="mt-[3px] size-3.5 shrink-0 text-fg-3" />
          <span>{hubName(id)} forgot this phone. Pair it again from the hub to see its pages here.</span>
        </p>
      ))}
      {outdated.map((host) => (
        <p key={host} role="status" className={clsx('flex items-start gap-1.5 text-[13px] text-fg-2', className)}>
          <CircleArrowUp className="mt-[3px] size-3.5 shrink-0 text-fg-3" />
          <span>Update inyon on {machine(host).name} to open its pages here.</span>
        </p>
      ))}
    </>
  )
}

function StateCard({ icon, title, children }: { icon: React.ReactNode; title: string; children: React.ReactNode }) {
  return (
    <div role="status" className="animate-fade-in m-4 w-full max-w-[440px] self-center rounded-3xl border border-line bg-elev p-6 shadow-card">
      <span className="flex size-10 items-center justify-center rounded-xl border border-line bg-sunken text-fg-2">{icon}</span>
      <h2 className="mt-3 text-[18px] font-semibold tracking-[-0.02em]">{title}</h2>
      <div className="mt-1.5 text-[14px] leading-relaxed text-fg-2">{children}</div>
    </div>
  )
}

/** The command an agent runs, with a copy button. */
function AddCommand() {
  return (
    <div className="flex items-center gap-2 rounded-xl border border-line bg-code py-1.5 pr-1.5 pl-3">
      <code className="no-scrollbar min-w-0 flex-1 overflow-x-auto font-mono text-[13px] whitespace-nowrap">{ADD_COMMAND}</code>
      <IconButton label="Copy command" className="size-10 md:size-8" onClick={() => void copyText(ADD_COMMAND).then((ok) => ok && toast({ title: 'Copied the command', tone: 'success' }))}>
        <Copy className="size-4" />
      </IconButton>
    </div>
  )
}

function NoPages({ wide }: { wide?: boolean }) {
  return (
    <div className={clsx('animate-fade-in', wide ? 'max-w-[460px] p-7' : 'rounded-3xl border border-line bg-elev p-5 shadow-card')}>
      {wide && <span className="mb-3 flex size-10 items-center justify-center rounded-xl border border-line bg-elev text-fg-2"><PanelsTopLeft className="size-[18px]" /></span>}
      <h2 className="text-[19px] font-semibold tracking-[-0.02em]">No pages yet</h2>
      <p className="mt-1.5 mb-4 text-[14px] leading-relaxed text-fg-2">When an agent builds a plan or mockups, it runs one command and the page shows up here{wide ? ', under its machine and project' : ''}.</p>
      <AddCommand />
      <div className="mt-4 flex flex-wrap gap-2">
        <Button variant={wide ? 'primary' : 'secondary'} size={wide ? 'md' : 'lg'} className={clsx(!wide && 'w-full')} onClick={() => openPageSheet({ kind: 'add' })}>
          <FolderPlus className="size-4" /> Add a folder
        </Button>
        {wide && (
          <a href={PAGES_DOCS} target="_blank" rel="noreferrer" className="press inline-flex h-9 items-center gap-1.5 rounded-full px-4 text-[13.5px] font-medium text-fg-2 hover:bg-hover hover:text-fg">
            <BookOpen className="size-4" /> How pages work ↗
          </a>
        )}
      </div>
    </div>
  )
}

function ListHeader({ right }: { right?: React.ReactNode }) {
  return (
    <header className="pt-safe sticky top-0 z-20 border-b border-line bg-bg">
      <div className="flex h-[52px] items-center gap-1 pr-1.5 pl-3 lg:px-7">
        <span className="md:hidden"><Logo /></span>
        <h1 className="min-w-0 flex-1 truncate px-1 text-[15px] font-semibold tracking-tight">Pages</h1>
        {right}
      </div>
    </header>
  )
}

// ---------- phone: the list ----------

function PhoneList() {
  const pages = usePages((s) => s.pages)
  const seen = usePages((s) => s.seen)
  const error = usePages((s) => s.error)
  const machine = useMachines()
  const [filtering, setFiltering] = useState(false)
  const [q, setQ] = useState('')
  const names = useRunningNames()
  const groups = useMemo(() => groupPages((pages ?? []).filter((p) => matchesPage(p, machine(p.host).name, q, names)), (h) => machine(h).name, names), [pages, q, machine, names])
  return (
    <PullRefresh onRefresh={loadPages} className="min-h-0 flex-1 overflow-y-auto">
      <ListHeader
        right={
          !!pages?.length && (
            <>
              <IconButton label="Filter pages" active={filtering} className="size-11" onClick={() => (setFiltering(!filtering), setQ(''))}><Search className="size-[18px]" /></IconButton>
              <IconButton label="Add a folder" className="size-11" onClick={() => openPageSheet({ kind: 'add' })}><FolderPlus className="size-[18px]" /></IconButton>
            </>
          )
        }
      />
      <div className="mx-auto w-full max-w-[720px] px-3 pt-2 pb-8">
        {filtering && <FilterInput value={q} onChange={setQ} placeholder="Filter pages" className="my-1.5 h-11" />}
        {error && <p role="alert" className="mx-1 my-2 text-[13px] text-danger">{error}</p>}
        <NoticeLines className="mx-1 my-2" />
        {pages === null ? (
          <Waiting shape="cards" count={2} label="Loading pages" className="pt-3" />
        ) : !pages.length ? (
          <div className="pt-2"><NoPages /></div>
        ) : !groups.length ? (
          <div className="px-4 py-8 text-center text-[13px] text-fg-3">
            No page matches “{q}”.
            <div className="mt-3"><Button onClick={() => setQ('')}>Clear filter</Button></div>
          </div>
        ) : (
          groups.map((g) => {
            const m = machine(g.host)
            return (
              <section key={g.host} className="mt-4 first:mt-1.5">
                <MachineHeading m={m} />
                {g.projects.map((pr) => (
                  <div key={pr.key}>
                    <div className="mx-1 mt-3 mb-2 font-mono text-[12px] text-fg-3">{pr.name}</div>
                    <div className="space-y-2">
                      {pr.pages.map((p) => <PageCard key={p.id} p={p} m={m} changed={isChanged(p, seen)} />)}
                    </div>
                  </div>
                ))}
              </section>
            )
          })
        )}
      </div>
    </PullRefresh>
  )
}

function MachineHeading({ m, small }: { m: PageMachine; small?: boolean }) {
  return (
    <div className={clsx('flex items-center gap-2 font-semibold', small ? 'h-8 px-2 text-[13px]' : 'mx-1 text-[15px] tracking-[-0.01em]')}>
      <MachineIcon kind={m.host ? machineKindOf(m.host) : 'server'} size={small ? 14 : 16} className="shrink-0 text-fg-2" />
      <span className="min-w-0 truncate">{m.name}</span>
      {m.hub && <span className="inline-flex h-5 shrink-0 items-center rounded-full border border-line bg-white/45 px-2 font-mono text-[12px] font-normal text-fg-2 dark:bg-white/[0.03]">hub</span>}
      {!m.online && <span className="ml-auto inline-flex shrink-0 items-center gap-1 font-mono text-[12px] font-normal text-danger"><CloudOff className="size-3.5" /> offline</span>}
    </div>
  )
}

function PageCard({ p, m, changed }: { p: Page; m: PageMachine; changed: boolean }) {
  const nav = useNavigate()
  const items = () => pageMenu(p, m, nav)
  const open = () => (m.online ? nav(pageHref(p.host, p.slug)) : toast({ title: `${m.name} is offline`, body: 'This page opens when it’s back.', tone: 'warn' }))
  return (
    <div
      role="link"
      tabIndex={0}
      aria-label={pageLabel(p, { changed, online: m.online })}
      onClick={open}
      onKeyDown={(e) => e.key === 'Enter' && e.target === e.currentTarget && open()}
      {...contextMenu(items, { touch: true, header: p.name })}
      className="press block cursor-pointer rounded-3xl border border-line bg-elev py-3.5 pr-1.5 pl-4 shadow-card active:scale-[0.985] active:bg-hover"
    >
      <div className={clsx('-mt-2.5 flex items-center gap-2', !m.online && 'opacity-50')}>
        <b className="min-w-0 truncate text-[16px] font-semibold tracking-[-0.015em]">{p.name}</b>
        {changed && <ChangedDot />}
        <span className="ml-auto" onClick={(e) => e.stopPropagation()}>
          <Menu align="end" header={p.name} items={items()} trigger={({ toggle, ref }) => <IconButton label={`More for ${p.name}`} className="size-11" onClick={toggle} ref={ref}><Ellipsis className="size-[18px]" /></IconButton>} />
        </span>
      </div>
      {p.progress && (
        <div className={clsx(!m.online && 'opacity-50')}>
          <div className="mt-0.5 mr-2.5 flex items-center gap-2.5">
            <ProgressBar done={p.progress.done} total={p.progress.total} className="h-[3px] flex-1" />
            <span className="tabular font-mono text-[12px] text-fg-2">{p.progress.done}/{p.progress.total}</span>
          </div>
          {p.progress.now && <p className="mt-2 mr-2.5 line-clamp-2 text-[14px] leading-snug text-fg-2">{p.progress.now}</p>}
        </div>
      )}
      <div className={clsx('mt-2 flex items-center gap-1.5 font-mono text-[12px]', m.online ? 'text-fg-3' : 'text-fg-2')}>
        {m.online ? updatedLine(p) : <><CloudOff className="size-3.5" /> Machine offline</>}
      </div>
    </div>
  )
}

// ---------- phone: the viewer ----------

function PhoneViewer({ id }: { id: string }) {
  const page = usePages((s) => s.pages?.find((p) => p.id === id))
  const loaded = usePages((s) => s.pages !== null)
  // a page is gone only when its hub answered without it; a hub out of reach keeps its pages
  const unreached = usePages((s) => hubUnreachable(id, s.unreachable))
  const forgot = usePages((s) => hubForgot(id, s.forgotten))
  const all = usePages((s) => s.pages)
  const machine = useMachines()
  const nav = useNavigate()
  // back closes the viewer; it never walks back through the pages opened inside the frame
  const back = () => nav('/pages', { replace: true })
  useBackCloses(true, back)
  const v = useViewer(page)
  const m = page ? machine(page.host) : undefined
  const siblings = page ? (all ?? []).filter((p) => p.host === page.host) : []
  const more: MenuItem[] = page
    ? [
        { key: 'reload', label: 'Reload', icon: <RotateCw className="size-4" />, disabled: !v.link, onSelect: v.reload },
        ...(v.parts.length > 1 ? v.parts.map((pt, i) => ({ key: pt, label: PART_LABEL[pt], icon: PART_ICON[pt], selected: !v.note && v.part === pt, separated: i === 0, disabled: !v.link, onSelect: () => v.setPart(pt) })) : []),
        ...(v.bridged ? [] : [{ key: 'browser', label: 'Open in browser', hint: BROWSER_HINT, icon: <ExternalLink className="size-4" />, separated: true, disabled: !v.link, onSelect: () => void openInBrowser(page) }]),
      ]
    : []
  const machineItems: MenuItem[] = page && m
    ? [
        ...siblings.map((p) => ({ key: p.id, label: p.name, icon: <FileText className="size-4" />, selected: p.id === page.id, onSelect: () => nav(pageHref(p.host, p.slug), { replace: true }) })),
        ...(m.host ? [{ key: 'machine', label: `Open ${m.name}`, icon: <MachineIcon kind={machineKindOf(m.host)} size={16} />, separated: true, onSelect: () => nav(`/machines/${encodeURIComponent(m.host!.config.id)}`) }] : []),
      ]
    : []
  return (
    <div className="animate-page flex min-h-0 flex-1 flex-col bg-bg">
      <header className="pt-safe shrink-0 border-b border-line bg-bg">
        <div className="flex h-11 items-center gap-0.5 px-0.5">
          <IconButton label="Back to Pages" className="size-11 rounded-xl" onClick={back}><ChevronLeft className="size-5" /></IconButton>
          <h1 className="min-w-0 flex-1 truncate pl-0.5 text-[15px] font-semibold tracking-[-0.01em]">{page?.name ?? ''}</h1>
          {m && (
            <Menu
              align="end"
              header={m.name}
              items={machineItems}
              trigger={({ toggle, ref }) => (
                <button ref={ref} type="button" onClick={toggle} aria-label={`Machine: ${m.name}`} className="press flex h-[30px] max-w-[132px] shrink-0 items-center gap-1.5 rounded-full border border-line bg-white/45 px-2.5 font-mono text-[12px] text-fg-2 dark:bg-white/[0.03]">
                  <MachineIcon kind={m.host ? machineKindOf(m.host) : 'server'} size={14} className="shrink-0" />
                  <span className="truncate">{m.name}</span>
                </button>
              )}
            />
          )}
          {!!page?.notes?.length && (
            <Menu
              align="end"
              header="Notes"
              items={notesMenu(page, v)}
              trigger={({ toggle, ref }) => (
                <IconButton label={v.note ? `Notes: ${noteTitle(v.note)} open` : 'Notes'} className={clsx('size-11 rounded-xl', v.note && 'text-fg')} onClick={toggle} ref={ref}><NotebookText className="size-[18px]" /></IconButton>
              )}
            />
          )}
          {page && <Menu align="end" items={more} trigger={({ toggle, ref }) => <IconButton label="More" className="size-11 rounded-xl" onClick={toggle} ref={ref}><Ellipsis className="size-[18px]" /></IconButton>} />}
        </div>
        {/* always on screen, so a page can't pass itself off as the app asking for a password */}
        <div className="flex h-6 items-center gap-1.5 border-t border-line px-3 font-mono text-[12px] text-fg-3">
          <ShieldCheck className="size-3.5 shrink-0" />
          <span className="truncate">{SANDBOX_NOTE}</span>
        </div>
        {v.stale && m && <StaleLine name={m.name} className="h-6 border-t border-line px-3" />}
        {v.note && (
          <div className="flex h-9 items-center gap-1.5 border-t border-line pr-1 pl-3 text-[13px]">
            <NotebookText className="size-3.5 shrink-0 text-fg-3" />
            <span className="min-w-0 flex-1 truncate">{noteTitle(v.note)}</span>
            <button type="button" onClick={() => v.setPart(v.part)} className="press h-9 shrink-0 rounded-lg px-2.5 font-medium text-fg-2">Back to {PART_LABEL[v.part]}</button>
          </div>
        )}
      </header>
      <div className="relative flex min-h-0 flex-1 flex-col">
        {!page ? (
          forgot ? (
            <StateCard icon={<Link2Off className="size-5" />} title={`${hubName(hubIdOf(id))} forgot this phone`}>
              Pair it again from the hub to open its pages here.
              <div className="mt-4"><Button size="sm" onClick={back}><ChevronLeft className="size-3.5" /> Back to Pages</Button></div>
            </StateCard>
          ) : unreached ? (
            <StateCard icon={<CloudOff className="size-5 text-danger" />} title="The hub is out of reach">
              This page opens again as soon as its hub answers.
              <div className="mt-4"><Button size="sm" onClick={() => void loadPages()}><RotateCw className="size-3.5" /> Try again</Button></div>
            </StateCard>
          ) : loaded ? (
            <StateCard icon={<FolderOpen className="size-5" />} title="This page is gone">
              It isn't on the list any more.
              <div className="mt-4"><Button size="sm" onClick={back}><ChevronLeft className="size-3.5" /> Back to Pages</Button></div>
            </StateCard>
          ) : (
            <Waiting shape="panel" count={2} label="Opening" className="p-5" />
          )
        ) : v.src && (m!.online || v.stale) ? (
          <PageFrame key={v.frameKey} src={v.src} bridge={v.bridge} title={page.name} className="absolute inset-0 size-full border-0 bg-bg" />
        ) : (
          <FrameState page={page} machine={m!} error={v.error} onRetry={() => void v.retry()} onBack={back} />
        )}
      </div>
    </div>
  )
}

// ---------- desktop: list and viewer side by side ----------

function PagesDesktop({ selected }: { selected?: string }) {
  const pages = usePages((s) => s.pages)
  const forgot = usePages((s) => !!selected && hubForgot(selected, s.forgotten))
  const seen = usePages((s) => s.seen)
  const error = usePages((s) => s.error)
  const machine = useMachines()
  const nav = useNavigate()
  const [q, setQ] = useState('')
  const [shut, setShut] = useState<ReadonlySet<string>>(() => new Set())
  const filter = useRef<HTMLDivElement>(null)
  const page = pages?.find((p) => p.id === selected)
  const names = useRunningNames()
  const all = useMemo(() => groupPages(pages ?? [], (h) => machine(h).name, names), [pages, machine, names])
  const groups = useMemo(() => (q ? groupPages((pages ?? []).filter((p) => matchesPage(p, machine(p.host).name, q, names)), (h) => machine(h).name, names) : all), [all, pages, q, machine, names])

  // nothing picked: open the page that changed last (or the first) instead of an empty pane
  useEffect(() => {
    if (selected || !pages?.length) return
    const first = all.flatMap((g) => g.projects.flatMap((p) => p.pages))
    const pick = first.find((p) => isChanged(p, seen)) ?? first[0]
    if (pick) nav(pageHref(pick.host, pick.slug), { replace: true })
  }, [selected, pages?.length])

  // "/" jumps to the filter
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.key !== '/' || (e.target as HTMLElement)?.closest?.('input, textarea, [contenteditable]')) return
      e.preventDefault()
      filter.current?.querySelector('input')?.focus()
    }
    window.addEventListener('keydown', on)
    return () => window.removeEventListener('keydown', on)
  }, [])

  const toggle = (key: string) => setShut((s) => { const n = new Set(s); if (n.has(key)) n.delete(key); else n.add(key); return n })

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[280px_minmax(0,1fr)]">
      <section aria-label="Pages" className="pt-safe flex min-h-0 flex-col border-r border-line bg-bg">
        <div className="flex h-[52px] shrink-0 items-center gap-0.5 pr-2 pl-4">
          <h1 className="flex-1 text-[18px] font-semibold tracking-[-0.03em]">Pages</h1>
          <IconButton label="Add a folder" onClick={() => openPageSheet({ kind: 'add' })}><FolderPlus className="size-4" /></IconButton>
        </div>
        {!!pages?.length && (
          <div ref={filter} className="shrink-0 px-3 pb-2">
            <FilterInput value={q} onChange={setQ} placeholder="Filter" className="h-8" />
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto px-2 pt-0.5 pb-3">
          {error && <p role="alert" className="px-2 py-2 text-[13px] text-danger">{error}</p>}
          <NoticeLines className="px-2 py-2" />
          {pages === null ? (
            <Waiting shape="rows" label="Loading pages" className="px-1" />
          ) : !pages.length ? (
            <div className="px-3 py-4 text-[13px] text-fg-3">Nothing here yet.</div>
          ) : !groups.length ? (
            <div className="px-3 py-4 text-[13px] text-fg-3">
              No page matches “{q}”.
              <div className="mt-2.5"><Button size="sm" onClick={() => setQ('')}>Clear filter</Button></div>
            </div>
          ) : (
            groups.map((g) => {
              const m = machine(g.host)
              return (
                <div key={g.host} className="mt-1.5 first:mt-0">
                  <MachineHeading m={m} small />
                  {g.projects.map((pr) => {
                    const closed = shut.has(pr.key) && !q
                    return (
                      <div key={pr.key}>
                        <button type="button" aria-expanded={!closed} onClick={() => toggle(pr.key)} className="flex h-[30px] w-full items-center gap-1.5 rounded-xl pr-2 pl-3 text-left text-[13px] text-fg-2 hover:bg-hover">
                          {closed ? <ChevronRight className="size-3.5 text-fg-3" /> : <ChevronDown className="size-3.5 text-fg-3" />}
                          <span className="min-w-0 flex-1 truncate">{pr.name}</span>
                          <span className="font-mono text-[12px] text-fg-3">{pr.pages.length}</span>
                        </button>
                        {!closed && pr.pages.map((p) => <TreeRow key={p.id} p={p} m={m} active={p.id === selected} changed={isChanged(p, seen)} />)}
                      </div>
                    )
                  })}
                </div>
              )
            })
          )}
        </div>
        {/* the empty viewer shows the whole command already */}
        {!!pages?.length && (
          <div className="shrink-0 border-t border-line px-4 py-2.5 text-[12px] text-fg-3">
            Agents add pages with <code className="font-mono text-[12px] text-fg-2">inyon pages add</code>
          </div>
        )}
      </section>
      <section aria-label="Viewer" className="pt-safe flex min-h-0 min-w-0 flex-col">
        {forgot ? (
          <div className="flex flex-1 items-center justify-center"><Empty icon={<Link2Off className="size-5" />} title={`${hubName(hubIdOf(selected!))} forgot this phone`}>Pair it again from the hub to open its pages here.</Empty></div>
        ) : pages && !pages.length ? (
          <div className="flex flex-1 items-center justify-center bg-sunken/60"><NoPages wide /></div>
        ) : page ? (
          <DesktopViewer key={page.id} page={page} m={machine(page.host)} />
        ) : pages && selected ? (
          <div className="flex flex-1 items-center justify-center"><Empty icon={<FolderOpen className="size-5" />} title="This page is gone">It isn't on the list any more. Pick another on the left.</Empty></div>
        ) : (
          <div className="flex-1" />
        )}
      </section>
    </div>
  )
}

function TreeRow({ p, m, active, changed }: { p: Page; m: PageMachine; active: boolean; changed: boolean }) {
  const nav = useNavigate()
  return (
    <Link
      to={pageHref(p.host, p.slug)}
      // the open page clicked again opens with a new link, as Reload does
      onClick={() => active && renewLink(p.id)}
      aria-current={active ? 'page' : undefined}
      aria-label={pageLabel(p, { changed, online: m.online })}
      {...contextMenu(() => pageMenu(p, m, nav), { header: p.name })}
      className={clsx('grid min-h-[34px] grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-xl pr-2 pl-8 text-[13.5px] transition-colors', active ? 'bg-accent-soft' : 'hover:bg-hover')}
    >
      <span className={clsx('flex min-w-0 items-center gap-1.5', !m.online && 'opacity-55')}>
        <span className="truncate">{p.name}</span>
        {changed && <ChangedDot />}
      </span>
      <span className={clsx('tabular flex items-center gap-1.5 font-mono text-[12px] text-fg-3', !m.online && 'opacity-55')}>
        {p.progress ? (
          <>
            <ProgressBar done={p.progress.done} total={p.progress.total} className="h-[3px] w-8" />
            {p.progress.done}/{p.progress.total}
          </>
        ) : null}
      </span>
    </Link>
  )
}

const DEVICE_OPTIONS: { v: Device; label: React.ReactNode; name: string }[] = [
  { v: 'fit', label: 'Fit', name: 'Fit the pane' },
  { v: 'phone', label: <Smartphone className="size-[15px]" />, name: 'Phone 390 × 844' },
  { v: 'tablet', label: <Tablet className="size-[15px]" />, name: 'Tablet 820 × 1180' },
  { v: 'desktop', label: <Monitor className="size-[15px]" />, name: 'Desktop 1440 × 900' },
]

function DesktopViewer({ page, m }: { page: Page; m: PageMachine }) {
  const names = useRunningNames()
  const nav = useNavigate()
  const v = useViewer(page)
  const [device, setDevice] = useState<Device>('fit')
  const stage = useRef<HTMLDivElement>(null)
  const [box, setBox] = useState({ w: 0, h: 0 })
  useEffect(() => {
    const el = stage.current
    if (!el) return
    const measure = () => setBox({ w: el.clientWidth, h: el.clientHeight })
    measure()
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : undefined
    ro?.observe(el)
    return () => ro?.disconnect()
  }, [])
  const size = device === 'fit' ? undefined : DEVICES[device]
  const scale = size ? fitScale(size.w, size.h, box.w, box.h) : 1
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000)
    return () => clearInterval(t)
  }, [])
  const menu: MenuItem[] = [
    { key: 'revoke', label: 'Revoke links', icon: <Link2Off className="size-4" />, onSelect: () => void revokeLinks(page) },
    { key: 'copy', label: 'Copy folder path', icon: <Copy className="size-4" />, onSelect: () => void copyFolder(page) },
    { key: 'folder', label: 'Show folder', icon: <FolderOpen className="size-4" />, onSelect: () => openPageSheet({ kind: 'folder', page }) },
    { key: 'remove', label: 'Remove', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void removePage(page, m.name, () => nav('/pages', { replace: true })) },
  ]
  return (
    <>
      <div className="flex h-[52px] shrink-0 items-center gap-2 border-b border-line pr-2.5 pl-4">
        <nav aria-label="Page location" className="flex min-w-0 flex-1 items-center gap-1.5 text-[13px] whitespace-nowrap text-fg-2">
          <span className="inline-flex shrink-0 items-center gap-1.5">
            <MachineIcon kind={m.host ? machineKindOf(m.host) : 'server'} size={14} className="text-fg-3" />
            {m.name}
          </span>
          <span className="text-fg-3">/</span>
          <span className="truncate">{groupName(page, names)}</span>
          <span className="text-fg-3">/</span>
          <b className="truncate font-semibold text-fg">{page.name}</b>
        </nav>
        {v.parts.length > 1 && <Segmented size="sm" label="Part of the page" value={v.note ? ('' as Part) : v.part} onChange={v.setPart} options={v.parts.map((p) => ({ v: p, label: PART_LABEL[p] }))} />}
        {!!page.notes?.length && (
          <Menu
            align="end"
            header="Notes"
            items={notesMenu(page, v)}
            trigger={({ toggle, ref }) => (
              <button
                ref={ref}
                type="button"
                onClick={toggle}
                aria-label={v.note ? `Notes: ${noteTitle(v.note)} open` : 'Notes'}
                className={clsx('press flex h-[30px] max-w-[180px] shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-[12px] font-medium', v.note ? 'border-line-strong bg-elev text-fg shadow-card' : 'border-line text-fg-2 hover:text-fg')}
              >
                <NotebookText className="size-3.5 shrink-0" />
                <span className="truncate">{v.note ? noteTitle(v.note) : 'Notes'}</span>
                <ChevronDown className="size-3.5 shrink-0 text-fg-3" />
              </button>
            )}
          />
        )}
        <Segmented size="sm" label="Frame size" value={device} onChange={setDevice} options={DEVICE_OPTIONS} />
        <IconButton label="Reload" disabled={!v.link} onClick={v.reload}><RotateCw className="size-4" /></IconButton>
        {/* a fresh 12 h link each time; the tab sits at the hub's address, which is never a sign-in page */}
        {!v.bridged && (
          <IconButton label="Open in new tab" title="Open in new tab. A 12 h link; never sign in there." disabled={!v.link} onClick={() => void openInBrowser(page)}>
            <ExternalLink className="size-4" />
          </IconButton>
        )}
        <Menu align="end" items={menu} header={page.name} trigger={({ toggle, ref }) => <IconButton label={`More for ${page.name}`} onClick={toggle} ref={ref}><Ellipsis className="size-4" /></IconButton>} />
      </div>
      <div ref={stage} className={clsx('relative flex min-h-0 flex-1 overflow-hidden bg-sunken/60', size ? 'items-start justify-center p-3' : 'p-3')}>
        {v.src && (m.online || v.stale) ? (
          size ? (
            <div className="relative shrink-0" style={{ width: size.w * scale, height: size.h * scale }}>
              <div className="animate-pop absolute top-0 left-0 origin-top-left overflow-hidden rounded-xl border border-line-strong bg-bg shadow-soft" style={{ width: size.w, height: size.h, transform: `scale(${scale})` }}>
                <PageFrame key={v.frameKey} src={v.src} bridge={v.bridge} title={page.name} className="block size-full border-0" />
              </div>
            </div>
          ) : (
            <div className="animate-pop size-full overflow-hidden rounded-xl border border-line-strong bg-bg shadow-soft">
              <PageFrame key={v.frameKey} src={v.src} bridge={v.bridge} title={page.name} className="block size-full border-0" />
            </div>
          )
        ) : (
          <div className="flex size-full items-center justify-center">
            <FrameState page={page} machine={m} error={v.error} onRetry={() => void v.retry()} onBack={() => nav('/pages', { replace: true })} />
          </div>
        )}
      </div>
      <div className="flex h-8 shrink-0 items-center gap-2 overflow-hidden border-t border-line px-4 font-mono text-[12px] whitespace-nowrap text-fg-3">
        <ShieldCheck className="size-3.5 shrink-0" />
        <span className="truncate">{SANDBOX_NOTE}{v.link && !v.stale ? ` · link lasts ${leftText(v.link.until, now)}` : ''}</span>
        {v.stale && <StaleLine name={m.name} className="shrink-0" />}
        <span className="ml-auto shrink-0">{size ? `${size.name} · ${size.w} × ${size.h}${scale < 1 ? ` · ${Math.round(scale * 100)}%` : ''}` : box.w ? `Fit · ${Math.max(0, box.w - 24)} × ${Math.max(0, box.h - 24)}` : ''}</span>
      </div>
    </>
  )
}
