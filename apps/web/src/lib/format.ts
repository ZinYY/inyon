/** The separator a path uses: a Windows path (C:\Users\me) uses backslashes. */
const sepOf = (p: string) => (p.includes('\\') && !p.includes('/') ? '\\' : '/')

/** Shortens an absolute path: home → ~, long paths keep the last segments. Windows paths too. */
export function shortPath(p: string | undefined, max = 42) {
  if (!p) return ''
  const sep = sepOf(p)
  let s = sep === '/' ? p.replace(/^\/(home\/[^/]+|Users\/[^/]+|root)(?=\/|$)/, '~') : p.replace(/^[A-Za-z]:\\Users\\[^\\]+(?=\\|$)/i, '~')
  if (s.length <= max) return s
  const parts = s.split(sep)
  const n = parts.length
  while (parts.length > 2 && parts.join(sep).length > max - 2) parts.splice(1, 1)
  // nothing to drop (one long name): show it whole rather than a "…" that hides nothing
  return parts.length < n ? `${parts[0]}${sep}…${sep}${parts.slice(1).join(sep)}` : s
}

/** A long folder as its last two parts ("…/qa/oc1", "…\demo\api"), for a small chip. */
export function tailPath(p: string, max = 32) {
  if (p.length <= max) return p
  const sep = sepOf(p)
  const parts = p.split(sep).filter(Boolean)
  return parts.length > 2 ? `…${sep}${parts.slice(-2).join(sep)}` : p
}

/**
 * How long a machine took to answer its last status check: "38 ms", "1.4 s". It's the time the
 * check takes to run there and come back, not the network's round trip.
 */
export const RESPONSE_TITLE = 'How long the machine took to report its sessions at the last check'
export const responseTime = (ms: number) => (ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`)

export const basename = (p: string | undefined) => (p ?? '').replace(/\/+$/, '').split('/').pop() || p || ''

export function duration(ms: number) {
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  return `${Math.floor(m / 60)}h ${m % 60}m`
}

export function timeAgo(ts: string | number) {
  const t = typeof ts === 'number' ? ts : Date.parse(ts)
  if (!t) return ''
  const s = Math.max(0, (Date.now() - t) / 1000)
  if (s < 45) return 'just now'
  if (s < 3600) return `${Math.round(s / 60)}m ago`
  if (s < 86400) return `${Math.round(s / 3600)}h ago`
  return new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

export function clock(ts: string) {
  const t = Date.parse(ts)
  return t ? new Date(t).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : ''
}

export function modelName(id?: string) {
  if (!id) return ''
  // "claude-opus-4-6[1m]" is the 1M-context variant of the same model
  const m = id.match(/claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:[-[]|$)/)
  if (!m) return id
  return `${m[1][0].toUpperCase()}${m[1].slice(1)} ${m[2]}${m[3] && m[3].length < 3 ? `.${m[3]}` : ''}`
}

/** "%12" → "12" for display */
export const paneLabel = (id: string) => id.replace(/^%/, '')

export function tokens(n?: number) {
  if (n === undefined || n === null || Number.isNaN(n)) return '—'
  // 999.5k and up would otherwise read "1000k"
  if (n >= 999_500) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1).replace(/\.0$/, '')}M`
  if (n >= 1000) return `${Math.round(n / 1000)}k`
  return String(n)
}

export function bytes(n: number) {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(n >= 100 * 1024 ** 2 ? 0 : 1)} MB`
  if (n >= 1024) return `${Math.round(n / 1024)} KB`
  return `${n} B`
}

export function money(n?: number) {
  if (n === undefined) return '—'
  return n >= 100 ? `$${n.toFixed(0)}` : `$${n.toFixed(2)}`
}

/** "in 2h 10m" until a unix-seconds time, plus the clock time. */
/**
 * A plan-limit window as of now. Once its reset time passes, the old percentage is history: show
 * it as fresh until the agent reports the new window.
 */
export function liveWindow<W extends { pct: number; resetsAt?: number }>(w?: W): (W & { reset?: boolean }) | undefined {
  if (!w) return undefined
  if (w.resetsAt && w.resetsAt * 1000 <= Date.now()) return { ...w, pct: 0, resetsAt: undefined, reset: true }
  return w
}

export function resetsIn(unix?: number) {
  if (!unix) return ''
  const s = unix - Date.now() / 1000
  if (s <= 0) return 'resetting'
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const d = Math.floor(h / 24)
  const span = d >= 1 ? `${d}d ${h % 24}h` : h ? `${h}h ${m}m` : `${m}m`
  return `resets in ${span}`
}

export function resetClock(unix?: number) {
  if (!unix) return ''
  const d = new Date(unix * 1000)
  const sameDay = d.toDateString() === new Date().toDateString()
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
    : d.toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })
}

export function since(iso?: string | number) {
  const t = typeof iso === 'number' ? iso : iso ? Date.parse(iso) : NaN
  if (!t) return ''
  return duration(Math.max(0, Date.now() - t))
}

/** Opens a web link in a new tab; anything that isn't http(s) is ignored. */
export function openLink(url: string | undefined) {
  if (url && /^https?:\/\//i.test(url)) window.open(url, '_blank', 'noopener')
}
