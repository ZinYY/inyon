import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { agentDef, AGENTS, type AgentKind, type HostConfig } from '@inyon/protocol'
import { api, type CustomAgent } from '../lib/api.ts'
import { toast, useStore } from '../lib/store.ts'
import { allPanes, paneHref } from '../lib/overview.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Field, Segmented, Sheet, inputClass } from './ui/primitives.tsx'
import { Check, Copy, ShieldCheck } from 'lucide-react'
import { copyText } from '../lib/clipboard.ts'
import { isPhone } from '../lib/phone.ts'
import { Select } from './ui/Select.tsx'
import { useOpened } from '../lib/useOpened.ts'
import { Logo } from './AppShell.tsx'
import { tailPath } from '../lib/format.ts'
import { ServerSheet } from './ServerSheet.tsx'
import { LaptopSheet } from './LaptopSheet.tsx'

/** Pairing a machine that connects out to the hub: nothing is opened on that machine. */
export function PairMachine({ onClose, existing }: { onClose?: () => void; existing?: string }) {
  const [name, setName] = useState('')
  const [pair, setPair] = useState<{ id: string; command: string; commands?: { unix: string; windows: string } } | null>(null)
  // only a guess: the machine being added is often not the one with this browser
  const [os, setOs] = useState<'unix' | 'windows'>(() => (/Windows/.test(navigator.userAgent) ? 'windows' : 'unix'))
  const [busy, setBusy] = useState(false)
  const host = useStore((s) => (pair ? s.hosts.find((h) => h.config.id === pair.id) : undefined))
  const online = host?.status === 'online'
  const start = async () => {
    setBusy(true)
    try {
      setPair(await (existing ? api.repair(existing) : api.pair(name || 'Laptop')))
    } catch (e) {
      toast({ title: 'Could not create a pairing', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }
  if (!pair && existing) {
    return (
      <div className="space-y-3">
        <p className="text-[13.5px] leading-relaxed text-fg-2">
          This machine was added but hasn't connected yet. Run its pairing command on it. If you no longer have the command, make a new one; the old one stops working.
        </p>
        <Button variant="primary" onClick={start} loading={busy}>New pairing command</Button>
      </div>
    )
  }
  if (!pair) {
    return (
      <div className="space-y-4">
        <div className="flex gap-3 rounded-2xl bg-sunken px-3.5 py-3 text-[13px] leading-relaxed text-fg-2">
          <ShieldCheck className="mt-0.5 size-4 shrink-0 text-fg-2" />
          <span>
            You run one command on the machine you're adding. From then on it connects out to this hub by itself, so it works behind a router or firewall and you don't open SSH or any port on it. Best for laptops and anything the hub can't reach. Remove the machine to revoke it.
          </span>
        </div>
        <Field label="Name">
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder="My laptop" autoFocus />
        </Field>
        <div className="flex justify-end">
          <Button variant="primary" onClick={start} loading={busy}>Create pairing command</Button>
        </div>
      </div>
    )
  }
  const command = (pair.commands && pair.commands[os]) ?? pair.command
  return (
    <div className="space-y-4">
      {pair.commands && (
        <Segmented full value={os} onChange={setOs} options={[{ v: 'unix', label: 'macOS or Linux' }, { v: 'windows', label: 'Windows' }]} />
      )}
      <p className="text-[13.5px] leading-relaxed text-fg-2">
        {os === 'windows'
          ? 'Paste this once into PowerShell or Command Prompt on that machine. It installs a small background app for your user that starts when you sign in, plus psmux (tmux for Windows) if it is missing. No admin rights, no open ports.'
          : 'Run this once in a terminal on that machine. It downloads inyon from this hub and keeps it running in the background for your user. No sudo, no open ports.'}
      </p>
      <div className="rounded-2xl border border-line bg-code p-3">
        <code className="block break-all font-mono text-[12px] leading-relaxed">{command}</code>
        <Button size="sm" className="mt-2.5" onClick={() => copyText(command).then((ok) => toast(ok ? { title: 'Command copied', tone: 'success' } : { title: 'Could not copy', body: 'Select the command and copy it by hand.', tone: 'error' }))}>
          <Copy className="size-3.5" /> Copy command
        </Button>
      </div>
      <div className="flex items-center gap-2.5 text-[13px]">
        {online ? <Check className="size-4 text-fg" /> : <Logo className="size-4 mark-connecting" />}
        {online ? `${host!.config.name} is connected.` : 'Waiting for the machine to connect…'}
      </div>
      <p className="text-[12px] leading-relaxed text-fg-3">
        The command contains this machine's secret token: don't share it. Plain http is only accepted on private networks (home LAN, Tailscale, NetBird); otherwise put the hub behind https.
      </p>
      {onClose && (
        <div className="flex justify-end">
          <Button variant="primary" onClick={onClose}>{online ? 'Done' : 'Close'}</Button>
        </div>
      )}
    </div>
  )
}

type MachineSheetProps = { open: boolean; onClose: () => void; edit?: HostConfig; link?: string }

/**
 * The hub adds machines by pairing or over its own SSH. The phone app pairs a laptop by its code,
 * or SSHes into a server itself.
 */
export function MachineSheet(props: MachineSheetProps) {
  const [ssh, setSsh] = useState(false)
  useOpened(props.open, props.edit?.id, () => setSsh(false))
  if (!isPhone()) return <HubMachineSheet {...props} />
  if (props.edit) return props.edit.kind === 'link' ? <LaptopSheet {...props} /> : <ServerSheet {...props} />
  return ssh ? <ServerSheet {...props} onLaptop={() => setSsh(false)} /> : <LaptopSheet {...props} onServer={() => setSsh(true)} />
}

function HubMachineSheet({ open, onClose, edit }: MachineSheetProps) {
  const [mode, setMode] = useState<'pair' | 'ssh'>('pair')
  const [name, setName] = useState('')
  const [target, setTarget] = useState('')
  const [port, setPort] = useState('')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)

  useOpened(open, edit?.id, () => {
    setMode('pair')
    setName(edit?.name ?? '')
    setTarget(edit?.target ?? '')
    setPort(edit?.port ? String(edit.port) : '')
    setKey(edit?.identityFile ?? '')
  })

  const local = edit?.kind === 'local' || edit?.kind === 'node'
  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    try {
      const body = local ? { name } : { name, target, port: port ? Number(port) : undefined, identityFile: key || undefined }
      if (edit) await api.updateHost(edit.id, body)
      else await api.addHost(body as any)
      toast({ title: edit ? 'Machine updated' : 'Machine added', body: local ? undefined : 'Connecting over SSH…', tone: 'success' })
      onClose()
    } catch (err) {
      toast({ title: 'Could not save', body: (err as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={edit ? `Edit ${edit.name}` : 'Add a machine'}
      footer={
        !edit && mode === 'pair' ? undefined : <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" form="machine-form" loading={busy} disabled={!name || (!local && !target)}>
            {edit ? 'Save' : 'Add machine'}
          </Button>
        </div>
      }
    >
      {!edit && (
        <div className="mb-4">
          <Segmented full value={mode} onChange={setMode} options={[{ v: 'pair', label: 'Run a command on it' }, { v: 'ssh', label: 'Reach it over SSH' }]} />
        </div>
      )}
      {!edit && mode === 'pair' ? <PairMachine key={String(open)} onClose={onClose} /> : (
      <form id="machine-form" onSubmit={submit} className="space-y-4">
        {!edit && (
          <p className="text-[13px] leading-relaxed text-fg-2">
            The hub signs in with its own SSH keys and config. Nothing is installed on the machine. Use this when the hub can already reach it: same network, a VPN, or a jump host set up in the hub's <code className="font-mono text-[12px]">~/.ssh/config</code>.
          </p>
        )}
        <Field label="Name">
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder="Work server" autoFocus />
        </Field>
        {!local && (
          <>
            <Field label="SSH destination" hint="user@host, or a Host alias from ~/.ssh/config on the hub machine. The hub uses its own SSH keys, agent and config.">
              <input className={clsx(inputClass, 'font-mono text-[13.5px]')} value={target} onChange={(e) => setTarget(e.target.value)} placeholder="deploy@work-server" autoCapitalize="off" autoCorrect="off" spellCheck={false} />
            </Field>
            <div className="grid grid-cols-[100px_1fr] gap-3">
              <Field label="Port">
                <input className={inputClass} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} placeholder="22" inputMode="numeric" />
              </Field>
              <Field label="Key file (optional)">
                <input className={clsx(inputClass, 'font-mono text-[13.5px]')} value={key} onChange={(e) => setKey(e.target.value)} placeholder="~/.ssh/id_ed25519" autoCapitalize="off" spellCheck={false} />
              </Field>
            </div>
          </>
        )}
      </form>
      )}
    </Sheet>
  )
}

type Pick = AgentKind | 'shell'

export function NewSessionSheet({ open, onClose, hostId }: { open: boolean; onClose: () => void; hostId?: string }) {
  const allHosts = useStore((s) => s.hosts)
  const hosts = allHosts.filter((h) => h.status === 'online')
  const nav = useNavigate()
  const [host, setHost] = useState('')
  const [kind, setKind] = useState<Pick>('claude')
  // one of your agents (Settings › Agents): `kind` is then the agent it works like
  const [custom, setCustom] = useState('')
  const [customs, setCustoms] = useState<CustomAgent[]>([])
  const [cwd, setCwd] = useState('~')
  const [session, setSession] = useState('')
  const [name, setName] = useState('')
  const [model, setModel] = useState('')
  const [mode, setMode] = useState('')
  const [prompt, setPrompt] = useState('')
  const [busy, setBusy] = useState(false)

  const h = hosts.find((x) => x.config.id === host)
  const installed = h?.agents ?? []
  const recent = useMemo(
    () => [...new Set(allPanes(allHosts).filter((r) => r.host.config.id === host).map((r) => r.pane.cwd.replace(/^\/home\/[^/]+|^\/Users\/[^/]+|^\/root/, '~')))].slice(0, 6),
    [allHosts, host],
  )

  useEffect(() => {
    if (!open) return
    const first = hostId ?? hosts[0]?.config.id ?? ''
    setHost(first)
    setSession('')
    setPrompt('')
    setModel('')
    const inst = hosts.find((x) => x.config.id === first)?.agents ?? []
    setKind(inst.includes('claude') ? 'claude' : inst[0] ?? 'shell')
    setCustom('')
    api.settings().then((s) => setCustoms(s.customAgents ?? []), () => setCustoms([]))
  }, [open])

  // opened before the machines arrived (app starting, phone reconnecting): pick one once they do
  const firstHost = hostId ?? hosts[0]?.config.id
  useEffect(() => {
    if (!open || host || !firstHost) return
    setHost(firstHost)
    const inst = hosts.find((x) => x.config.id === firstHost)?.agents ?? []
    setKind(inst.includes('claude') ? 'claude' : inst[0] ?? 'shell')
  }, [open, host, firstHost])

  const def = kind !== 'shell' ? agentDef(kind) : undefined
  const mine = customs.find((c) => c.id === custom)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    try {
      const { pane } = await api.launch(host, {
        agent: kind,
        custom: mine?.id,
        cwd,
        session: session || undefined,
        name: name || undefined,
        model: model || undefined,
        mode: (kind === 'claude' || kind === 'antigravity') && mode ? mode : undefined,
        prompt: (def?.chat && prompt) || undefined,
      })
      onClose()
      nav(paneHref(host, pane, kind === 'shell' ? 'terminal' : undefined))
    } catch (err) {
      toast({ title: 'Could not start', body: (err as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }

  const choices: Pick[] = [...(Object.keys(AGENTS) as AgentKind[]).filter((k) => installed.includes(k)), 'shell']

  return (
    <Sheet
      open={open}
      onClose={onClose}
      wide
      title="New session"
      footer={
        <div className="flex items-center gap-2">
          <span className="hidden truncate text-[12px] text-fg-3 sm:block">Opens in a new tmux {session ? 'window' : 'session'}</span>
          <Button className="ml-auto" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" form="new-form" loading={busy} disabled={!host}>
            Start {mine?.name ?? def?.name ?? 'terminal'}
          </Button>
        </div>
      }
    >
      <form id="new-form" onSubmit={submit} className="space-y-5">
        <div>
          <div className="mb-2 text-[12.5px] font-medium text-fg-2">Agent</div>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
            {choices.map((k) => (
              <button
                key={k}
                type="button"
                onClick={() => (setKind(k), setCustom(''))}
                className={clsx('flex items-center gap-3 rounded-2xl border px-3 py-3 text-left transition-colors', k === 'shell' && 'order-2', kind === k && !mine ? 'border-fg bg-hover' : 'border-line hover:bg-hover')}
              >
                {k === 'shell' ? <AgentLogo size={26} /> : <AgentLogo kind={k} size={26} />}
                <span className="min-w-0">
                  <span className="block truncate text-[13.5px] font-semibold">{k === 'shell' ? 'Terminal' : agentDef(k).name}</span>
                  <span className="block truncate text-[12px] text-fg-3">{k === 'shell' ? 'Plain shell' : agentDef(k).vendor}</span>
                </span>
              </button>
            ))}
            {customs.map((c) => (
              <button
                key={`custom-${c.id}`}
                type="button"
                title={c.command}
                onClick={() => (setKind(c.like), setCustom(c.id))}
                className={clsx('order-1 flex items-center gap-3 rounded-2xl border px-3 py-3 text-left transition-colors', custom === c.id ? 'border-fg bg-hover' : 'border-line hover:bg-hover')}
              >
                <AgentLogo kind={c.like} size={26} />
                <span className="min-w-0">
                  <span className="block truncate text-[13.5px] font-semibold">{c.name}</span>
                  <span className="block truncate text-[12px] text-fg-3">Like {agentDef(c.like).name}</span>
                </span>
              </button>
            ))}
          </div>
          {h && !installed.length && <p className="mt-2 text-[12px] text-fg-3">No agent CLIs found on {h.config.name}'s PATH.</p>}
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label={isPhone() ? 'Computer' : 'Machine'}>
            <Select title={isPhone() ? 'Computer' : 'Machine'} value={host} onChange={(v) => (setHost(v), setSession(''))} options={hosts.map((x) => ({ value: x.config.id, label: x.config.name, hint: x.config.kind === 'link' ? 'Paired' : x.config.target }))} />
          </Field>
          <Field label="Folder">
            <input className={clsx(inputClass, 'font-mono text-[13.5px]')} value={cwd} onChange={(e) => setCwd(e.target.value)} autoCapitalize="none" autoCorrect="off" spellCheck={false} />
          </Field>
        </div>
        {recent.length > 0 && (
          <div className="-mt-2 flex flex-wrap gap-1.5">
            {recent.map((r) => (
              <button key={r} type="button" title={r} onClick={() => setCwd(r)} className={clsx('max-w-full truncate rounded-full border px-2 py-0.5 font-mono text-[12px] pointer-coarse:px-2.5 pointer-coarse:py-1.5', cwd === r ? 'border-fg text-fg' : 'border-line text-fg-3 hover:text-fg-2')}>
                {tailPath(r)}
              </button>
            ))}
          </div>
        )}

        {def && (def.models.length > 0 || kind === 'claude' || kind === 'antigravity') && (
          <div className="grid gap-4 sm:grid-cols-2">
            {def.models.length > 0 && (
              <Field label="Model">
                <Select title="Model" value={model} onChange={setModel} options={[{ value: '', label: 'Default' }, ...def.models.map((m) => ({ value: m.id, label: m.label }))]} />
              </Field>
            )}
            {(kind === 'claude' || kind === 'antigravity') && (
              <Field label="Permissions">
                {/* Default passes no flag: the agent's own setting decides */}
                <Select title="Permissions" value={mode} onChange={setMode} options={[{ value: '', label: 'Default' }, ...def.modes.map((m) => ({ value: m.id, label: m.label }))]} />
              </Field>
            )}
          </div>
        )}

        {/* a terminal-first agent starts without one; you type it in its terminal */}
        {def?.chat && (
          <Field label="First message (optional)">
            <textarea rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="What should it work on?" className={clsx(inputClass, 'h-auto resize-none py-2.5 leading-relaxed')} />
          </Field>
        )}

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="tmux">
            <Select title="tmux" value={session} onChange={setSession} options={[{ value: '', label: 'New session' }, ...(h?.sessions ?? []).map((s) => ({ value: s.name, label: `New window in “${s.name}”` }))]} />
          </Field>
          {!session && (
            <Field label="Session name">
              <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder={mine?.name ?? (kind === 'shell' ? 'shell' : kind)} autoCapitalize="none" autoCorrect="off" spellCheck={false} />
            </Field>
          )}
        </div>
      </form>
    </Sheet>
  )
}
