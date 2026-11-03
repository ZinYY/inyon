/**
 * What may differ between two runs of the same hub, and nothing else. Each rule says what it
 * replaces and why; a value no rule matches is kept exactly, so a real difference shows in the diff.
 *
 * Placeholders are numbered by first appearance in a scenario's transcript (`<uuid:1>`, `<uuid:2>`),
 * so "the same id again" and "a different id" still read differently after normalization.
 */

export interface Rule {
  name: string
  why: string
  /** returns the replacement, or undefined to leave the value to the next rule */
  string?(s: string, n: Normalizer, key: string): string | undefined
  number?(v: number, n: Normalizer, key: string): string | number | undefined
  /** for an object as a whole, before its fields: returns a replacement, or undefined */
  object?(o: Record<string, unknown>, n: Normalizer, key: string): Record<string, unknown> | undefined
}

const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi
const SHA256 = /\b[0-9a-f]{64}\b/g
const ISO_TIME = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})\b/g
/** Terminal views are named inyon-view-<6 random characters> (crates/inyon-core/src/terminal/terminal.rs). */
const VIEW = /\binyon-view-[a-z0-9]{1,8}\b/g
/** Paste buffers are named inyon-<uuid> (type_text in crates/inyon-core/src/tmux/tmux.rs); the uuid rule numbers them. */

/**
 * A clock reading from this run: from two days before it started to 40 days after (a session
 * cookie lasts 30). Fixtures are dated January 2026, so they stay outside it.
 */
const BEFORE_MS = 2 * 86_400_000
const AFTER_MS = 40 * 86_400_000

const order = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0)

export const RULES: Rule[] = [
  {
    name: 'learned',
    why: 'Values the hub or the harness made up for this run and the harness was handed: tokens, setup and pairing codes, cookies, hub and phone ids, temp paths, the port. Replaced wherever they appear, longest first. Some (the computer\'s name, which can be a plain word) only when they are the whole string.',
    string: (s, n) => n.replaceLearned(s),
  },
  {
    name: 'uuid',
    why: 'Random UUIDs (session nonces, send ids, paste buffers). Numbered, so identity is kept.',
    string: (s, n) => s.replace(UUID, (m) => n.placeholder('uuid', m.toLowerCase())),
  },
  {
    name: 'sha256',
    why: 'Checksums of build outputs and of the install script inside pairing commands, which change with every commit. Not the harness\'s own hash of a long response body (contentSha256): that one is the content and must match.',
    string: (s, n, key) => (key === 'contentSha256' ? s : s.replace(SHA256, (m) => n.placeholder('sha256', m))),
  },
  {
    name: 'view-name',
    why: 'Terminal view sessions get a random name (Math.random in terminal.ts).',
    string: (s) => s.replace(VIEW, 'inyon-view-<random>'),
  },
  {
    name: 'iso-time',
    why: 'ISO timestamps from this run (fixture timestamps are older and stay).',
    string: (s, n) => s.replace(ISO_TIME, (m) => (n.isNow(Date.parse(m)) ? '<time>' : m)),
  },
  {
    name: 'epoch',
    why: 'Clock readings in ms or s since the epoch from this run: updatedAt, lastSeen, startedAt, expiresAt, cookie expiries (30 days ahead).',
    number: (v, n) => (n.isNow(v) ? '<time>' : n.isNow(v * 1000) && Number.isInteger(v) ? '<time:s>' : undefined),
  },
  {
    name: 'key-order',
    why: 'The order of an object\'s fields. JSON objects are unordered and every client reads them with JSON.parse; the Bun hub writes fields in the order its code happened to add them (seq last, error after updatedAt, a different order per agent), a Rust struct in the order it declares them. Fields are sorted by their normalized name, before their values, so placeholders are numbered the same either way. A key that is an id not seen before (a map keyed by session ids) has no name yet: such keys are numbered in the order of what they hold, so two maps alike but for order still match. Arrays keep their order.',
    object: (o, n) => {
      const keys = Object.keys(o).map((k) => n.peek(k))
      // what a field holds only matters for keys alike so far, and peeking at it all costs
      const alike = new Set(keys).size < keys.length
      const rough = Object.entries(o).map(([k, v], i) => [keys[i], alike ? n.peek(v) : '', k, v] as const)
      rough.sort(([a, x], [b, y]) => order(a, b) || order(x, y))
      const named = rough.map(([, , k, v]) => [n.string(k), k, v] as const)
      named.sort(([a], [b]) => order(a, b))
      return Object.fromEntries(named.map(([, k, v]) => [k, v]))
    },
  },
  {
    name: 'latency',
    why: 'How long the machine took to answer a probe (latencyMs).',
    number: (_v, _n, key) => (key === 'latencyMs' ? '<ms>' : undefined),
  },
]

export class Normalizer {
  private learned: { value: string; as: string }[] = []
  private exact = new Map<string, string>()
  private numbered = new Map<string, Map<string, string>>()
  /** numbering for comparisons made while the run goes on (see key), kept apart from the transcript's */
  private compared = new Map<string, Map<string, string>>()
  /** while peeking, a value not numbered yet reads as `<kind:?>` and stays unnumbered */
  private peeking = false

  constructor(
    readonly startedAt = Date.now(),
    readonly rules: Rule[] = RULES,
  ) {}

  /**
   * Replaces `value` with `<as>` (or `<as:n>` when `numbered`) wherever it shows up from now on.
   * Values shorter than 4 characters are refused: they would match inside unrelated text.
   */
  learn(value: string | undefined, as: string, numbered = false) {
    if (!value || value.length < 4) return
    if (this.learned.some((l) => l.value === value)) return
    const label = numbered ? this.placeholder(as, value) : `<${as}>`
    this.learned.push({ value, as: label })
    this.learned.sort((a, b) => b.value.length - a.value.length)
  }

  /** Replaces `value` with `<as>` only where it is a whole string (a JSON value or key). */
  learnExact(value: string | undefined, as: string) {
    if (value) this.exact.set(value, `<${as}>`)
  }

  replaceLearned(s: string) {
    const whole = this.exact.get(s)
    if (whole) return whole
    for (const { value, as } of this.learned) if (s.includes(value)) s = s.split(value).join(as)
    return s
  }

  /** `<kind:n>`, the same n for the same value within this normalizer */
  placeholder(kind: string, value: string) {
    let m = this.numbered.get(kind)
    if (!m) this.numbered.set(kind, (m = new Map()))
    let p = m.get(value)
    if (!p && this.peeking) return `<${kind}:?>`
    if (!p) m.set(value, (p = `<${kind}:${m.size + 1}>`))
    return p
  }

  /** The normalized JSON of `value` as far as it is known now, numbering nothing: what it holds, apart from ids not seen yet. */
  peek(value: unknown) {
    const was = this.peeking
    this.peeking = true
    try {
      return JSON.stringify(this.value(value))
    } finally {
      this.peeking = was
    }
  }

  /**
   * The normalized JSON of `value`, for telling whether two values differ only in what the rules
   * replace. Uses its own numbering, so it doesn't change the transcript's placeholders.
   */
  key(value: unknown) {
    const own = this.numbered
    this.numbered = this.compared
    try {
      return JSON.stringify(this.value(value))
    } finally {
      this.numbered = own
    }
  }

  isNow(ms: number) {
    return Number.isFinite(ms) && ms > this.startedAt - BEFORE_MS && ms < this.startedAt + AFTER_MS
  }

  string(s: string, key = '') {
    for (const r of this.rules) if (r.string) s = r.string(s, this, key) ?? s
    return s
  }

  number(v: number, key = ''): string | number {
    for (const r of this.rules) {
      const out = r.number?.(v, this, key)
      if (out !== undefined) return out
    }
    return v
  }

  /** A deep copy of `value` with every rule applied; object keys are normalized too (and sorted, see key-order). */
  value(value: unknown, key = ''): unknown {
    if (typeof value === 'string') return this.string(value, key)
    if (typeof value === 'number') return this.number(value, key)
    if (Array.isArray(value)) return value.map((v) => this.value(v, key))
    if (value && typeof value === 'object') {
      let obj = value as Record<string, unknown>
      for (const r of this.rules) obj = r.object?.(obj, this, key) ?? obj
      const out: Record<string, unknown> = {}
      for (const [k, v] of Object.entries(obj)) out[this.string(k)] = this.value(v, k)
      return out
    }
    return value
  }
}
