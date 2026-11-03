import clsx from 'clsx'
import { agentDef, type AgentKind, type AgentState, type HostConfig, type HostStatus, type TurnFailure } from '@inyon/protocol'

/**
 * The agent's own spinner glyphs (Claude's ✻ cycle, Codex's braille dots, …). The frames sit in a
 * column that CSS steps through, so a screen full of working sessions costs no re-renders.
 */
export function AgentSpinner({ kind = 'claude', active, className }: { kind?: AgentKind; active?: boolean; className?: string }) {
  const frames = agentDef(kind).spinner
  const ms = (kind === 'claude' ? 120 : 90) * frames.length
  return (
    <span aria-hidden className={clsx('inline-flex h-[1em] select-none items-start justify-center overflow-hidden font-mono leading-none text-accent', className)}>
      <span
        className="flex flex-col"
        style={active ? { animation: `spinner-frames ${ms}ms steps(${frames.length}) infinite`, ['--frames' as string]: frames.length } : { transform: `translateY(-${Math.floor(frames.length / 2)}em)` }}
      >
        {frames.map((f, i) => (
          <span key={i} className="block h-[1em] leading-none">{f}</span>
        ))}
      </span>
    </span>
  )
}

/** What a session shows: an idle agent with shells or tasks still running counts as busy in the background. */
export type ShownState = AgentState | 'background' | 'offline'
/** On a machine that isn't answering, a session is shown as it was last seen: "Offline". */
export const shownState = (a: { state: AgentState; background?: string }, host?: { status: HostStatus }): ShownState =>
  host && host.status !== 'online' ? 'offline' : a.state === 'idle' && a.background ? 'background' : a.state

/** "1 background task" → "1 task running in the background"; "1 shell, 1 agent" → "1 shell and 1 agent running in the background" */
export const backgroundText = (bg: string) => {
  const parts = bg.split(', ').map((p) => p.replace(/\bbackground /, ''))
  const list = parts.length > 1 ? `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}` : parts[0]
  return `${list} running in the background`
}

const FAILURE_WORDS: Record<string, string> = {
  rate_limit: 'rate limit',
  billing_error: 'billing problem',
  authentication_failed: 'signed out',
  overloaded: 'servers overloaded',
  server_error: 'server error',
  invalid_request: 'request refused',
  max_output_tokens: 'reply too long',
}

/** "Stopped: rate limit · resets 14:05", or the day too when the reset is more than 20 hours away. */
export function failureText(f: TurnFailure, now = Date.now()) {
  const words = `Stopped: ${FAILURE_WORDS[f.error] ?? 'an error'}`
  const at = f.resetsAt ? f.resetsAt * 1000 : 0
  if (at <= now) return words
  const time = new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  const day = at - now > 20 * 3600_000 ? `${new Date(at).toLocaleDateString([], { weekday: 'short' })} ` : ''
  return `${words} · resets ${day}${time}`
}

const stateMeta: Record<ShownState, { label: string; dot: string; pill: string }> = {
  // chips stay neutral and the dot carries the state: ink with a ring while working, colour only
  // when something needs you
  working: { label: 'Working', dot: 'bg-fg live-ring', pill: 'border-line bg-white/45 text-fg-2 dark:bg-white/[0.03] text-fg' },
  // work still going on behind an idle prompt: ink, pulsing, so it isn't taken for idle
  background: { label: 'Background', dot: 'bg-fg-2 pulse-soft', pill: 'border-line bg-white/45 text-fg-2 dark:bg-white/[0.03]' },
  waiting: { label: 'Needs you', dot: 'bg-warn', pill: 'border-line bg-white/45 text-fg-2 dark:bg-white/[0.03] text-fg' },
  idle: { label: 'Idle', dot: 'bg-fg-3', pill: 'border-line bg-white/45 text-fg-2 dark:bg-white/[0.03]' },
  unknown: { label: 'Running', dot: 'bg-fg-3', pill: 'border-line bg-white/45 text-fg-2 dark:bg-white/[0.03]' },
  offline: { label: 'Offline', dot: 'bg-fg-3/50', pill: 'border-line text-fg-3' },
}

/** An SSH server's address, and the paired computer it's reached through if it isn't the phone. */
export function sshAddress(config: HostConfig, hosts: { config: HostConfig }[]) {
  const via = config.via ? hosts.find((h) => h.config.id === config.via)?.config.name ?? 'a removed computer' : undefined
  return via ? `${config.target} via ${via}` : config.target ?? ''
}

/** One line about a machine: its sessions when online, when it was last seen when not. */
export function hostLine(h: { status: HostStatus; error?: string; lastSeen?: number; offlineSince?: number }, online: string) {
  if (h.status === 'online') return online
  if (h.lastSeen) return `Offline · last seen ${ago(h.lastSeen)}`
  // the reason, when there is one, beats a bare "Connecting…"
  return h.status === 'error' ? h.error ?? 'Not reachable' : h.error ?? 'Connecting…'
}

function ago(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.round(s / 60)} min ago`
  if (s < 86400) return `${Math.round(s / 3600)} h ago`
  return `${Math.round(s / 86400)} d ago`
}

export function StatePill({ state, title, className }: { state: ShownState; title?: string; className?: string }) {
  const m = stateMeta[state]
  // the dot's colour and the chip's fade to the new state; the word comes in fresh
  return (
    <span title={title} className={clsx('inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2.5 font-mono text-[12px] transition-colors', m.pill, className)}>
      <span className={clsx('size-1.5 rounded-full transition-colors', m.dot)} />
      <span key={m.label} className="animate-fade-in">{m.label}</span>
    </span>
  )
}

export function StateDot({ state, className }: { state: ShownState; className?: string }) {
  return <span className={clsx('inline-block size-[7px] shrink-0 rounded-full transition-colors', stateMeta[state].dot, className)} />
}

export function HostDot({ status }: { status: HostStatus }) {
  return (
    <span
      className={clsx(
        'inline-block size-[7px] shrink-0 rounded-full transition-colors',
        status === 'online' && 'bg-fg',
        status === 'connecting' && 'bg-fg-3 pulse-soft',
        status === 'error' && 'bg-danger',
      )}
    />
  )
}
