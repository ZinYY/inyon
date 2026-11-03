import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { Check, Copy, Download, ExternalLink, LogIn, Plug, RefreshCw, TriangleAlert } from 'lucide-react'
import { agentDef, type AgentKind, type HostState } from '@inyon/protocol'
import { api, type SetupReport } from '../lib/api.ts'
import { toast } from '../lib/store.ts'
import { paneHref } from '../lib/overview.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Toggle, Waiting } from './ui/primitives.tsx'
import { SignInSheet } from './SignInSheet.tsx'
import { ProvidersSheet } from './ProvidersSheet.tsx'
import { copyText } from '../lib/clipboard.ts'

function CopyLine({ cmd }: { cmd: string }) {
  return (
    <div className="mt-2 flex items-center gap-2 rounded-xl border border-line bg-code px-3 py-2">
      <code className="min-w-0 flex-1 truncate font-mono text-[12px]">{cmd}</code>
      <button
        type="button"
        aria-label="Copy command"
        onClick={() => copyText(cmd).then((ok) => toast(ok ? { title: 'Command copied', tone: 'success' } : { title: 'Could not copy', body: 'Select the command and copy it by hand.', tone: 'error' }))}
        className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg"
      >
        <Copy className="size-3.5" />
      </button>
    </div>
  )
}

/** "codex-cli 0.49.0" and "2.1.296 (Claude Code)" as just the number */
const short = (v?: string) => v?.match(/\d+(?:\.\d+)+/)?.[0] ?? v ?? ''

/** A step that failed, shown under the row it belongs to. */
interface Failure {
  action: string
  hint?: string
  output?: string
  manual?: string
  docs?: string
}

function FailureBlock({ f, className }: { f: Failure; className?: string }) {
  const tail = f.output?.split('\n').slice(-12).join('\n').trim()
  return (
    <div className={clsx('animate-slide-up mt-2 space-y-2 text-[12.5px]', className)}>
      <p className="text-danger">{f.hint ?? "It didn't finish."}</p>
      {tail && <pre className="max-h-48 overflow-auto rounded-2xl border border-line bg-code px-3 py-2.5 font-mono text-[12px] leading-relaxed text-fg-2">{tail}</pre>}
      {f.manual && (
        <div>
          <p className="text-fg-3">Or run it on the machine yourself:</p>
          <CopyLine cmd={f.manual} />
        </div>
      )}
      {f.docs && (
        <a href={f.docs} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-fg-2 hover:text-fg">
          Install guide <ExternalLink className="size-3" />
        </a>
      )}
    </div>
  )
}

/** Everything a machine needs for Inyon, with one-tap fixes where it's safe to run them. */
export function SetupPanel({ host }: { host: HostState }) {
  const [report, setReport] = useState<SetupReport | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [failure, setFailure] = useState<Failure | null>(null)
  const [signIn, setSignIn] = useState<AgentKind | null>(null)
  const [providers, setProviders] = useState(false)
  const nav = useNavigate()
  const id = host.config.id

  // an agent that stays in its terminal signs in there too: its own sign-in, in a new session
  const signInTerminal = async (kind: AgentKind) => {
    try {
      const { pane } = await api.launch(id, { agent: kind, signIn: true, name: `${kind}-sign-in` })
      nav(paneHref(id, pane, 'terminal'))
    } catch (e) {
      toast({ title: 'Could not start the sign-in', body: (e as Error).message, tone: 'error' })
    }
  }

  const load = () => api.setup(id).then(setReport).catch((e) => toast({ title: 'Could not check the machine', body: e.message, tone: 'error' }))
  useEffect(() => {
    if (host.status === 'online') load()
  }, [id, host.status])

  /** Runs a step; `done` is the toast title when it worked, given what the hub reported. */
  const run = async (action: string, done: (version?: string) => string, more: Partial<Failure> = {}) => {
    setBusy(action)
    setFailure(null)
    try {
      const r = await api.runSetup(id, action)
      if (r.pane) return nav(paneHref(id, r.pane, 'terminal'))
      if (r.ok) toast({ title: done(r.version), tone: 'success' })
      else setFailure({ action, hint: r.hint, output: r.output, manual: r.manual ?? more.manual, docs: more.docs })
      await load()
    } catch (e) {
      setFailure({ action, hint: (e as Error).message, manual: more.manual, docs: more.docs })
    } finally {
      setBusy(null)
    }
  }

  if (host.status !== 'online') return <p className="text-[13px] text-fg-3">Setup checks run once the machine is connected.</p>
  if (!report) return <Waiting shape="rows" label={`Checking ${host.config.name}`} className="max-w-3xl" />

  return (
    <div className="max-w-3xl space-y-6">
      <div className="flex items-center gap-2 text-[12.5px] text-fg-3">
        {report.os}
        {report.pkg && ` · ${report.pkg}`}
        {report.sudo ? ' · sudo available' : ' · no passwordless sudo'}
        <button type="button" onClick={load} className="ml-auto flex items-center gap-1 rounded-full px-2 py-1 hover:bg-hover hover:text-fg">
          <RefreshCw className="size-3.5" /> Re-check
        </button>
      </div>

      <section className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
        {report.checks.map((c) => (
          <div key={c.id} className="px-4 py-3.5">
            <div className="flex items-center gap-3">
              <span className={clsx('flex size-5 shrink-0 items-center justify-center rounded-full', c.ok ? 'bg-sunken text-fg' : 'bg-warn-soft text-warn')}>
                {c.ok ? <Check className="size-3" strokeWidth={3} /> : <TriangleAlert className="size-3" />}
              </span>
              <div className="min-w-0 flex-1">
                <div className="text-[14px] font-medium">{c.label}</div>
                {c.detail && <div className="truncate text-[12.5px] text-fg-3">{c.detail}</div>}
              </div>
              {c.fix && (
                <Button size="sm" variant="primary" loading={busy === c.fix.action} onClick={() => run(c.fix!.action, () => `${c.fix!.label}: done`, { manual: c.manual })}>
                  {c.fix.label}
                </Button>
              )}
            </div>
            {failure && c.fix && failure.action === c.fix.action && <FailureBlock f={failure} className="pl-8" />}
            {!c.ok && !c.fix && c.manual && (
              <div className="pl-8">
                <p className="mt-1 text-[12px] text-fg-3">Run this on the machine (it needs your password):</p>
                <CopyLine cmd={c.manual} />
              </div>
            )}
          </div>
        ))}
        <div className="flex items-center gap-3 px-4 py-3.5">
          <div className="min-w-0 flex-1 pl-8">
            <div className="text-[14px] font-medium">Keep a tmux session running</div>
            <div className="text-[12.5px] text-fg-3">If every session closes, Inyon starts a fresh one called “main”.</div>
          </div>
          <Toggle on={report.keepSession} onChange={async (v) => (await api.keepSession(id, v), load())} label="Keep a tmux session running" />
        </div>
      </section>

      <section>
        <h3 className="mb-2.5 text-[13px] font-semibold">Agents</h3>
        <div className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
          {report.agents.map((a) => {
            const def = agentDef(a.kind)
            const install = `agent:${a.kind}`
            const update = `update:${a.kind}`
            const working = busy === install ? 'Installing…' : busy === update ? 'Updating…' : null
            // OpenCode signs in per provider; its free models need none
            const account = a.kind === 'opencode' ? (a.providers?.length ? a.providers.join(', ') : 'free models only') : a.signedIn ? 'signed in' : 'not signed in'
            const state = !a.installed ? 'Not installed' : [short(a.version), a.latest && !a.outdated && 'up to date', account].filter(Boolean).join(' · ')
            const more = { docs: def.install.docs }
            return (
              <div key={a.kind} className="px-4 py-3">
                <div className="flex flex-wrap items-center gap-3">
                  <AgentLogo kind={a.kind} size={28} />
                  <div className="min-w-0 flex-[1_1_180px]">
                    <div className="text-[14px] font-medium">{def.name}</div>
                    <div className="truncate text-[12px] text-fg-3">{working ?? state}</div>
                  </div>
                  <div className="flex shrink-0 gap-1.5">
                    {!a.installed && (
                      <Button size="sm" loading={busy === install} onClick={() => run(install, (v) => `${def.name} ${short(v)} installed`, more)}>
                        <Download className="size-3.5" /> Install
                      </Button>
                    )}
                    {a.installed && a.outdated && (
                      <Button size="sm" loading={busy === update} onClick={() => run(update, (v) => `${def.name} updated to ${short(v)}`, more)}>
                        <RefreshCw className="size-3.5" /> Update to {short(a.latest)}
                      </Button>
                    )}
                    {/* no registry answer for this one: an update still runs its own updater */}
                    {a.installed && !a.latest && (
                      <Button size="sm" variant="ghost" loading={busy === update} onClick={() => run(update, (v) => `${def.name} updated to ${short(v)}`, more)}>
                        <RefreshCw className="size-3.5" /> Update
                      </Button>
                    )}
                    {a.installed && a.kind === 'opencode' && (
                      <Button size="sm" variant={a.providers?.length ? 'secondary' : 'primary'} onClick={() => setProviders(true)}>
                        <Plug className="size-3.5" /> Providers
                      </Button>
                    )}
                    {a.installed && a.kind !== 'opencode' && !a.signedIn && (
                      <Button size="sm" variant="primary" onClick={() => (def.chat ? setSignIn(a.kind) : signInTerminal(a.kind))}>
                        <LogIn className="size-3.5" /> Sign in
                      </Button>
                    )}
                    <a href={def.install.docs} target="_blank" rel="noreferrer" className="inline-flex size-8 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg" aria-label={`${def.name} docs`}>
                      <ExternalLink className="size-3.5" />
                    </a>
                  </div>
                </div>
                {failure && (failure.action === install || failure.action === update) && <FailureBlock f={failure} className="pl-10" />}
              </div>
            )
          })}
        </div>
        <p className="mt-2 text-[12px] text-fg-3">Install uses each agent's official installer; Update runs its own updater, or npm when it came from npm. Sign in runs the agent's sign-in on that machine and walks you through it here; for an agent that stays in its terminal, it opens the sign-in in a terminal session. Providers connects and disconnects OpenCode's model providers the same way.</p>
      </section>

      <SignInSheet hostId={id} hostName={host.config.name} agent={signIn} onClose={() => (setSignIn(null), load())} />
      <ProvidersSheet hostId={id} hostName={host.config.name} open={providers} onClose={() => (setProviders(false), load())} />
    </div>
  )
}
