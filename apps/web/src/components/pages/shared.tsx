import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, type NavigateFunction } from 'react-router-dom'
import { create } from 'zustand'
import clsx from 'clsx'
import { ChevronRight, Copy, ExternalLink, FolderOpen, Link2Off, PanelsTopLeft, Trash2 } from 'lucide-react'
import type { HostState } from '@inyon/protocol'
import { api, ApiError } from '../../lib/api.ts'
import { toast, useStore } from '../../lib/store.ts'
import { copyText } from '../../lib/clipboard.ts'
import { isNative, useFramesIsolated } from '../../lib/native.ts'
import { FrameRefused } from '../FrameRefused.tsx'
import { splitHostId } from '../../lib/hubs.ts'
import {
  PART_PATH, absoluteLink, agoText, changedAt, dropPage, frameMessage, hubUnreachable, isBridged, isChanged, isHubMachine, loadPages, localDeadline, openLink, pageBase, pageHosts, pageHref, projectName,
  RENEW_RETRY_MS, nextRetry, renewIn, runningNames, shouldRenew, usePages, type Page, type PageLink, type Part,
} from '../../lib/pages.ts'
import { Button, Field, SectionLabel, Sheet, inputClass, type MenuItem } from '../ui/primitives.tsx'
import { contextMenu } from '../ui/ContextMenu.tsx'
import { Select } from '../ui/Select.tsx'
import { confirmAction } from '../Confirm.tsx'
import { useOpened } from '../../lib/useOpened.ts'
import { publicFile } from '../../lib/base.ts'
import { bridgeFrame, virtualSrc } from '../../lib/pageBridge.ts'

/** The docs page about Pages, for "How pages work". */
export const PAGES_DOCS = 'https://inyon.tech/docs/pages/'
/** The one command an agent runs to add a page, as the empty states show it. */
export const ADD_COMMAND = 'inyon pages add ./private/plans/my-plan --name "My plan"'

// ---------- machines ----------

/** What the lists need to know about a page's machine. */
export interface PageMachine {
  name: string
  online: boolean
  /** the hub's own machine */
  hub: boolean
  host?: HostState
}

/**
 * The machine a page is on, by the hosts the app knows; a hub's own machine is "This hub" until its
 * name arrives. A paired hub that didn't answer the last time the list was asked for is offline.
 */
export function machineOf(hosts: HostState[], host: string, unreachable: string[] = []): PageMachine {
  const h = hosts.find((x) => x.config.id === host)
  const hub = isHubMachine(host)
  const reached = !hubUnreachable(host, unreachable)
  if (!h) return { name: hub ? (splitHostId(host).hub?.name ?? 'This hub') : host, online: reached, hub }
  return { name: h.config.name, online: reached && h.status === 'online', hub: h.config.kind === 'local' || hub, host: h }
}

/** The names running sessions have now, for naming page groups (see runningNames). */
export function useRunningNames() {
  const hosts = useStore((s) => s.hosts)
  return useMemo(() => runningNames(hosts), [hosts])
}

/** A function from host id to its machine, for the current list of hosts. */
export function useMachines() {
  const hosts = useStore((s) => s.hosts)
  const unreachable = usePages((s) => s.unreachable)
  return useCallback((host: string) => machineOf(hosts, host, unreachable), [hosts, unreachable])
}

// ---------- progress ----------

/** A thin done/total bar; it moves with transform only. */
export function ProgressBar({ done, total, className }: { done: number; total: number; className?: string }) {
  const f = total > 0 ? Math.min(1, done / total) : 0
  return (
    <div role="progressbar" aria-label={`${done} of ${total} done`} aria-valuemin={0} aria-valuemax={total} aria-valuenow={done} className={clsx('overflow-hidden rounded-full bg-line', className ?? 'h-[3px]')}>
      <div className="h-full origin-left rounded-full bg-fg transition-transform duration-(--dur-move) ease-out-soft" style={{ transform: `scaleX(${f})` }} />
    </div>
  )
}

/** "updated 3 min ago" by the board's own date, or when it was added when it has no board (or no readable date). */
export function updatedLine(p: Page, now = Date.now()) {
  const t = p.progress?.updated ? Date.parse(p.progress.updated) : NaN
  return Number.isFinite(t) ? `updated ${agoText(t, now)}` : `added ${agoText(p.added, now)}`
}

/** A page's name for a screen reader, with what the card shows of it. */
export function pageLabel(p: Page, o: { changed: boolean; online: boolean }, now = Date.now()) {
  const parts = [p.name]
  if (o.changed) parts.push('changed')
  if (p.progress) parts.push(`${p.progress.done} of ${p.progress.total} done`)
  if (p.progress?.now) parts.push(p.progress.now)
  parts.push(o.online ? updatedLine(p, now) : 'machine offline')
  return parts.join(', ')
}

/** The changed-since-you-opened-it dot. */
export const ChangedDot = () => <span role="img" aria-label="changed" title="Changed since you last opened it" className="animate-pop size-1.5 shrink-0 rounded-full bg-fg" />

/** What every viewer says about the page in it: a sandbox, and never a place to sign in. */
export const SANDBOX_NOTE = 'Sandboxed page: never sign in here'

// ---------- links ----------

/** Bumped by Revoke (and by opening the page that's open already), so an open viewer mints a new link at once. */
const useLinkRev = create<Record<string, number>>(() => ({}))

/** Mints a new link for a page an open viewer shows, as Reload does. */
export const renewLink = (id: string) => useLinkRev.setState((s) => ({ [id]: (s[id] ?? 0) + 1 }))

/**
 * A page's link: minted when the page opens, and again an hour before it runs out, by a timer while
 * the page is on screen and on coming back into view after that. Each new link reloads the frame.
 * Its end is counted on this device's clock (`until`).
 */
export function usePageLink(page: Pick<Page, 'id' | 'host' | 'slug'> | undefined) {
  const rev = useLinkRev((s) => (page ? (s[page.id] ?? 0) : 0))
  const [state, setState] = useState<{ id: string; link?: PageLink; error?: ApiError | Error }>()
  const id = page?.id
  // the page shown now: a mint that answers after another page was picked is dropped
  const shown = useRef(id)
  shown.current = id
  /** Asks for a new link; true when one came. */
  const mint = useCallback(async () => {
    if (!page) return false
    const asked = page.id
    const at = Date.now()
    try {
      const link = localDeadline(await openLink(page), at)
      if (shown.current !== asked) return false
      setState({ id: asked, link })
      return true
    } catch (e) {
      if (shown.current === asked) setState((s) => ({ id: asked, link: s?.id === asked ? s.link : undefined, error: e as Error }))
      return false
    }
  }, [id])
  useEffect(() => {
    if (!id) return
    setState((s) => (s?.id === id ? { id, link: s.link } : { id }))
    void mint()
  }, [id, rev, mint])
  // renewed on time by a timer, since a page left on screen (a second monitor, a tablet on a stand)
  // gets no focus event, and on coming back into view after that. A failed try (the hub restarting,
  // the phone off the VPN), whichever started it, is tried again with a growing wait while the page
  // stays on screen; a new link moves `until`, which starts this over
  const mine = state?.id === id ? state : undefined
  const until = mine?.link?.until
  useEffect(() => {
    if (until === undefined) return
    let stopped = false
    let busy = false
    let wait = RENEW_RETRY_MS
    let t: ReturnType<typeof setTimeout> | undefined
    const renew = async () => {
      clearTimeout(t)
      if (stopped || busy || document.visibilityState !== 'visible') return // coming back into view renews it
      busy = true
      const ok = await mint()
      busy = false
      if (ok || stopped) return
      t = setTimeout(renew, wait)
      wait = nextRetry(wait)
    }
    const check = () => {
      if (document.visibilityState === 'visible' && shouldRenew(until, Date.now())) void renew()
    }
    t = setTimeout(renew, renewIn(until, Date.now()))
    window.addEventListener('focus', check)
    document.addEventListener('visibilitychange', check)
    return () => {
      stopped = true
      clearTimeout(t)
      window.removeEventListener('focus', check)
      document.removeEventListener('visibilitychange', check)
    }
  }, [until, mint])
  return { link: mine?.link, error: mine?.error, retry: mint }
}

/**
 * Which parts of a Blueprint site exist (the plan always does), asked of the hub with each new link.
 * What was found stays shown while a renewed link is asked again.
 */
export function useSiteParts(page: Pick<Page, 'id' | 'host'> | undefined, link: PageLink | undefined): Part[] {
  const [found, setFound] = useState<{ id: string; parts: Part[] }>()
  useEffect(() => {
    if (!page || !link) return
    let live = true
    const base = pageBase(page.host)
    Promise.all(
      (['mockups', 'matrix'] as const).map(async (part) => {
        try {
          // a page on a computer paired with the phone: the phone core answers its link
          const r = isBridged(page.host)
            ? await api.pageFile(`${link.url}${PART_PATH[part]}`, 'HEAD')
            : await fetch(`${base}${link.url}${PART_PATH[part]}`, { method: 'HEAD', credentials: 'omit', referrerPolicy: 'no-referrer' })
          return r.ok ? part : undefined
        } catch {
          return undefined
        }
      }),
    ).then((list) => live && setFound({ id: page.id, parts: ['plan', ...list.filter((p) => p !== undefined)] }))
    return () => void (live = false)
  }, [page?.id, link?.url])
  return found && found.id === page?.id ? found.parts : ['plan']
}

/** The flags of every page frame, which match the hub's own CSP sandbox. */
const SANDBOX = 'allow-scripts allow-forms allow-popups allow-downloads'

/**
 * The page in its frame. The sandbox matches the hub's own CSP sandbox: scripts run, on an opaque
 * origin, with no way at the app's cookie, storage or DOM. The theme goes in with ?theme= in the
 * address, and as a message when the app's theme changes or the frame loads a new document.
 *
 * With `bridge` (the link, `/v/<token>/`, of a page on a computer paired with the phone) the frame
 * holds the app's page viewer instead, in the same sandbox, and the app answers it with the
 * link's files (lib/pageBridge.ts).
 */
export function PageFrame({ src, bridge, title, className, style }: { src: string; bridge?: string; title: string; className?: string; style?: React.CSSProperties }) {
  const ref = useRef<HTMLIFrameElement>(null)
  const theme = useStore((s) => s.resolvedTheme)
  const themeNow = useRef(theme)
  themeNow.current = theme
  const post = useCallback(() => {
    try {
      ref.current?.contentWindow?.postMessage({ type: 'theme', value: themeNow.current }, '*')
    } catch {}
  }, [])
  useEffect(post, [theme])
  useEffect(() => {
    const on = (e: MessageEvent) => {
      const m = frameMessage(e, ref.current?.contentWindow)
      if (m?.type === 'theme?') post()
    }
    window.addEventListener('message', on)
    return () => window.removeEventListener('message', on)
  }, [post])
  useEffect(() => {
    const win = ref.current?.contentWindow
    if (!bridge || !win) return
    return bridgeFrame({ win, base: bridge, url: virtualSrc(src, bridge), theme: themeNow.current, fetch: api.pageFile })
  }, [bridge, src])
  const isolated = useFramesIsolated()
  if (!isolated) return <FrameRefused src={bridge ? undefined : src} className={className} style={style} />
  return <iframe ref={ref} src={bridge ? publicFile('pageview.html') : src} title={title} sandbox={SANDBOX} referrerPolicy="no-referrer" allow="" onLoad={post} className={className} style={style} />
}


// ---------- what can be done to a page ----------

/**
 * Opens a page outside the app, with a new link so it lasts the full 12 hours. A browser blocks a
 * window opened after a wait, so one is opened first and sent to the link once it is minted; the
 * Android app hands the link to the browser.
 */
export async function openInBrowser(p: Page) {
  const pending = !isNative() ? window.open('', '_blank') : null
  if (pending) pending.opener = null
  try {
    const l = await openLink(p)
    const url = absoluteLink(p.host, l.url)
    if (pending) pending.location.href = url
    else window.open(url, '_blank', 'noopener')
  } catch (e) {
    pending?.close()
    toast({ title: `Couldn't open “${p.name}”`, body: (e as Error).message, tone: 'error' })
  }
}

/** Asks, then ends every link to a page; an open viewer gets a new one right away. */
export async function revokeLinks(p: Page) {
  const ok = await confirmAction({
    title: `Revoke links to “${p.name}”?`,
    body: 'Every link to this page stops working: tabs on other devices, links you copied, and the one in an open viewer, which gets a new one right away.',
    yes: 'Revoke links',
    danger: true,
  })
  if (!ok) return
  try {
    await api.revokePage(p.host, p.slug)
    renewLink(p.id)
    toast({ title: 'Links revoked', body: `Old links to “${p.name}” stop working.`, tone: 'success' })
  } catch (e) {
    toast({ title: "Couldn't revoke the links", body: (e as Error).message, tone: 'error' })
  }
}

/** Takes a page off the list, with Undo; the folder stays where it is. */
export async function removePage(p: Page, machine: string, after?: () => void) {
  try {
    await api.removePage(p.host, p.slug)
    dropPage(p.id)
    after?.()
    toast({
      title: `Removed “${p.name}”`,
      body: `The folder stays on ${machine}.`,
      tone: 'success',
      action: {
        label: 'Undo',
        onSelect: () =>
          void api.addPage(p.host, { path: p.folder, name: p.name, slug: p.slug }).then(
            () => loadPages(),
            (e) => toast({ title: `Couldn't put “${p.name}” back`, body: (e as Error).message, tone: 'error' }),
          ),
      },
    })
  } catch (e) {
    toast({ title: `Couldn't remove “${p.name}”`, body: (e as Error).message, tone: 'error' })
  }
}

export const copyFolder = (p: Page) => copyText(p.folder).then((ok) => ok && toast({ title: 'Copied the folder path', tone: 'success' }))

/** The sheets the page menus open, mounted once (in the shell). */
type PageSheet = { kind: 'folder'; page: Page } | { kind: 'add' }
const usePageSheet = create<{ sheet?: PageSheet; open: boolean }>(() => ({ open: false }))
export const openPageSheet = (sheet: PageSheet) => usePageSheet.setState({ sheet, open: true })

/** What Open in browser says under it: the link's life, and that the tab is no place to sign in. */
export const BROWSER_HINT = 'A 12 h link. Never sign in there'

/** One menu for a page wherever it's listed: long-press, right-click and ⋯ share it. */
export function pageMenu(p: Page, machine: PageMachine, nav: NavigateFunction, o: { open?: boolean } = {}): MenuItem[] {
  const off = !machine.online
  return [
    ...(o.open === false ? [] : [{ key: 'open', label: 'Open', icon: <PanelsTopLeft className="size-4" />, onSelect: () => nav(pageHref(p.host, p.slug)) }]),
    // a page on a computer paired with the phone has no address outside the app
    ...(isBridged(p.host) ? [] : [{ key: 'browser', label: 'Open in browser', hint: off ? `${machine.name} is offline` : BROWSER_HINT, icon: <ExternalLink className="size-4" />, disabled: off, onSelect: () => void openInBrowser(p) }]),
    { key: 'revoke', label: 'Revoke links', icon: <Link2Off className="size-4" />, separated: true, onSelect: () => void revokeLinks(p) },
    { key: 'folder', label: 'Show folder', icon: <FolderOpen className="size-4" />, onSelect: () => openPageSheet({ kind: 'folder', page: p }) },
    { key: 'remove', label: 'Remove', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void removePage(p, machine.name) },
  ]
}

export function PageSheets() {
  const { sheet, open } = usePageSheet()
  const close = () => usePageSheet.setState({ open: false })
  if (!sheet) return null
  return sheet.kind === 'folder' ? <FolderSheet page={sheet.page} open={open} onClose={close} /> : <AddFolderSheet open={open} onClose={close} />
}

function FolderSheet({ page: p, open, onClose }: { page: Page; open: boolean; onClose: () => void }) {
  const machine = useMachines()(p.host)
  const rows: [string, React.ReactNode, boolean?][] = [
    ['Page', p.name],
    ['Folder', p.folder, true],
    ['Machine', machine.online ? machine.name : `${machine.name} · offline`],
    ['Project', projectName(p.project)],
    ['Added by', `${p.by === 'cli' ? 'inyon pages add' : p.by === 'app' ? 'the app' : p.by} · ${new Date(p.added).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`],
  ]
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Folder"
      icon={<FolderOpen className="size-4 text-fg-2" />}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="danger" onClick={() => (onClose(), void removePage(p, machine.name))}>
            <Trash2 className="size-3.5" /> Remove
          </Button>
          <Button variant="primary" onClick={() => void copyFolder(p)}>
            <Copy className="size-3.5" /> Copy path
          </Button>
        </div>
      }
    >
      <dl className="grid grid-cols-[96px_minmax(0,1fr)] text-[14px]">
        {rows.map(([k, v, mono], i) => (
          <div key={k} className={clsx('contents', i > 0 && '[&>*]:border-t [&>*]:border-line')}>
            <dt className="py-2.5 text-[13px] text-fg-2">{k}</dt>
            <dd className={clsx('min-w-0 py-2.5 break-words', mono && 'font-mono text-[13px]')}>{v}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-3 text-[13px] text-fg-3">Remove takes the page out of Inyon. The folder and its files stay.</p>
    </Sheet>
  )
}

function AddFolderSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const hosts = useMemo(() => (open ? pageHosts(useStore.getState().hosts) : []), [open])
  const machines = useMachines()
  const [host, setHost] = useState('')
  const [folder, setFolder] = useState('')
  const [name, setName] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  useOpened(open, undefined, () => {
    setHost(hosts[0] ?? '')
    setFolder('')
    setName('')
    setError('')
  })
  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    const path = folder.trim()
    if (!path.startsWith('/') && !/^[A-Za-z]:[\\/]/.test(path)) return setError('Give the full path, starting with /.')
    setBusy(true)
    setError('')
    try {
      const added = await api.addPage(host || hosts[0]!, { path, ...(name.trim() ? { name: name.trim() } : {}) })
      await loadPages()
      toast({ title: `Added “${added.name}”`, body: `On ${machines(host || hosts[0]!).name}`, tone: 'success' })
      onClose()
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Add a folder"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" form="add-page" loading={busy} disabled={!folder.trim() || !hosts.length}>Add</Button>
        </div>
      }
    >
      <form id="add-page" onSubmit={submit} className="space-y-3">
        {hosts.length > 1 && (
          <Field label="Machine">
            <Select value={host} onChange={setHost} options={hosts.map((h) => ({ value: h, label: machines(h).name }))} aria-label="Machine" />
          </Field>
        )}
        <Field label="Folder" hint="The site's own folder, the one with index.html in it.">
          <input value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="/home/me/project/private/plans/my-plan" spellCheck={false} autoCapitalize="off" className={clsx(inputClass, 'font-mono text-[13px]')} />
        </Field>
        <Field label="Name">
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Shown in the list" className={inputClass} />
        </Field>
        {error && <p role="alert" className="text-[13px] text-danger">{error}</p>}
        <p className="text-[13px] text-fg-3">
          Agents do this for you with <span className="font-mono text-[12px]">inyon pages add</span>.
        </p>
      </form>
    </Sheet>
  )
}

/**
 * Pages as rows in one card, each a link to its viewer with the page menu: on a machine's page, in
 * a session's Pages tab and under a reply that links to them. `where` adds the machine's name.
 */
export function PageRows({ pages, where, className }: { pages: Page[]; where?: boolean; className?: string }) {
  const seen = usePages((s) => s.seen)
  const nav = useNavigate()
  const machineOf = useMachines()
  return (
    <div className={clsx('divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card', className)}>
      {pages.map((p) => {
        const machine = machineOf(p.host)
        return (
          <Link key={p.id} to={pageHref(p.host, p.slug)} {...contextMenu(() => pageMenu(p, machine, nav), { touch: true, header: p.name })} className="flex min-h-[52px] items-center gap-3 px-4 py-2.5 hover:bg-hover/60 active:bg-hover">
            <PanelsTopLeft className="size-4 shrink-0 text-fg-3" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-[14px] font-medium">{p.name}</span>
                {isChanged(p, seen) && <ChangedDot />}
              </div>
              <div className="truncate font-mono text-[12px] text-fg-3">
                {where ? `${machine.name} · ` : ''}
                {p.progress?.now ? p.progress.now : projectName(p.project)} · {machine.online ? updatedLine(p) : 'offline'}
              </div>
            </div>
            {p.progress && (
              <span className="tabular flex shrink-0 items-center gap-2 font-mono text-[12px] text-fg-2">
                <ProgressBar done={p.progress.done} total={p.progress.total} className="h-[3px] w-12 max-sm:hidden" />
                {p.progress.done}/{p.progress.total}
              </span>
            )}
            <ChevronRight className="size-4 shrink-0 text-fg-3" />
          </Link>
        )
      })}
    </div>
  )
}

/** "Pages on this machine", on a machine's page; nothing when it has none. */
export function MachinePages({ host }: { host: string }) {
  const all = usePages((s) => s.pages)
  const pages = useMemo(() => (all ?? []).filter((p) => p.host === host).sort((a, b) => changedAt(b) - changedAt(a)), [all, host])
  if (!pages.length) return null
  return (
    <section className="mt-8">
      <SectionLabel>Pages on this machine</SectionLabel>
      <PageRows pages={pages} />
    </section>
  )
}
