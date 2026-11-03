/**
 * Golden transcript against a fresh one. Lines are compared whole; a changed line is also broken
 * down into the JSON paths that differ, since one line can hold a whole overview.
 */

type Op = { kind: 'same' | 'del' | 'add'; line: string; at: number }

/** Longest common subsequence diff over lines (transcripts are a few hundred lines). */
export function diffLines(a: string[], b: string[]): Op[] {
  const n = a.length
  const m = b.length
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!)
  const ops: Op[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) ops.push({ kind: 'same', line: a[i]!, at: i++ }), j++
    else if (lcs[i + 1]![j]! >= lcs[i]![j + 1]!) ops.push({ kind: 'del', line: a[i]!, at: i++ })
    else ops.push({ kind: 'add', line: b[j]!, at: j++ })
  }
  while (i < n) ops.push({ kind: 'del', line: a[i]!, at: i++ })
  while (j < m) ops.push({ kind: 'add', line: b[j]!, at: j++ })
  return ops
}

/** The JSON paths where two values differ, as "path: old → new". */
export function jsonChanges(a: unknown, b: unknown, path = '', out: string[] = [], limit = 30): string[] {
  if (out.length >= limit) return out
  if (JSON.stringify(a) === JSON.stringify(b)) return out
  const isObj = (v: unknown) => v !== null && typeof v === 'object'
  if (isObj(a) && isObj(b) && Array.isArray(a) === Array.isArray(b)) {
    const keys = [...new Set([...Object.keys(a as object), ...Object.keys(b as object)])]
    for (const k of keys) jsonChanges((a as any)[k], (b as any)[k], Array.isArray(a) ? `${path}[${k}]` : path ? `${path}.${k}` : k, out, limit)
    return out
  }
  const show = (v: unknown) => (v === undefined ? '(missing)' : JSON.stringify(v).slice(0, 200))
  out.push(`${path || '(line)'}: ${show(a)} → ${show(b)}`)
  return out
}

const parse = (l: string) => {
  try {
    return JSON.parse(l)
  } catch {
    return l
  }
}

/** A readable report, or '' when the transcripts match. */
export function report(golden: string[], fresh: string[], context = 1): string {
  const ops = diffLines(golden, fresh)
  if (ops.every((o) => o.kind === 'same')) return ''
  const out: string[] = []
  const show = (o: Op) => `${o.kind === 'same' ? ' ' : o.kind === 'del' ? '-' : '+'} ${o.line.length > 300 ? `${o.line.slice(0, 300)}…` : o.line}`
  let lastShown = -1
  for (let k = 0; k < ops.length; k++) {
    const o = ops[k]!
    if (o.kind === 'same') continue
    for (let c = Math.max(lastShown + 1, k - context); c < k; c++) out.push(show(ops[c]!))
    out.push(show(o))
    lastShown = k
    // a removed line followed by an added one: what changed inside it
    const next = ops[k + 1]
    if (o.kind === 'del' && next?.kind === 'add') {
      out.push(show(next))
      for (const c of jsonChanges(parse(o.line), parse(next.line))) out.push(`    ~ ${c}`)
      lastShown = ++k
    }
    for (let c = k + 1; c <= Math.min(ops.length - 1, k + context) && ops[c]!.kind === 'same'; c++) out.push(show(ops[c]!)), (lastShown = c)
  }
  return out.join('\n')
}
