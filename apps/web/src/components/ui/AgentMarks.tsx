import { useId } from 'react'
import clsx from 'clsx'
import { AGENT_TILES } from './agentTiles.ts'

/** Antigravity's arch, filled with its colour blobs. The marks hold still; the state dot says when an agent works. */
export function AntigravityMark({ className }: { active?: boolean; className?: string }) {
  const id = useId().replace(/:/g, '')
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={clsx('overflow-visible', className)}>
      <defs>
        <mask id={`${id}a`} maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24">
          <path
            fill="#fff"
            d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z"
          />
        </mask>
        <filter id={`${id}b`} x="-50%" y="-50%" width="200%" height="200%">
          <feGaussianBlur stdDeviation="2.6" />
        </filter>
      </defs>
      <g mask={`url(#${id}a)`}>
        <rect width="24" height="24" fill="#3186FF" />
        <g filter={`url(#${id}b)`}>
          <circle cx="5" cy="3" r="6" fill="#FFE432" />
          <circle cx="21" cy="5" r="6.5" fill="#FC413D" />
          <circle cx="1" cy="13" r="7" fill="#00B95C" />
          <circle cx="13" cy="23" r="8" fill="#3186FF" />
          <circle cx="23" cy="15" r="4.5" fill="#749BFF" />
          <circle cx="12" cy="9" r="3" fill="#FBBC04" />
        </g>
      </g>
    </svg>
  )
}

/** Codex's mark in the text colour. */
export function CodexMark({ className }: { active?: boolean; className?: string }) {
  const id = useId().replace(/:/g, '')
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={clsx('text-fg', className)}>
      <defs>
        <mask id={`${id}m`} maskUnits="userSpaceOnUse" x="-2" y="-2" width="28" height="28">
          <rect x="-2" y="-2" width="28" height="28" fill="#fff" />
          <path
            fill="#000"
            d="M7.282 8.307a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393z"
          />
          <path
            fill="#000"
            d="M12.728 14.547a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z"
          />
        </mask>
      </defs>
      <g mask={`url(#${id}m)`}>
        <path
          fill="currentColor"
          d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457z"
        />
      </g>
    </svg>
  )
}

/** The other agents' app-icon tiles, drawn inline. OpenCode's cursor block shows while it works. */
export function TileMark({ kind, active, className }: { kind: keyof typeof AGENT_TILES; active?: boolean; className?: string }) {
  const t = AGENT_TILES[kind]
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={className}>
      <rect width="24" height="24" rx="5.5" fill={t.bg} />
      {t.edge && <rect x="0.25" y="0.25" width="23.5" height="23.5" rx="5.5" fill="none" stroke={t.edge} strokeWidth="0.5" />}
      <g>
        <g transform={t.transform} fill={t.fg} fillRule="evenodd">
          {t.d.map((d, i) => <path key={i} d={d} />)}
          {kind === 'opencode' && <rect x="8" y="6" width="8" height="12" className={active ? undefined : 'opacity-0'} />}
        </g>
      </g>
    </svg>
  )
}
