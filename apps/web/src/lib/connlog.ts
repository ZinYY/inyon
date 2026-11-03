import { useSyncExternalStore } from 'react'
import { redact } from './redact.ts'

/**
 * When each hub and computer connected, dropped and why, so "Can't connect?" and Settings can say
 * what happened instead of a bare "Reconnecting". Kept in memory, with a bounded copy in this
 * device's storage that survives a restart of the app. Details are redacted before they're kept.
 */

/** What the connection itself said when it ended. */
export type Cause =
  /** the socket closed (its close code says more) */
  | 'closed'
  /** nothing came back for too long */
  | 'timeout'
  /** the hub signed this device out, or forgot this phone */
  | 'signed-out'
  /** the computer answered but the encrypted handshake failed */
  | 'handshake'
  /** nothing answered at the address */
  | 'unreachable'
  /** the hub turned the address or the request away (421, plain http refused) */
  | 'refused'
  /** the hub that carries this machine is out of reach */
  | 'hub-down'
  /** the machine itself is offline, as its hub tells it */
  | 'offline'

/** What was going on around it, which often explains it better than the socket does. */
export type Context = 'offline' | 'background' | 'frozen' | 'doze' | 'restricted' | 'saver' | 'no-keepalive'

export interface Drop {
  at: number
  /** when it was back; unset while it's still down */
  back?: number
  /** the app was closed while it was down, so when it came back isn't known */
  cut?: boolean
  cause: Cause
  /** the WebSocket close code */
  code?: number
  detail?: string
  while: Context[]
  /** failed attempts since, and what the last one said */
  tries?: number
  last?: string
}

export type SubjectKind = 'hub' | 'computer' | 'machine'

export interface Subject {
  id: string
  kind: SubjectKind
  name: string
  /** unknown until this run of the app has seen it either way */
  up?: boolean
  /** when it last connected */
  since?: number
  drops: Drop[]
}

const KEY = 'inyon.connlog.v1'
const MAX_SUBJECTS = 40
const MAX_DROPS = 12
const STORED_DROPS = 8
const DETAIL_CHARS = 300

const clip = (s: string | undefined) => (s ? redact(s).slice(0, DETAIL_CHARS) : undefined)

/** The page's own state: offline, hidden, or just back from being frozen. */
export interface Env {
  online(): boolean
  hidden(): boolean
  /** when the page was last frozen, and when it came back (the Page Lifecycle's freeze and resume) */
  frozen(): { at?: number; back?: number }
}

const browserEnv: Env = {
  online: () => typeof navigator === 'undefined' || navigator.onLine !== false,
  hidden: () => typeof document !== 'undefined' && document.visibilityState === 'hidden',
  frozen: () => lifecycle,
}

const lifecycle: { at?: number; back?: number; offlineAt?: number } = {}
/** A drop noticed this soon after the page thawed, or the network went, happened while it was so. */
const RECENT_MS = 10_000

/** What was going on when a drop was noticed at `now`. */
export function contextAt(env: Env, now: number): Context[] {
  const out: Context[] = []
  if (!env.online() || (lifecycle.offlineAt !== undefined && now - lifecycle.offlineAt < RECENT_MS)) out.push('offline')
  const f = env.frozen()
  if (f.at !== undefined && (f.back === undefined || f.back < f.at || now - f.back < RECENT_MS)) out.push('frozen')
  if (env.hidden()) out.push('background')
  return out
}

/** Android's state as InyonDevice.powerState reports it. */
export interface Power {
  idle?: boolean
  powerSave?: boolean
  ignoringOptimizations?: boolean
  backgroundRestricted?: boolean
  keepAlive?: boolean
  onScreen?: boolean
  /** when Doze last began and ended (ms), 0 when not seen since the app started */
  idleSince?: number
  idleEnded?: number
  standbyBucket?: string
}

/**
 * Whether the phone was in Doze when a drop at `at` happened. The app is often frozen during
 * Doze and notices the drop only when it wakes, so a Doze that ended within a minute before it
 * counts too.
 */
export function dozedAt(p: Power, at: number): boolean {
  const since = p.idleSince ?? 0
  const ended = p.idleEnded ?? 0
  // a Doze that began after the drop didn't cause it, even if it's still on
  if (since > at) return false
  if (p.idle) return true
  if (!since) return false
  return ended < since || ended >= at - 60_000
}

/** The Android contexts of a drop at `at`. */
export function powerContexts(p: Power, at: number): Context[] {
  const out: Context[] = []
  if (dozedAt(p, at)) out.push('doze')
  if (p.backgroundRestricted || p.ignoringOptimizations === false) out.push('restricted')
  if (p.powerSave) out.push('saver')
  if (p.keepAlive === false) out.push('no-keepalive')
  return out
}

export class ConnLog {
  private subjects = new Map<string, Subject>()
  private listeners = new Set<() => void>()
  private version = 0

  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = typeof localStorage === 'undefined' ? undefined : localStorage,
    private env: Env = browserEnv,
  ) {
    this.load()
  }

  private load() {
    try {
      const v = JSON.parse(this.storage?.getItem(KEY) ?? '[]')
      if (!Array.isArray(v)) return
      for (const s of v) {
        if (!s || typeof s.id !== 'string' || !Array.isArray(s.drops)) continue
        // the app closed with this one down: when it came back isn't known
        const drops: Drop[] = s.drops.filter((d: Drop) => d && typeof d.at === 'number').map((d: Drop) => (d.back === undefined ? { ...d, cut: true } : d))
        this.subjects.set(s.id, { id: s.id, kind: s.kind, name: String(s.name ?? s.id), since: s.since, drops })
      }
    } catch {}
  }

  private save() {
    this.version++
    const list = [...this.subjects.values()]
      .sort((a, b) => lastAt(b) - lastAt(a))
      .slice(0, MAX_SUBJECTS)
      .map(({ up: _, ...s }) => ({ ...s, drops: s.drops.slice(-STORED_DROPS) }))
    try {
      this.storage?.setItem(KEY, JSON.stringify(list))
    } catch {}
    for (const fn of this.listeners) fn()
  }

  private subject(id: string, about: { kind: SubjectKind; name: string }): Subject {
    let s = this.subjects.get(id)
    if (!s) {
      s = { id, kind: about.kind, name: about.name, drops: [] }
      this.subjects.set(id, s)
      // the oldest go first once there are too many
      if (this.subjects.size > MAX_SUBJECTS) {
        const oldest = [...this.subjects.values()].filter((x) => x !== s).sort((a, b) => lastAt(a) - lastAt(b))[0]
        if (oldest) this.subjects.delete(oldest.id)
      }
    }
    s.kind = about.kind
    s.name = about.name
    return s
  }

  /** It's connected (again). */
  up(id: string, about: { kind: SubjectKind; name: string }, at = Date.now()) {
    const s = this.subject(id, about)
    if (s.up) return
    const open = s.drops.at(-1)
    if (open && open.back === undefined && !open.cut) open.back = at
    s.up = true
    s.since = at
    this.save()
  }

  /**
   * It dropped, or couldn't connect. While it's already down this only counts another failed
   * attempt and keeps what it said; a sign-out is kept as the cause, since it explains the rest.
   */
  down(id: string, about: { kind: SubjectKind; name: string }, cause: Cause, o: { code?: number; detail?: string; at?: number } = {}): Drop {
    const at = o.at ?? Date.now()
    const s = this.subject(id, about)
    const open = s.drops.at(-1)
    if (s.up === false && open && open.back === undefined && !open.cut) {
      open.tries = (open.tries ?? 0) + 1
      open.last = clip(o.detail) ?? open.last
      if (cause === 'signed-out' || cause === 'refused') open.cause = cause
      this.save()
      return open
    }
    const drop: Drop = { at, cause, while: contextAt(this.env, at), ...(o.code !== undefined && { code: o.code }), ...(o.detail && { detail: clip(o.detail) }) }
    s.up = false
    s.drops.push(drop)
    if (s.drops.length > MAX_DROPS) s.drops.splice(0, s.drops.length - MAX_DROPS)
    this.save()
    return drop
  }

  /** Adds what was going on (Android's Doze, say) to a drop once it's known. */
  annotate(drop: Drop, more: Context[]) {
    const add = more.filter((c) => !drop.while.includes(c))
    if (!add.length) return
    drop.while = [...drop.while, ...add]
    this.save()
  }

  /** Changes the cause of the drop that's open (the hub said afterwards it signed this device out). */
  explain(id: string, cause: Cause, detail?: string) {
    const open = this.subjects.get(id)?.drops.at(-1)
    if (!open || open.back !== undefined) return
    open.cause = cause
    if (detail) open.detail = clip(detail)
    this.save()
  }

  forget(id: string) {
    if (this.subjects.delete(id)) this.save()
  }

  get(id: string): Subject | undefined {
    return this.subjects.get(id)
  }

  lastDrop(id: string): Drop | undefined {
    return this.subjects.get(id)?.drops.at(-1)
  }

  all(): Subject[] {
    return [...this.subjects.values()]
  }

  /** The latest drops of every subject, newest first. */
  recent(n = 10): { subject: Subject; drop: Drop }[] {
    return this.all()
      .flatMap((subject) => subject.drops.map((drop) => ({ subject, drop })))
      .sort((a, b) => b.drop.at - a.drop.at)
      .slice(0, n)
  }

  on(fn: () => void) {
    this.listeners.add(fn)
    return () => void this.listeners.delete(fn)
  }

  /** Changes with every write, for useSyncExternalStore. */
  get stamp() {
    return this.version
  }
}

const lastAt = (s: Subject) => Math.max(s.since ?? 0, s.drops.at(-1)?.at ?? 0)

export const connLog = new ConnLog()

/** Android's state, where the app can read it (connwatch.ts sets it). */
let powerSource: (() => Promise<Power | undefined>) | undefined
export const setPowerSource = (fn: typeof powerSource) => void (powerSource = fn)

/** Records a drop on the shared log, then adds what Android was doing at the time. */
export function dropped(id: string, about: { kind: SubjectKind; name: string }, cause: Cause, o: { code?: number; detail?: string } = {}): Drop {
  const fresh = connLog.get(id)?.up !== false
  const d = connLog.down(id, about, cause, o)
  if (fresh && powerSource) void powerSource().then((p) => p && connLog.annotate(d, powerContexts(p, d.at)), () => {})
  return d
}

/** Re-renders with every change to the log. */
export const useConnLog = () =>
  useSyncExternalStore(
    (fn) => connLog.on(fn),
    () => connLog.stamp,
  )

/** The last drop of a hub or computer, kept current. */
export function useLastDrop(id: string | undefined): Drop | undefined {
  useConnLog()
  return id ? connLog.lastDrop(id) : undefined
}

/** Whether a hub or computer is down as far as this run of the app has seen. */
export const isDown = (id: string) => connLog.get(id)?.up === false

let watching = false
/** Follows the page going offline and being frozen, for the contexts above. Once. */
export function watchPage() {
  if (watching || typeof window === 'undefined') return
  watching = true
  window.addEventListener('offline', () => void (lifecycle.offlineAt = Date.now()))
  document.addEventListener('freeze', () => void (lifecycle.at = Date.now()))
  document.addEventListener('resume', () => void (lifecycle.back = Date.now()))
}

// ---------- in words ----------

const CLOSE_CODES: Record<number, string> = {
  1000: 'the connection was closed',
  1001: 'the other side went away (a restart, most likely)',
  1006: 'the connection was cut without a goodbye',
  1011: 'the hub hit an error',
  1012: 'the hub restarted',
  1013: 'the hub was too busy',
  4001: 'the hub signed this device out',
  4408: 'it stopped answering pings',
}

/** Why it dropped, in one plain phrase ("Android's Doze paused the phone"). */
export function why(d: Drop): string {
  const bg = d.while.includes('background') ? ' while Inyon was in the background' : ''
  switch (d.cause) {
    case 'signed-out':
      return d.detail ?? 'the hub signed this device out'
    case 'refused':
      return d.detail ?? 'the hub turned the address away'
    case 'hub-down':
      return 'its hub was out of reach'
    case 'offline':
      return d.detail ?? 'the hub lost it'
  }
  if (d.while.includes('doze')) return "Android's Doze paused the phone"
  if (d.while.includes('frozen')) return 'the page was frozen in the background'
  if (d.while.includes('offline')) return 'the network went away'
  if (d.while.includes('restricted') && bg) return `Android's battery limits stopped Inyon${bg}`
  switch (d.cause) {
    case 'handshake':
      return `the computer answered but the handshake failed${d.detail ? `: ${d.detail}` : ''}`
    case 'timeout':
      return `nothing came back for a minute${bg}`
    case 'unreachable':
      return `nothing answered${d.detail ? ` (${d.detail})` : ''}`
  }
  const said = d.code !== undefined ? CLOSE_CODES[d.code] ?? `it closed with code ${d.code}` : 'the connection closed'
  return `${said}${bg}`
}

/** How long it was down: "4m 10s", "still down", or that the app closed first. */
export function downFor(d: Drop, now = Date.now()): string {
  if (d.back !== undefined) return span(d.back - d.at)
  if (d.cut) return 'until the app closed'
  return `still down after ${span(now - d.at)}`
}

export function span(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return s % 60 ? `${m}m ${s % 60}s` : `${m}m`
  const h = Math.floor(m / 60)
  return m % 60 ? `${h}h ${m % 60}m` : `${h}h`
}

/** What the contexts were, for the report: "Doze, in the background". */
export function contextWords(c: Context[]): string {
  const words: Record<Context, string> = {
    offline: 'offline',
    background: 'in the background',
    frozen: 'page frozen',
    doze: 'Doze',
    restricted: 'battery limits',
    saver: 'battery saver',
    'no-keepalive': 'Stay connected off',
  }
  return c.map((x) => words[x]).join(', ')
}

/** What a machine's own error says about why it isn't connected. */
export function causeOf(error: string | undefined): Cause {
  const e = error ?? ''
  if (/no longer (knows|paired)|not paired|isn't paired|unpaired|forgot/i.test(e)) return 'signed-out'
  if (/handshake|bad key|something else answered/i.test(e)) return 'handshake'
  if (/^(Reconnecting|Connecting) to .+/.test(e) && !/\(/.test(e)) return 'hub-down'
  if (/couldn't find|isn't answering|didn't answer|timed? ?out/i.test(e)) return 'unreachable'
  if (/plain http|doesn't answer to that address/i.test(e)) return 'refused'
  return e ? 'offline' : 'closed'
}
