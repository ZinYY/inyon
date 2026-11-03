import { create } from 'zustand'
import { agentDef, type HostState, type PageEntry, type PageList, type PageOpen } from '@inyon/protocol'
import { api, ApiError } from './api.ts'
import { hubHostId, isHubHost, listHubs, splitHostId, type PairedHub } from './hubs.ts'
import { isPhone } from './phone.ts'
import { allPanes, paneTitle } from './overview.ts'

/**
 * Pages: sites an agent made (plans, mockups) that the hub serves under a token link, shown in a
 * sandboxed frame. The list comes from GET /api/pages on the hub this app runs on, or, in the
 * phone app, from every paired hub with the phone's token and from the phone's own core, which
 * asks the computers paired with the phone. A page on one of those has no address a browser can
 * open: the app's viewer renders it from its files (lib/pageBridge.ts, src/pageview).
 */

/** A page as the app lists it: its entry, with the hub's machine id as this app names it. */
export interface Page extends PageEntry {
  /** `<host>/<slug>` */
  id: string
}

export const pageId = (host: string, slug: string) => `${host}/${slug}`
export const pageHref = (host: string, slug: string) => `/pages/${encodeURIComponent(host)}/${slug}`

/** The last folder of a project path, as the list names it. */
export const projectName = (project: string) => project.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || project

/** When a page last changed, as far as the app can tell: its progress, or when it was added. */
export function changedAt(p: PageEntry): number {
  const t = p.progress?.updated ? Date.parse(p.progress.updated) : NaN
  return Number.isFinite(t) ? Math.max(t, p.added) : p.added
}

/** A hub's own machine: "local" here, or "hub:<id>/local" for a paired hub's. */
export const isHubMachine = (host: string) => (isHubHost(host) ? host.slice(host.indexOf('/') + 1) : host) === 'local'

/**
 * What a page's group is called: the name its session has in the app now (`running`, by
 * `<host>/<session>`), else the name Inyon last gave that session, else its project's folder. So a
 * session renamed after it added the page renames the group too.
 */
export function groupName(p: Page, running?: ReadonlyMap<string, string>) {
  return (p.session && running?.get(pageId(p.host, p.session))) || p.sessionName || projectName(p.project)
}

/**
 * The names running agent sessions have in the app, by `<host>/<conversation id>`, for groupName.
 * An agent that shows no title of its own ("Claude Code") gives no name.
 */
export function runningNames(hosts: HostState[]): Map<string, string> {
  const out = new Map<string, string>()
  for (const r of allPanes(hosts)) {
    const a = r.pane.agent
    if (!a?.sessionId) continue
    const name = paneTitle(r)
    if (name && name !== (a.custom ?? agentDef(a.kind)?.name) && name !== r.pane.command) out.set(pageId(r.host.config.id, a.sessionId), name)
  }
  return out
}

/** A machine's projects and their pages, the newest change first. */
export interface PageGroup {
  host: string
  projects: { name: string; key: string; pages: Page[] }[]
}

/**
 * Machine, then session or project, then page. The hub's own machine comes first, then the others
 * by name; groups by name (see groupName); inside a group the page that changed last is on top.
 */
export function groupPages(pages: Page[], hostName: (host: string) => string, running?: ReadonlyMap<string, string>): PageGroup[] {
  const byHost = new Map<string, Map<string, { name: string; list: Page[] }>>()
  for (const p of pages) {
    const groups = byHost.get(p.host) ?? byHost.set(p.host, new Map()).get(p.host)!
    const name = groupName(p, running)
    const key = p.session && name !== projectName(p.project) ? `s:${p.session}` : `p:${name}`
    ;(groups.get(key) ?? groups.set(key, { name, list: [] }).get(key)!).list.push(p)
  }
  const hosts = [...byHost.keys()].sort((a, b) => Number(isHubMachine(b)) - Number(isHubMachine(a)) || hostName(a).localeCompare(hostName(b)))
  return hosts.map((host) => ({
    host,
    projects: [...byHost.get(host)!.entries()]
      .sort(([, a], [, b]) => a.name.localeCompare(b.name))
      .map(([key, { name, list }]) => ({ name, key: `${host}/${key}`, pages: [...list].sort((a, b) => changedAt(b) - changedAt(a)) })),
  }))
}

/** Whether a page matches what's typed in the filter: its name, group, project or machine. */
export function matchesPage(p: Page, machine: string, q: string, running?: ReadonlyMap<string, string>) {
  const t = q.trim().toLowerCase()
  return !t || [p.name, groupName(p, running), projectName(p.project), machine].some((s) => s.toLowerCase().includes(t))
}

// ---------- a session's pages ----------

/** A folder as the app compares it: forward slashes, no slash at the end, and a Windows path in any case. */
export function folderKey(p: string) {
  const s = p.replace(/\\/g, '/').replace(/(.)\/+$/, '$1')
  return /^[a-z]:/i.test(s) ? s.toLowerCase() : s
}

const within = (child: string, parent: string) => child === parent || child.startsWith(parent.endsWith('/') ? parent : `${parent}/`)

/**
 * The pages a session running in `cwd` on `host` works on, the newest change first: on the same
 * machine, the ones its conversation (`session`) added, and those with the session inside the
 * page's project or the page's folder under the session's. A session in a home folder or higher
 * (two folders deep or less) gets no pages by folder below it, or every page on the machine would
 * be its own.
 */
export function sessionPages(pages: Page[], host: string, cwd: string | undefined, session?: string): Page[] {
  const at = cwd ? folderKey(cwd) : undefined
  const deep = !!at && at.replace(/^[a-z]:/, '').split('/').filter(Boolean).length > 2
  return pages
    .filter((p) => p.host === host && ((!!session && p.session === session) || (!!at && (within(at, folderKey(p.project)) || (deep && within(folderKey(p.folder), at))))))
    .sort((a, b) => changedAt(b) - changedAt(a))
}

/**
 * The pages a reply links to, as `/pages/<host>/<slug>` (what `inyon pages add` prints), each once
 * and in order, with the host as this app names it: a session that came through a paired hub
 * names that hub's machines.
 */
export function linkedPages(text: string, sessionHost: string): string[] {
  const hub = isHubHost(sessionHost) ? sessionHost.slice(0, sessionHost.indexOf('/') + 1) : ''
  const out: string[] = []
  for (const m of text.matchAll(/(?:^|[^\w/.-])(?:https?:\/\/[^\s/]+)?\/pages\/([\w.%-]+)\/([a-z0-9][a-z0-9-]{0,62})(?![\w-])/g)) {
    let host: string
    try {
      host = decodeURIComponent(m[1]!)
    } catch {
      continue
    }
    const id = pageId(hub + host, m[2]!)
    if (!out.includes(id)) out.push(id)
  }
  return out
}

// ---------- changed since you last opened it ----------

const SEEN_KEY = 'inyon.pages.seen.v1'

function loadSeen(): Record<string, number> {
  try {
    const v = JSON.parse(localStorage.getItem(SEEN_KEY) ?? '{}')
    return v && typeof v === 'object' && !Array.isArray(v) ? v : {}
  } catch {
    return {}
  }
}

/** A page counts as changed until it is opened, and again whenever it changes after that. */
export const isChanged = (p: Page, seen: Record<string, number>) => !(p.id in seen) || changedAt(p) > seen[p.id]!

// ---------- links ----------

/** A link is minted again when less than this is left of it: by a timer while the page is on screen, and when the app comes back into view. */
export const RENEW_BEFORE_MS = 60 * 60 * 1000
export const shouldRenew = (until: number, now: number) => until - now < RENEW_BEFORE_MS

/**
 * A link with when it runs out by this device's clock. The hub says how long it lasts (`ttl`), counted
 * here from when it was asked for, since the phone's clock and the hub's may not agree; a hub that
 * doesn't say gives only its own `expires`.
 */
export interface PageLink extends PageOpen {
  until: number
}
export const localDeadline = (l: PageOpen, askedAt: number): PageLink => ({ ...l, until: typeof l.ttl === 'number' ? askedAt + l.ttl : l.expires })

/** How long to wait before renewing a link that is on screen: until an hour is left, at least 5 s. */
export const renewIn = (until: number, now: number) => Math.max(5_000, until - RENEW_BEFORE_MS - now)

/** A timed renewal that failed is tried again after a minute, then twice as long each time, up to 10 minutes. */
export const RENEW_RETRY_MS = 60_000
export const RENEW_RETRY_MAX_MS = 10 * 60_000
export const nextRetry = (wait: number) => Math.min(wait * 2, RENEW_RETRY_MAX_MS)

/** Where a page's hub is: '' for the hub this app came from, else the paired hub's address. */
export const pageBase = (host: string) => splitHostId(host).hub?.url ?? ''

/**
 * Whether a page is on a computer paired with the phone rather than behind a hub: the phone core
 * answers its link, which only the app's viewer can open (no browser outside the app can).
 */
export const isBridged = (host: string) => isPhone() && !isHubHost(host)

/** A page link as the browser outside the app needs it, with the hub's address in front. */
export const absoluteLink = (host: string, url: string) => `${pageBase(host) || location.origin}${url}`

/** The parts of a Blueprint site the viewer can jump to. */
export type Part = 'plan' | 'mockups' | 'matrix'
export const PART_PATH: Record<Part, string> = { plan: '', mockups: 'mockups/', matrix: 'matrix.html' }
export const PART_LABEL: Record<Part, string> = { plan: 'Plan', mockups: 'Mockups', matrix: 'Matrix' }

/**
 * The frame's address: the token path (it ends in a slash, so the query survives without a
 * redirect), the part, and the app's theme for the page to start in.
 */
export const frameSrc = (base: string, url: string, part: Part, theme: 'light' | 'dark') => `${base}${url}${PART_PATH[part]}?theme=${theme}`

/**
 * A note in the site's `research/` folder, which the hub shows as a page to read (`?as=note`)
 * inside the same sandbox as the site.
 */
export const noteSrc = (base: string, url: string, note: string, theme: 'light' | 'dark') => `${base}${url}research/${encodeURIComponent(note)}?as=note&theme=${theme}`

/** A note's name as the app shows it: the file name without `.md`. */
export const noteTitle = (name: string) => name.replace(/\.md$/i, '')

/** Messages a page may send the app. Anything else, or from anywhere but the frame, is ignored. */
export type FrameMessage = { type: 'theme?' }
const FRAME_TYPES = new Set<string>(['theme?'])

/** The message, when it came from the page's own frame and is a kind the app knows; null otherwise. */
export function frameMessage(e: { source: unknown; data: unknown }, frame: Window | null | undefined): FrameMessage | null {
  if (!frame || e.source !== frame) return null
  const d = e.data as { type?: unknown } | null
  if (!d || typeof d !== 'object' || typeof d.type !== 'string' || !FRAME_TYPES.has(d.type)) return null
  return { type: d.type as FrameMessage['type'] }
}

/**
 * The Preview tab frames what it shows with same-origin rights, so it must never show Inyon itself:
 * that page would run as you. Returns why it won't, or null.
 */
export function previewRefusal(url: string, appOrigins: string[]): string | null {
  let origin: string
  try {
    origin = new URL(url).origin
  } catch {
    return null
  }
  return appOrigins.includes(origin) ? 'That address is Inyon itself, and a preview of it could act as you. Pages an agent made open under Pages.' : null
}

/** This app's own address and every paired hub's, which the Preview tab refuses. */
export const appOrigins = () => [location.origin, ...listHubs().flatMap((h) => { try { return [new URL(h.url).origin] } catch { return [] } })]

// ---------- words ----------

/** "just now", "3 min ago", "2 h ago", "4 d ago", or a date. */
export function agoText(t: number, now = Date.now()) {
  const s = Math.max(0, (now - t) / 1000)
  if (s < 45) return 'just now'
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`
  if (s < 86400) return `${Math.round(s / 3600)} h ago`
  if (s < 14 * 86400) return `${Math.round(s / 86400)} d ago`
  return new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

/** "11 h 52 m" until a link runs out. */
export function leftText(expires: number, now = Date.now()) {
  const m = Math.max(0, Math.floor((expires - now) / 60000))
  return m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} m` : `${m} m`
}

/** The device sizes the desktop viewer offers; Fit fills the pane. */
export type Device = 'fit' | 'phone' | 'tablet' | 'desktop'
export const DEVICES: Record<Exclude<Device, 'fit'>, { name: string; w: number; h: number }> = {
  phone: { name: 'Phone', w: 390, h: 844 },
  tablet: { name: 'Tablet', w: 820, h: 1180 },
  desktop: { name: 'Desktop', w: 1440, h: 900 },
}
/** How much a device frame shrinks to fit the stage, never grown past its real size. */
export const fitScale = (w: number, h: number, stageW: number, stageH: number, pad = 24) => Math.max(0.1, Math.min(1, (stageW - pad) / w, (stageH - pad) / h))

// ---------- the list ----------

interface PagesState {
  /** null until the first answer */
  pages: Page[] | null
  /** in the phone app, the computers paired with it whose inyon is too old to serve their pages (by host id) */
  outdated: string[]
  error?: string
  seen: Record<string, number>
  /** in the phone app, the paired hubs that didn't answer the last time: their pages stay listed, shown offline */
  unreachable: string[]
  /** in the phone app, the paired hubs that refused the phone's token (401 or 403): it was forgotten there, so their pages are gone until it pairs again */
  forgotten: string[]
}

export const usePages = create<PagesState>(() => ({ pages: null, outdated: [], seen: loadSeen(), unreachable: [], forgotten: [] }))

/** A hub that answered "who are you?": the phone was forgotten there, or this app was signed out. */
export const isRefusal = (e: unknown) => e instanceof ApiError && (e.status === 401 || e.status === 403)

/** The paired hub a page's host belongs to, by id; undefined for this app's own hub. */
export const hubIdOf = (host: string) => (isHubHost(host) ? host.slice('hub:'.length, host.indexOf('/')) : undefined)

/** Whether a host's hub refused the phone the last time the list was asked for. */
export const hubForgot = (host: string, forgotten: string[]) => {
  const id = hubIdOf(host)
  return id !== undefined && forgotten.includes(id)
}

/** Whether a host's hub didn't answer the last time the list was asked for. */
export const hubUnreachable = (host: string, unreachable: string[]) => {
  const id = hubIdOf(host)
  return id !== undefined && unreachable.includes(id)
}

const asPages = (entries: PageEntry[] | undefined, host: (h: string) => string = (h) => h): Page[] =>
  (Array.isArray(entries) ? entries : []).map((e) => ({ ...e, host: host(e.host), id: pageId(host(e.host), e.slug) }))

/** What the phone core lists: the paired computers' pages, and those too old to say. */
type ComputerPages = PageList & { outdated?: unknown }

async function fromComputers(): Promise<{ pages: Page[]; outdated: string[] }> {
  const r = (await api.pages()) as ComputerPages
  return { pages: asPages(r?.pages), outdated: Array.isArray(r?.outdated) ? r.outdated.filter((h): h is string => typeof h === 'string') : [] }
}

async function fromHub(hub: PairedHub): Promise<Page[]> {
  try {
    const r = await api.hubPages(hub)
    return asPages(r?.pages, (h) => hubHostId(hub.id, h))
  } catch (e) {
    // a hub from before Pages has no such route: it simply has none
    if (e instanceof ApiError && e.status === 404) return []
    throw e
  }
}

const ok = <T>(value: T): PromiseSettledResult<T> => ({ status: 'fulfilled', value })
const no = (reason: unknown): PromiseRejectedResult => ({ status: 'rejected', reason })

let loading: Promise<void> | undefined
/** Asks for the list again (at most one request at a time). */
export function loadPages(): Promise<void> {
  if (loading) return loading
  // cleared once it settles: a finally inside could run before the promise is even stored
  const p = fetchPages().finally(() => {
    if (loading === p) loading = undefined
  })
  return (loading = p)
}

async function fetchPages(): Promise<void> {
  try {
    if (!isPhone()) {
      const r = await api.pages()
      usePages.setState({ pages: asPages(r?.pages), outdated: [], error: undefined, unreachable: [], forgotten: [] })
      return
    }
    const hubs = listHubs()
    const [own, ...got] = await Promise.all([fromComputers().then(ok, no), ...hubs.map((h) => fromHub(h).then(ok, no))])
    // a hub out of reach (a blip, a restart, the phone off the VPN) keeps the pages it had, shown
    // offline; only an answer from a hub changes its list, and one that refuses the phone empties it
    const before = usePages.getState().pages ?? []
    const pages: Page[] = []
    const unreachable: string[] = []
    const forgotten: string[] = []
    let outdated = usePages.getState().outdated
    if (own!.status === 'fulfilled') {
      pages.push(...own!.value.pages)
      outdated = own!.value.outdated
    } else pages.push(...before.filter((p) => hubIdOf(p.host) === undefined))
    got.forEach((g, i) => {
      const hub = hubs[i]!
      if (g.status === 'fulfilled') pages.push(...g.value)
      else if (isRefusal(g.reason)) forgotten.push(hub.id)
      else {
        unreachable.push(hub.id)
        pages.push(...before.filter((p) => hubIdOf(p.host) === hub.id))
      }
    })
    const failed = [own!, ...got].find((g): g is PromiseRejectedResult => g.status === 'rejected' && !isRefusal(g.reason))
    // all of them out of reach is an error (the core answering with no computer's pages doesn't
    // count); a forgotten hub says so on its own line
    const answered = got.some((g) => g.status === 'fulfilled') || (own!.status === 'fulfilled' && own!.value.pages.length > 0)
    usePages.setState({ pages, outdated, unreachable, forgotten, error: failed && !answered ? (failed.reason as Error).message : undefined })
  } catch (e) {
    // signed out: the list goes with the sign-in (the app asks for it again); anything else keeps it
    if (isRefusal(e)) usePages.setState({ pages: [], error: (e as Error).message, unreachable: [], forgotten: [] })
    else usePages.setState((s) => ({ pages: s.pages ?? [], error: (e as Error).message }))
  }
}

/** Remembers that a page was looked at now, so its changed mark goes. */
export function markSeen(p: Page, now = Date.now()) {
  const seen = { ...usePages.getState().seen, [p.id]: Math.max(now, changedAt(p)) }
  usePages.setState({ seen })
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify(seen))
  } catch {}
}

/** Takes a page off the list here at once (the hub is asked separately). */
export const dropPage = (id: string) => usePages.setState((s) => ({ pages: s.pages?.filter((p) => p.id !== id) ?? null }))

/** Mints a link for a page on its hub. */
export const openLink = (p: Pick<Page, 'host' | 'slug'>): Promise<PageOpen> => api.openPage(p.host, p.slug)

/**
 * The machines a folder can be added on: the hub's own machine; in the phone app, each paired
 * hub's own machine and each computer paired with the phone that is online (`hosts`, the app's).
 */
export function pageHosts(hosts: HostState[] = []): string[] {
  if (!isPhone()) return ['local']
  const computers = hosts.filter((h) => h.config.kind !== 'local' && !isHubHost(h.config.id) && h.status === 'online').map((h) => h.config.id)
  return [...listHubs().map((h) => hubHostId(h.id, 'local')), ...computers]
}
