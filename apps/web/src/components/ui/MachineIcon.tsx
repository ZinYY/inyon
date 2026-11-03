import type { HostState, MachineKind } from '@inyon/protocol'

/**
 * A machine's icon, drawn like the kite mark: rounded geometry on an even stroke, and one solid node.
 * The machine says what it is (the probe looks at its chassis, battery, display and hypervisor).
 */
export function MachineIcon({ kind, size = 18, className }: { kind?: MachineKind; size?: number; className?: string }) {
  const k = kind ?? 'desktop'
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} className={className} fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-label={LABEL[k]} role="img">
      {k === 'server' ? (
        <>
          <rect x="3.5" y="4" width="17" height="7" rx="2.5" />
          <rect x="3.5" y="13" width="17" height="7" rx="2.5" />
          <circle cx="7.5" cy="7.5" r="1.25" fill="currentColor" stroke="none" />
          <circle cx="7.5" cy="16.5" r="1.25" fill="currentColor" stroke="none" />
          <path d="M12 7.5h4.5M12 16.5h4.5" />
        </>
      ) : k === 'laptop' ? (
        <>
          <rect x="5" y="4.5" width="14" height="10.5" rx="2.5" />
          <path d="M2.75 19h18.5" />
          <circle cx="12" cy="9.75" r="1.25" fill="currentColor" stroke="none" />
        </>
      ) : (
        <>
          <rect x="3" y="3.5" width="18" height="12.5" rx="2.5" />
          <path d="M12 16v4M8.5 20h7" />
          <circle cx="12" cy="9.75" r="1.25" fill="currentColor" stroke="none" />
        </>
      )}
    </svg>
  )
}

const LABEL: Record<MachineKind, string> = { server: 'Server', laptop: 'Laptop', desktop: 'Desktop' }

/** The kind to draw: what the machine reported, or a server for this hub before its first look. */
export const machineKindOf = (h: HostState): MachineKind | undefined => h.machineKind ?? (h.config.kind === 'local' ? 'server' : undefined)

/** Online machines first, then the rest, each group in the order they were added. */
export function onlineFirst(hosts: HostState[]) {
  return [...hosts].sort((a, b) => Number(b.status === 'online') - Number(a.status === 'online'))
}
