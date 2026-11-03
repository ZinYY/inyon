import { useEffect, type CSSProperties, type ReactNode } from 'react'
import clsx from 'clsx'
import { loadSerifFont } from './serif.ts'
import { agentDef, knownAgent, type AgentKind } from '@inyon/protocol'
import { useStore } from './store.ts'
import { publicFile } from './base.ts'
import { AntigravityMark, CodexMark, TileMark } from '../components/ui/AgentMarks.tsx'
import { AGENT_TILES } from '../components/ui/agentTiles.ts'

/** CSS variables that re-skin everything inside a session to match its agent. */
export function skinStyle(kind: AgentKind | undefined, theme: 'light' | 'dark'): CSSProperties | undefined {
  if (!kind) return undefined
  const t = agentDef(kind).skin[theme]
  return {
    '--bg': t.bg,
    '--panel': t.panel,
    '--raised': t.raised,
    '--hover': t.hover,
    '--sidebar': t.raised,
    '--line': t.line,
    '--line-strong': t.lineStrong,
    '--fg': t.fg,
    '--fg-2': t.fg2,
    '--fg-3': t.fg3,
    '--accent': t.accent,
    '--accent-fg': t.accentFg,
    '--accent-soft': t.accentSoft,
    '--accent-text': t.accentText,
    '--bubble': t.bubble,
    '--code': t.code,
    background: t.bg,
    color: t.fg,
  } as CSSProperties
}

/** Only an agent's accent colours, for a card of that agent on a page that isn't skinned. */
export function useAgentAccent(kind: AgentKind | undefined): CSSProperties | undefined {
  const theme = useStore((s) => s.resolvedTheme)
  const on = useStore((s) => s.settings.agentSkins)
  if (!kind || !on) return undefined
  const t = agentDef(kind).skin[theme]
  return { '--accent': t.accent, '--accent-fg': t.accentFg, '--accent-soft': t.accentSoft, '--accent-text': t.accentText } as CSSProperties
}

/** Applies an agent's skin (when skins are on) to its children. */
export function SkinScope({ agent, className, children }: { agent?: AgentKind; className?: string; children: ReactNode }) {
  const theme = useStore((s) => s.resolvedTheme)
  const on = useStore((s) => s.settings.agentSkins)
  return (
    <div className={className} style={on ? skinStyle(agent, theme) : undefined} data-agent={agent}>
      {children}
    </div>
  )
}

export const agentLogo = (kind: AgentKind) => publicFile(`agents/${kind}.svg`)

export function AgentLogo({ kind, size = 20, className, active }: { kind?: AgentKind; size?: number; className?: string; active?: boolean }) {
  if (active && (kind === 'codex' || kind === 'antigravity')) {
    const Mark = kind === 'codex' ? CodexMark : AntigravityMark
    return (
      <span role="img" aria-label={agentDef(kind).name} className={clsx('inline-flex shrink-0', className)} style={{ width: size, height: size }}>
        <Mark active className="size-full" />
      </span>
    )
  }
  // the other tiles animate their mark in place, so the icon doesn't jump when work starts
  if (active && kind && kind in AGENT_TILES) {
    return (
      <span role="img" aria-label={agentDef(kind).name} className={clsx('inline-flex shrink-0', className)} style={{ width: size, height: size }}>
        <TileMark kind={kind as keyof typeof AGENT_TILES} active className="size-full" />
      </span>
    )
  }
  // no agent, or one this build has no mark for (a newer hub's): a plain terminal tile
  if (!kind || !knownAgent(kind)) {
    return (
      <span
        className={clsx('inline-flex shrink-0 items-center justify-center rounded-[28%] border border-line bg-sunken text-fg-2', className)}
        style={{ width: size, height: size }}
        aria-hidden
      >
        <svg viewBox="0 0 24 24" width={size * 0.58} height={size * 0.58} fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round">
          <path d="M5 7l5 5-5 5" />
          <path d="M13 17h6" />
        </svg>
      </span>
    )
  }
  return (
    <img
      src={agentLogo(kind)}
      alt={agentDef(kind).name}
      width={size}
      height={size}
      draggable={false}
      className={clsx('shrink-0 select-none rounded-[28%]', className)}
      style={{ width: size, height: size }}
    />
  )
}

/** Reply font for an agent, unless the user overrides it. */
export function useReplyFont(kind?: AgentKind): 'serif' | 'sans' | 'mono' {
  const override = useStore((s) => s.settings.replyFont)
  const font = override !== 'auto' ? override : kind ? agentDef(kind).replyFont : 'sans'
  // the serif face isn't in the first load; the first session that reads in it fetches it
  useEffect(() => {
    if (font === 'serif') loadSerifFont()
  }, [font])
  return font
}
