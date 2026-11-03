import { useEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import { Check, Plug, Search, TriangleAlert } from 'lucide-react'
import type { OpenCodeProvider, ProvidersReport } from '@inyon/protocol'
import { api } from '../lib/api.ts'
import { toast } from '../lib/store.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Sheet, Waiting, inputClass } from './ui/primitives.tsx'
import { SignInSheet } from './SignInSheet.tsx'
import { confirmAction } from './Confirm.tsx'

/** Tells every open model list that the machine's providers changed, so it asks the hub again. */
export const modelsChanged = () => window.dispatchEvent(new Event('inyon:models-changed'))

const SHOWN = 12

/** How a connected provider got that way, in a word or two. */
function how(p: OpenCodeProvider) {
  if (p.via === 'env') return `From ${p.env?.[0] ?? 'the environment'}`
  if (p.via === 'credential') return p.type === 'oauth' ? 'Signed in' : p.type === 'api' ? 'API key' : 'Connected'
  return p.id === 'opencode' ? 'Free models, no sign-in' : 'From OpenCode’s config'
}

/**
 * OpenCode's model providers on one machine: the connected ones with Disconnect, and the rest to
 * connect. Connecting runs OpenCode's own sign-in on the machine, shown here as steps; a key
 * typed goes to that command and nowhere else.
 */
export function ProvidersSheet({ hostId, hostName, pane, open, onClose }: { hostId: string; hostName?: string; pane?: string; open: boolean; onClose: () => void }) {
  return (
    <Sheet open={open} onClose={onClose} title="OpenCode providers" icon={<AgentLogo kind="opencode" size={24} />}>
      {open && <Providers hostId={hostId} hostName={hostName} pane={pane} />}
    </Sheet>
  )
}

function Providers({ hostId, hostName, pane }: { hostId: string; hostName?: string; pane?: string }) {
  const [report, setReport] = useState<ProvidersReport | null>(null)
  const [failed, setFailed] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [connecting, setConnecting] = useState<OpenCodeProvider | null>(null)
  const search = useRef<HTMLInputElement>(null)

  const load = () => {
    setFailed(null)
    return api.providers(hostId, pane).then(setReport).catch((e) => setFailed((e as Error).message))
  }
  useEffect(() => {
    load()
  }, [hostId, pane])

  const connected = useMemo(() => report?.providers.filter((p) => p.connected) ?? [], [report])
  const others = useMemo(() => {
    const q = query.trim().toLowerCase()
    const rest = report?.providers.filter((p) => !p.connected) ?? []
    return q ? rest.filter((p) => p.name.toLowerCase().includes(q) || p.id.includes(q)) : rest
  }, [report, query])

  const disconnect = async (p: OpenCodeProvider) => {
    if (!(await confirmAction({ title: `Disconnect ${p.name}?`, body: `OpenCode on ${hostName ?? 'the machine'} forgets its credential. Its models leave the list.`, yes: 'Disconnect', danger: true }))) return
    setBusy(p.id)
    try {
      await api.disconnectProvider(hostId, p.id)
      toast({ title: `${p.name} disconnected`, tone: 'success' })
      modelsChanged()
      await load()
    } catch (e) {
      toast({ title: 'Could not disconnect', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }

  const credentials = connected.filter((p) => p.via === 'credential')
  const signOut = async () => {
    if (!(await confirmAction({ title: 'Sign out of every provider?', body: `${credentials.map((p) => p.name).join(', ')}: OpenCode on ${hostName ?? 'the machine'} forgets their credentials. Providers set through the environment stay.`, yes: 'Sign out', danger: true }))) return
    setBusy('all')
    try {
      for (const p of credentials) await api.disconnectProvider(hostId, p.id)
      toast({ title: 'Signed out of OpenCode', body: `${credentials.length} ${credentials.length === 1 ? 'provider' : 'providers'} disconnected`, tone: 'success' })
      modelsChanged()
      await load()
    } catch (e) {
      toast({ title: 'Could not sign out', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(null)
    }
  }

  if (failed || report?.error) {
    return (
      <div className="space-y-4">
        <div className="flex items-start gap-2 rounded-2xl bg-danger-soft px-3.5 py-3 text-[13.5px] text-danger">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" />
          <span className="min-w-0 break-words">{failed ?? report?.error}</span>
        </div>
        <div className="flex justify-end">
          <Button variant="primary" onClick={load}>Try again</Button>
        </div>
      </div>
    )
  }
  if (!report) return <Waiting shape="rows" count={3} label={`Reading providers on ${hostName ?? 'the machine'}`} />

  return (
    <div className="space-y-5">
      <section>
        <div className="mb-2 text-[13px] font-medium text-fg-2">Connected</div>
        {connected.length ? (
          <div className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
            {connected.map((p) => (
              <div key={p.id} className="flex items-center gap-3 px-4 py-3">
                <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-sunken text-fg">
                  <Check className="size-3" strokeWidth={3} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[14px] font-medium">{p.name}</div>
                  <div className="truncate font-mono text-[12px] text-fg-3">{how(p)}</div>
                </div>
                {p.via === 'credential' && (
                  <Button size="sm" variant="ghost" loading={busy === p.id} disabled={busy !== null} onClick={() => disconnect(p)}>
                    Disconnect
                  </Button>
                )}
              </div>
            ))}
          </div>
        ) : (
          <p className="text-[13px] text-fg-3">No provider connected on {hostName ?? 'this machine'}.</p>
        )}
      </section>

      <section>
        <div className="mb-2 text-[13px] font-medium text-fg-2">Connect a provider</div>
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-3" />
          <input
            ref={search}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && others[0]) {
                e.preventDefault()
                setConnecting(others[0])
              }
            }}
            placeholder="Search providers"
            aria-label="Search providers"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            className={clsx(inputClass, 'pl-9')}
          />
        </div>
        <div role="listbox" aria-label="Providers to connect" className="mt-2 divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
          {others.slice(0, SHOWN).map((p) => (
            <button
              key={p.id}
              type="button"
              role="option"
              aria-selected={false}
              onClick={() => setConnecting(p)}
              className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-hover focus-visible:bg-hover focus-visible:outline-none"
            >
              <Plug className="size-4 shrink-0 text-fg-3" />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[14px] font-medium">{p.name}</span>
                {p.env?.length ? <span className="block truncate font-mono text-[12px] text-fg-3">or set {p.env[0]}</span> : null}
              </span>
            </button>
          ))}
          {!others.length && <div className="px-4 py-3 text-[13px] text-fg-3">No provider matches “{query}”.</div>}
        </div>
        {others.length > SHOWN && <p className="mt-2 text-[12px] text-fg-3">{others.length - SHOWN} more. Keep typing to narrow the list.</p>}
      </section>

      <p className="text-[12px] leading-relaxed text-fg-3">
        A key you type goes to <span className="font-mono">opencode auth login</span> on {hostName ?? 'the machine'}, nowhere else. Sessions Inyon started see a change at once; an <span className="font-mono">opencode</span> already running on its own sees it after a restart.
      </p>
      {credentials.length > 0 && (
        <div className="flex justify-end">
          <Button variant="danger" size="sm" loading={busy === 'all'} disabled={busy !== null} onClick={signOut}>
            Sign out of OpenCode
          </Button>
        </div>
      )}

      <SignInSheet
        hostId={hostId}
        hostName={hostName}
        agent={connecting ? 'opencode' : null}
        provider={connecting ?? undefined}
        onClose={() => {
          setConnecting(null)
          setQuery('')
          modelsChanged()
          load()
        }}
      />
    </div>
  )
}
