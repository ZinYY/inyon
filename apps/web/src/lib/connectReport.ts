import { contextWords, downFor, why, type Drop, type Power, type Subject } from './connlog.ts'
import type { Step } from './connectCheck.ts'
import { redact } from './redact.ts'

/**
 * The plain-text report "Copy report" puts on the clipboard, for support or an agent to read:
 * what's installed, what the checks found and the last drops. It goes through `redact` as a
 * whole, so no token, cookie, ticket, pairing code or key leaves the device in it.
 */
export interface ReportInput {
  now: number
  version: string
  commit?: string
  /** the Android app's own version */
  app?: { version: string; build?: string }
  platform: string
  userAgent?: string
  online: boolean
  hidden: boolean
  power?: Power
  keepAlive?: boolean
  target?: { name: string; kind: string; address?: string }
  steps?: Step[]
  drops: { subject: Subject; drop: Drop }[]
}

const MARK: Record<Step['state'], string> = { pass: 'ok', warn: 'warn', fail: 'FAIL', skip: 'skip', pending: '-', running: '...' }

export const when = (t: number) => new Date(t).toISOString().replace('T', ' ').replace(/:\d\d\.\d+Z$/, ' UTC')

const yes = (v: boolean | undefined, on = 'on', off = 'off') => (v === undefined ? 'unknown' : v ? on : off)

export function buildReport(r: ReportInput): string {
  const lines: string[] = [`Inyon connection report, ${when(r.now)}`]
  lines.push(`App: Inyon ${r.version}${r.commit ? ` (${r.commit})` : ''}${r.app ? `, Android app ${r.app.version}${r.app.build ? ` build ${r.app.build}` : ''}` : ''}`)
  lines.push(`Platform: ${r.platform}${r.userAgent ? ` · ${r.userAgent}` : ''}`)
  lines.push(`Network: ${r.online ? 'online' : 'offline'} · page ${r.hidden ? 'hidden' : 'visible'}`)
  if (r.power || r.keepAlive !== undefined) {
    const p = r.power ?? {}
    lines.push(
      `Android: Stay connected ${yes(r.keepAlive)} · battery limits ${p.ignoringOptimizations === undefined ? 'unknown' : p.ignoringOptimizations ? 'off' : 'on'} · background restricted ${yes(p.backgroundRestricted, 'yes', 'no')} · battery saver ${yes(p.powerSave)} · Doze ${yes(p.idle, 'yes', 'no')}${p.standbyBucket ? ` · standby bucket ${p.standbyBucket}` : ''}`,
    )
  }
  if (r.target) {
    lines.push('', `Checks for ${r.target.name} (${r.target.kind}${r.target.address ? ` at ${r.target.address}` : ''})`)
    for (const s of r.steps ?? []) {
      lines.push(`  [${MARK[s.state]}] ${s.title}: ${s.say ?? (s.state === 'pending' ? 'not run' : '')}`)
      if (s.fix && (s.state === 'fail' || s.state === 'warn')) lines.push(`         Do: ${s.fix}`)
      if (s.detail) for (const d of s.detail.split('\n')) lines.push(`         ${d}`)
    }
  }
  lines.push('', 'Last drops')
  if (!r.drops.length) lines.push('  None recorded.')
  for (const { subject, drop } of r.drops) {
    const bits = [why(drop), drop.back !== undefined || drop.cut ? `down ${downFor(drop, r.now)}` : downFor(drop, r.now)]
    if (drop.code !== undefined) bits.push(`close code ${drop.code}`)
    if (drop.while.length) bits.push(`while ${contextWords(drop.while)}`)
    if (drop.tries) bits.push(`${drop.tries} failed ${drop.tries === 1 ? 'try' : 'tries'}${drop.last ? `, last: ${drop.last}` : ''}`)
    lines.push(`  ${when(drop.at)}  ${subject.name} (${subject.kind}): ${bits.join('; ')}`)
  }
  return redact(lines.join('\n'))
}
