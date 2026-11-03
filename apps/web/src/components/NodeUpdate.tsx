import { useEffect, useRef, useState } from 'react'
import clsx from 'clsx'
import { AlertCircle, ArrowUpCircle, CheckCircle2 } from 'lucide-react'
import type { HostState } from '@inyon/protocol'
import { api } from '../lib/api.ts'
import { haptic } from '../lib/haptics.ts'
import { Button, Pulse } from './ui/primitives.tsx'

/**
 * A machine whose inyon is behind: what's available and an Update button (only ever on a
 * tap). While it runs it says so in a sentence, then that it worked once the machine is back on
 * the new version, or what went wrong.
 */
export function NodeUpdate({ host }: { host: HostState }) {
  const u = host.nodeUpdate
  const [asking, setAsking] = useState(false)
  const [refused, setRefused] = useState('')
  const last = useRef(u?.state)
  useEffect(() => {
    if (u?.state !== last.current && (u?.state === 'updated' || u?.state === 'failed')) haptic(u.state === 'updated' ? 'success' : 'error')
    last.current = u?.state
    // what the hub says now replaces an earlier refusal
    if (u?.state) setRefused('')
  }, [u?.state])
  if (!u) return null

  const start = async () => {
    setAsking(true)
    setRefused('')
    try {
      await api.nodeUpdate(host.config.id)
    } catch (e) {
      setRefused((e as Error).message)
    } finally {
      setAsking(false)
    }
  }

  const online = host.status === 'online'
  const failed = u.state === 'failed' || !!refused
  const icon =
    u.state === 'updating' ? <span className="flex size-4 items-center justify-center"><Pulse /></span>
    : u.state === 'updated' ? <CheckCircle2 className="size-4 text-fg" />
    : failed ? <AlertCircle className="size-4 text-danger" />
    : <ArrowUpCircle className="size-4 text-fg-2" />
  const title =
    u.state === 'updating' ? 'Updating inyon…'
    : u.state === 'updated' ? `Updated to inyon ${u.latest}`
    : failed ? "Couldn't update inyon"
    : `inyon ${host.nodeVersion ?? ''} · update available`
  const detail =
    u.state === 'updating' ? 'It restarts and reconnects by itself, usually within a minute.'
    : u.state === 'updated' ? undefined
    : failed ? (refused || u.error || 'Something went wrong.')
    : !u.supported ? `This version can't update itself. Run the setup command on ${host.config.name} again to get ${u.latest}.`
    : online ? `Version ${u.latest} is ready. inyon restarts for a moment while it updates.`
    : `Version ${u.latest} is ready. Update it once ${host.config.name} is back online.`
  const button = u.supported && u.state !== 'updating' && u.state !== 'updated'

  return (
    <div role="status" className={clsx('animate-slide-up mb-4 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-2xl border px-3.5 py-3', failed ? 'border-transparent bg-danger-soft' : 'border-line bg-elev')}>
      <span className="shrink-0">{icon}</span>
      <div className="min-w-0 flex-[1_1_200px]">
        <div className="text-[13.5px] font-medium">{title}</div>
        {detail && <div className={clsx('mt-0.5 break-words text-[12.5px]', failed ? 'text-danger' : 'text-fg-3')}>{detail}</div>}
      </div>
      {button && (
        <Button size="sm" variant="primary" onClick={start} loading={asking} disabled={!online}>
          {failed ? 'Try again' : 'Update'}
        </Button>
      )}
    </div>
  )
}

/** A short label for the machine list: the update is there to see on the machine's page. */
export function nodeUpdateLabel(host: HostState) {
  const u = host.nodeUpdate
  if (!u || u.state === 'updated') return undefined
  return u.state === 'updating' ? 'Updating…' : u.state === 'failed' ? 'Update failed' : 'Update available'
}
