import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { Check, ChevronDown, Copy, Minus, RotateCcw, TriangleAlert, X } from 'lucide-react'
import type { HostState } from '@inyon/protocol'
import { Button, Sheet, Spinner } from './ui/primitives.tsx'
import { Fold, useFold } from './ui/Fold.tsx'
import { useStore, toast } from '../lib/store.ts'
import { runChecks, type CheckDeps, type Sock, type Step, type StepAction, type Target } from '../lib/connectCheck.ts'
import { buildReport } from '../lib/connectReport.ts'
import { connLog, downFor, useLastDrop, why } from '../lib/connlog.ts'
import { checkPlain, hubById, hubToken, hubWsUrl, splitHostId } from '../lib/hubs.ts'
import { wsUrl } from '../lib/hub.ts'
import { isPhone } from '../lib/phone.ts'
import { nativePower, platform, plugin } from '../lib/native.ts'
import { allowBackground, keepAliveSupported, keepAliveWanted, openBatterySettings, setKeepAlive } from '../lib/keepalive.ts'
import { copyText } from '../lib/clipboard.ts'
import { COMMIT, VERSION } from '../lib/version.ts'
import { timeAgo } from '../lib/format.ts'

// ---------- which hub or computer, opened from anywhere ----------

let openId: string | undefined
const heard = new Set<() => void>()
function setOpenId(id: string | undefined) {
  openId = id
  for (const fn of heard) fn()
}
/** Opens "Can't connect?" for a hub (`self`, `hub:<id>`) or a machine's id. */
export const openConnectHelp = (id: string) => setOpenId(id)
const closeConnectHelp = () => setOpenId(undefined)
const useOpenId = () =>
  useSyncExternalStore(
    (fn) => (heard.add(fn), () => void heard.delete(fn)),
    () => openId,
  )

/** "Can't connect?" as a quiet link after a sentence about reconnecting. */
export function HelpLink({ id, className }: { id: string; className?: string }) {
  return (
    <button type="button" onClick={() => openConnectHelp(id)} className={clsx('relative ml-1.5 text-fg-2 underline after:absolute after:-inset-x-1 after:-inset-y-3 decoration-line-strong underline-offset-2 hover:text-fg', className)}>
      Can't connect?
    </button>
  )
}

/** "Last drop 5m ago, the network went away", or nothing when none is recorded. */
export function LastDrop({ id, className }: { id: string; className?: string }) {
  const drop = useLastDrop(id)
  if (!drop) return null
  return (
    <span className={clsx('block text-[12px] leading-snug text-fg-3', className)}>
      Last drop {timeAgo(drop.at)}, {why(drop)}
      {drop.back !== undefined ? ` (down ${downFor(drop)})` : ''}
    </span>
  )
}

/** The id the connection log keeps a paired hub under. */
export const hubLogId = (hubId: string) => `hub:${hubId}`

/** What "Can't connect?" checks for an id, from what this app knows now. */
export function targetFor(id: string, hosts: HostState[]): Target | undefined {
  const self = (): Extract<Target, { kind: 'self' }> => ({ kind: 'self', id: 'self', name: location.host, origin: location.origin })
  if (id === 'self') return isPhone() ? undefined : self()
  if (id.startsWith('hub:') && !id.includes('/')) {
    const hub = hubById(id.slice(4))
    return hub && { kind: 'hub', id, name: hub.name, hub }
  }
  const host = hosts.find((h) => h.config.id === id)
  if (!host) return
  const { hub } = splitHostId(id)
  if (hub) return { kind: 'machine', id, name: host.config.name, host, via: { kind: 'hub', id: hubLogId(hub.id), name: hub.name, hub } }
  if (host.config.kind === 'link') return { kind: 'computer', id, name: host.config.name, host }
  return isPhone() ? undefined : { kind: 'machine', id, name: host.config.name, host, via: self() }
}

const KIND = { self: 'hub', hub: 'hub', computer: 'computer', machine: 'machine' } as const

/** The real network, sockets and phone. */
function deps(): CheckDeps {
  const device = plugin('InyonDevice')
  const android = platform() === 'android' && typeof device?.powerState === 'function'
  return {
    fetch: (url, init) => fetch(url, init),
    socket: (url) => new WebSocket(url) as unknown as Sock,
    online: () => navigator.onLine !== false,
    ...(typeof device?.lookUp === 'function' && {
      lookUp: async (host: string) => ((await device.lookUp({ host })) as { addresses?: string[] } | undefined)?.addresses ?? [],
    }),
    checkPlain,
    token: hubToken,
    wsUrl,
    hubWsUrl,
    reconnect: () => void import('../lib/phone.ts').then(({ phone }) => phone()).then((r) => r.resume(), () => {}),
    host: (id) => useStore.getState().hosts.find((h) => h.config.id === id),
    ...(android && { power: nativePower, keepAliveWanted }),
  }
}

/** The report for a target and its checks: everything a helper needs, nothing secret. */
async function report(t: Target | undefined, steps: Step[]) {
  const app = await Promise.resolve(plugin('App')?.getInfo?.()).catch(() => undefined) as { version?: string; build?: string } | undefined
  const power = await nativePower()
  return buildReport({
    now: Date.now(),
    version: VERSION,
    commit: COMMIT || undefined,
    ...(app?.version && { app: { version: app.version, build: app.build ? String(app.build) : undefined } }),
    platform: platform() === 'web' ? (isPhone() ? 'browser copy' : 'browser') : `${platform()} app`,
    userAgent: navigator.userAgent,
    online: navigator.onLine !== false,
    hidden: document.visibilityState === 'hidden',
    ...(power && { power, keepAlive: keepAliveSupported() ? keepAliveWanted() : undefined }),
    ...(t && { target: { name: t.name, kind: KIND[t.kind], address: t.kind === 'self' ? t.origin : t.kind === 'hub' ? t.hub.url : t.kind === 'machine' ? t.via.name : undefined } }),
    steps,
    drops: connLog.recent(10),
  })
}

// ---------- the sheet ----------

/** Mounted once, in the app's shell. */
export function ConnectHelpHost() {
  const id = useOpenId()
  const [shownId, setShownId] = useState(id)
  useEffect(() => {
    if (id) setShownId(id)
  }, [id])
  return <ConnectSheet id={shownId} open={!!id} onClose={closeConnectHelp} />
}

const ACTIONS: Record<StepAction, string> = {
  'sign-in': 'Sign in',
  'pair-hub': 'Pair again',
  'pair-computer': 'Pair again',
  battery: 'Allow',
  'battery-settings': 'Open settings',
  'stay-connected': 'Turn on',
}

export function ConnectSheet({ id, open, onClose, checkDeps }: { id: string | undefined; open: boolean; onClose: () => void; checkDeps?: CheckDeps }) {
  const hosts = useStore((s) => s.hosts)
  const [target, setTarget] = useState<Target | undefined>()
  const [steps, setSteps] = useState<Step[]>([])
  const [running, setRunning] = useState(false)
  const [text, setText] = useState('')
  const [details, mounted, toggle] = useFold()
  const run = useRef(0)
  const nav = useNavigate()
  const drop = useLastDrop(id)

  const start = useCallback(() => {
    if (!id) return
    const t = targetFor(id, useStore.getState().hosts)
    setTarget(t)
    if (!t) return setSteps([])
    const mine = ++run.current
    setRunning(true)
    // the last run's report doesn't match these checks
    setText('')
    void runChecks(t, checkDeps ?? deps(), (s) => mine === run.current && setSteps(s), () => mine !== run.current).then((s) => {
      if (mine !== run.current) return
      setRunning(false)
      void report(t, s).then((r) => mine === run.current && setText(r))
    })
  }, [id, checkDeps])

  // every opening runs the checks afresh; closing stops a run that's still going
  useEffect(() => {
    if (!open) return void run.current++
    setSteps([])
    setText('')
    start()
  }, [open, id])

  const act = async (a: StepAction) => {
    if (a === 'sign-in') return onClose(), window.dispatchEvent(new Event('inyon:signed-out'))
    if (a === 'pair-hub' || a === 'pair-computer') return onClose(), nav('/machines?add=1')
    if (a === 'battery') await allowBackground()
    if (a === 'battery-settings') await openBatterySettings()
    if (a === 'stay-connected') await setKeepAlive(true).catch(() => {})
    start()
  }

  const copy = async () => {
    const body = text || (await report(target, steps))
    if (await copyText(body)) toast({ title: 'Report copied', body: 'Paste it to whoever is helping. Tokens, codes and keys are left out.', tone: 'success' })
  }

  const name = target?.name ?? hosts.find((h) => h.config.id === id)?.config.name ?? 'this connection'
  const done = !running && steps.length > 0
  const allGood = done && steps.every((s) => s.state === 'pass' || s.state === 'skip')
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Can't connect?"
      footer={
        <div className="flex flex-wrap justify-end gap-2">
          <Button size="sm" onClick={copy} disabled={!target}>
            <Copy className="size-3.5" /> Copy report
          </Button>
          <Button size="sm" variant="primary" onClick={start} disabled={running || !target}>
            <RotateCcw className="size-3.5" /> Check again
          </Button>
        </div>
      }
    >
      <p className="text-[13px] text-fg-3">
        <span className="text-fg-2">{name}</span>
        {target && ` · ${KIND[target.kind]}`}
        {drop && ` · last drop ${timeAgo(drop.at)}, ${why(drop)}`}
        {drop && drop.back !== undefined && ` (down ${downFor(drop)})`}
      </p>
      {!target && <p className="mt-4 text-[14px] text-fg-2">There's nothing this app can check for it from here.</p>}
      <ol className="mt-2 divide-y divide-line" aria-label="Checks">
        {steps.map((s) => (
          <StepRow key={s.id} step={s} onAction={act} />
        ))}
      </ol>
      {allGood && <p className="animate-fade-in mt-3 text-[13px] text-fg-3">Everything here works, so it should reconnect by itself. If it doesn't, copy the report for whoever is helping.</p>}
      {done && (
        <div className="mt-3">
          <button type="button" onClick={toggle} aria-expanded={details} className="flex h-10 items-center gap-1.5 text-[13px] font-medium text-fg-2 hover:text-fg">
            <ChevronDown className={clsx('size-4 transition-transform duration-(--dur-small)', !details && '-rotate-90')} /> Details
          </button>
          <Fold open={details} mounted={mounted}>
            <pre className="max-h-[40dvh] overflow-auto rounded-2xl bg-sunken p-3 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-words text-fg-2">{text || '…'}</pre>
          </Fold>
        </div>
      )}
    </Sheet>
  )
}

function StepIcon({ state }: { state: Step['state'] }) {
  const box = 'flex size-5 shrink-0 items-center justify-center rounded-full'
  if (state === 'running') return <span className={box}><Spinner className="size-4" /></span>
  if (state === 'pass') return <span className={clsx(box, 'bg-sunken text-fg-2')}><Check className="size-3.5" aria-label="Passed" /></span>
  if (state === 'fail') return <span className={clsx(box, 'bg-danger-soft text-danger')}><X className="size-3.5" aria-label="Failed" /></span>
  if (state === 'warn') return <span className={clsx(box, 'bg-warn-soft text-warn')}><TriangleAlert className="size-3" aria-label="Worth a look" /></span>
  if (state === 'skip') return <span className={clsx(box, 'text-fg-3')}><Minus className="size-3.5" aria-label="Skipped" /></span>
  return <span className={box}><span className="size-[7px] rounded-full border border-fg-3" aria-label="Waiting" /></span>
}

function StepRow({ step: s, onAction }: { step: Step; onAction: (a: StepAction) => void }) {
  const sentence = s.state === 'running' ? s.doing : s.say
  const help = s.state === 'fail' || s.state === 'warn'
  return (
    <li className="flex gap-3 py-3" data-state={s.state}>
      <span className="mt-px">
        <StepIcon state={s.state} />
      </span>
      <div className="min-w-0 flex-1">
        <div className={clsx('text-[14px] font-medium', (s.state === 'pending' || s.state === 'skip') && 'text-fg-3')}>{s.title}</div>
        {sentence && (
          <p key={`${s.state}`} className={clsx('animate-fade-in mt-0.5 text-[13px] leading-snug break-words', s.state === 'skip' ? 'text-fg-3' : 'text-fg-2')} role={s.state === 'running' ? 'status' : undefined}>
            {sentence}
          </p>
        )}
        {help && s.fix && <p className="animate-fade-in mt-1.5 text-[13px] leading-snug break-words text-fg">{s.fix}</p>}
        {help && s.action && (
          <Button size="xs" className="mt-2" onClick={() => onAction(s.action!)}>
            {ACTIONS[s.action]}
          </Button>
        )}
      </div>
    </li>
  )
}
