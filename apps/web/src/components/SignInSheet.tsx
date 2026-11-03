import { useEffect, useRef, useState } from 'react'
import clsx from 'clsx'
import { Check, ChevronRight, Copy, ExternalLink, TriangleAlert } from 'lucide-react'
import { agentDef, type AgentKind } from '@inyon/protocol'
import { api, type SignInScreen } from '../lib/api.ts'
import { toast } from '../lib/store.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Pulse, Sheet, Spinner, Waiting, inputClass } from './ui/primitives.tsx'
import { copyText } from '../lib/clipboard.ts'

const copy = (text: string, what: string) => copyText(text).then((ok) => ok && toast({ title: `${what} copied`, tone: 'success' }))

/** An OpenCode model provider to connect, instead of the agent's whole sign-in. */
export interface ProviderToConnect {
  id: string
  name: string
}

/**
 * Signs an agent in on a machine from here: the hub runs the agent's own sign-in out of sight, and
 * this shows its link and code, takes the code the sign-in page hands back, and shows the output.
 * With `provider`, it connects that OpenCode provider the same way (`opencode auth login -p`).
 */
export function SignInSheet({ hostId, hostName, agent, provider, onClose }: { hostId: string; hostName?: string; agent: AgentKind | null; provider?: ProviderToConnect; onClose: () => void }) {
  const open = !!agent
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={agent ? (provider ? `Connect ${provider.name}` : `Sign in to ${agentDef(agent).name}`) : ''}
      icon={agent ? <AgentLogo kind={agent} size={24} /> : undefined}
      wide
    >
      {agent && <SignInFlow key={`${agent}:${provider?.id ?? ''}`} hostId={hostId} hostName={hostName} agent={agent} provider={provider} onClose={onClose} />}
    </Sheet>
  )
}

function SignInFlow({ hostId, hostName, agent, provider, onClose }: { hostId: string; hostName?: string; agent: AgentKind; provider?: ProviderToConnect; onClose: () => void }) {
  const [pane, setPane] = useState<string | null>(null)
  const [screen, setScreen] = useState<SignInScreen | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [answer, setAnswer] = useState('')
  const [run, setRun] = useState(0)
  /** the code went in; waiting for the agent to take it */
  const [sent, setSent] = useState(false)
  const [opened, setOpened] = useState(false)
  const out = useRef<HTMLPreElement>(null)
  const openLink = useRef<HTMLAnchorElement>(null)
  const codeInput = useRef<HTMLInputElement>(null)
  const name = provider?.name ?? agentDef(agent).name
  const done = screen?.exit !== undefined

  // start the sign-in, and end it if the sheet closes first
  useEffect(() => {
    let alive = true
    let started: string | null = null
    setPane(null)
    setScreen(null)
    setError(null)
    ;(provider ? api.connectProvider(hostId, provider.id) : api.signIn(hostId, agent))
      .then((r) => {
        started = r.pane
        if (alive) setPane(r.pane)
        else api.signInStop(hostId, r.pane).catch(() => {})
      })
      .catch((e) => alive && setError(e.message))
    return () => {
      alive = false
      if (started) api.signInStop(hostId, started).catch(() => {})
    }
  }, [hostId, agent, run])

  useEffect(() => {
    if (!pane || done) return
    let alive = true
    const tick = () =>
      api
        .signInScreen(hostId, pane)
        .then((s) => alive && setScreen(s))
        .catch((e) => alive && setError(e.message))
    tick()
    const t = setInterval(tick, 1000)
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [hostId, pane, done])

  useEffect(() => {
    out.current?.scrollTo({ top: out.current.scrollHeight })
  }, [screen?.lines.length])

  useEffect(() => {
    if (screen?.exit === 0) toast({ title: provider ? `${name} is connected` : `${name} is signed in`, body: hostName, tone: 'success' })
  }, [screen?.exit])

  const send = async (b: { text?: string; keys?: string[]; option?: number }) => {
    if (!pane) return
    try {
      await api.signInSend(hostId, pane, b)
      if (b.text) {
        setAnswer('')
        setSent(true)
      }
    } catch (e) {
      toast({ title: 'Could not send', body: (e as Error).message, tone: 'error' })
    }
  }

  // a code that didn't take: the agent asks again, so let it be pasted again
  useEffect(() => {
    if (!sent) return
    const t = setTimeout(() => setSent(false), 20000)
    return () => clearTimeout(t)
  }, [sent])

  // keyboard first: the link button when it appears, then the code box once you come back from the page
  const hasLink = !!screen?.links.length && !screen?.menu
  useEffect(() => {
    if (hasLink && !opened) openLink.current?.focus()
  }, [hasLink])
  useEffect(() => {
    if (!opened) return
    const back = () => codeInput.current?.focus()
    window.addEventListener('focus', back)
    return () => window.removeEventListener('focus', back)
  }, [opened])

  if (error) {
    return (
      <div className="space-y-4">
        <Notice tone="error">{error}</Notice>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>Close</Button>
          <Button variant="primary" onClick={() => setRun((n) => n + 1)}>Try again</Button>
        </div>
      </div>
    )
  }
  // the sign-in shows as steps or a menu: a couple of rows
  if (!screen) return <Waiting shape="rows" count={2} label={`Starting ${name}'s sign-in${hostName ? ` on ${hostName}` : ''}`} />

  const [link, ...more] = screen.links
  const menu = !done ? screen.menu : undefined
  const askCode = !done && !menu && !screen.code && (!!link || !!screen.input)
  return (
    <div className="space-y-4">
      {done ? (
        screen.exit === 0 ? (
          <Notice tone="ok">{name} is {provider ? 'connected' : 'signed in'}{hostName ? ` on ${hostName}` : ''}.</Notice>
        ) : (
          <Notice tone="error">{screen.error ?? `The ${provider ? 'connection' : 'sign-in'} ended without ${provider ? 'connecting' : 'signing in'}. The details below say why.`}</Notice>
        )
      ) : menu ? null : sent ? (
        <div className="flex items-center gap-2.5 text-[13.5px] text-fg-2">
          <Pulse /> Checking the code with {name}…
        </div>
      ) : screen.input && !link ? null : (
        // a box asking for a key says enough on its own
        <Steps code={!!screen.code} link={!!link} name={name} />
      )}

      {menu && <MenuStep key={menu.options.map((o) => o.label).join('|')} menu={menu} onPick={(n) => send({ option: n })} />}
      {agent === 'opencode' && menu?.question === 'Select provider' && (
        <div className="flex gap-2">
          <Button size="sm" onClick={() => send({ keys: Array(7).fill('Up') })}>Previous providers</Button>
          <Button size="sm" onClick={() => send({ keys: Array(7).fill('Down') })}>More providers</Button>
        </div>
      )}

      {!done && !menu && screen.code && (
        <div className="flex items-center justify-between gap-3 rounded-2xl border border-line bg-sunken px-4 py-3">
          <div>
            <div className="text-[12.5px] font-medium text-fg-3">Your code</div>
            <div className="tabular font-mono text-[22px] font-semibold tracking-[0.12em]">{screen.code}</div>
          </div>
          <Button size="sm" onClick={() => copy(screen.code!, 'Code')}>
            <Copy className="size-3.5" /> Copy
          </Button>
        </div>
      )}

      {!done && !menu && link && !sent && (
        <div className="flex gap-2">
          <a
            ref={openLink}
            href={link}
            target="_blank"
            rel="noreferrer"
            onClick={() => setOpened(true)}
            className="inline-flex h-11 flex-1 items-center justify-center gap-2 rounded-xl bg-fg px-4 text-[14.5px] font-medium text-bg outline-offset-2 hover:opacity-90 focus-visible:outline-2 focus-visible:outline-accent"
          >
            <ExternalLink className="size-4" /> Open the sign-in page
          </a>
          <Button size="lg" aria-label="Copy link" title="Copy link" onClick={() => copy(link, 'Link')}>
            <Copy className="size-4" />
          </Button>
        </div>
      )}
      {!done && !menu && more.length > 0 && (
        <div className="space-y-1 text-[12px]">
          {more.map((l) => (
            <a key={l} href={l} target="_blank" rel="noreferrer" className="block truncate text-accent-text hover:underline">{l}</a>
          ))}
        </div>
      )}

      {askCode && !sent && (
        <form
          onSubmit={(e) => {
            e.preventDefault()
            if (answer.trim()) send({ text: answer })
          }}
          className="space-y-1.5"
        >
          <label htmlFor="signin-code" className="text-[12.5px] font-medium text-fg-2">{screen.input?.label ?? 'Code from the sign-in page'}</label>
          <div className="flex gap-2">
            <input
              id="signin-code"
              type={screen.input?.secret ? 'password' : 'text'}
              ref={codeInput}
              className={clsx(inputClass, 'font-mono text-[13.5px] placeholder:font-sans')}
              value={answer}
              onChange={(e) => setAnswer(e.target.value)}
              placeholder={screen.input?.label ?? 'Paste the code'}
              autoCapitalize="off"
              autoCorrect="off"
              autoComplete="off"
              spellCheck={false}
            />
            <Button type="submit" variant="primary" disabled={!answer.trim()} className="h-10">{provider ? 'Connect' : 'Sign in'}</Button>
          </div>
        </form>
      )}

      <details className="group">
        <summary className="flex cursor-pointer list-none items-center gap-1.5 text-[12px] text-fg-3 select-none hover:text-fg-2">
          <ChevronRight className="size-3.5 transition-transform group-open:rotate-90" />
          Details from {hostName ?? 'the machine'}
        </summary>
        <pre ref={out} className="mt-2 max-h-[30dvh] overflow-auto rounded-2xl border border-line bg-code px-3 py-2.5 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-all text-fg-3">
          {screen.lines.join('\n') || ' '}
        </pre>
      </details>

      {done && (
        <div className="flex justify-end gap-2">
          {screen.exit !== 0 && <Button variant="ghost" onClick={() => setRun((n) => n + 1)}>Try again</Button>}
          <Button variant="primary" onClick={onClose} autoFocus>Done</Button>
        </div>
      )}
    </div>
  )
}

/** What to do next, as numbered steps that tick off. */
function Steps({ code, link, name }: { code: boolean; link: boolean; name: string }) {
  const steps = code
    ? ['Open the sign-in page', 'Sign in, and enter the code shown here', `${name} picks it up by itself`]
    : link
      ? ['Open the sign-in page and sign in', 'Copy the code the page shows you', 'Paste it below']
      : [`Waiting for ${name}…`]
  if (!link) return <div className="flex items-center gap-2.5 text-[13.5px] text-fg-2"><Pulse /> {steps[0]}</div>
  return (
    <ol className="space-y-1.5">
      {steps.map((t, i) => (
        <li key={t} className="flex items-center gap-2.5 text-[13.5px] text-fg-2">
          <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-sunken text-[12px] font-semibold text-fg-3">{i + 1}</span>
          {t}
        </li>
      ))}
    </ol>
  )
}

/**
 * A menu from the sign-in (e.g. "Select login method") as buttons: click one, or use ↑/↓ and
 * Enter, or press its number.
 */
function MenuStep({ menu, onPick }: { menu: NonNullable<SignInScreen['menu']>; onPick: (n: number) => Promise<void> }) {
  const [picked, setPicked] = useState<number | null>(null)
  const list = useRef<HTMLDivElement>(null)
  const buttons = () => [...(list.current?.querySelectorAll<HTMLButtonElement>('button') ?? [])]
  useEffect(() => {
    const i = Math.max(0, menu.options.findIndex((o) => o.selected))
    buttons()[i]?.focus()
  }, [])
  const pick = (n: number) => {
    if (picked !== null) return
    setPicked(n)
    onPick(n).finally(() => setTimeout(() => setPicked(null), 4000))
  }
  return (
    <div className="space-y-2.5">
      {menu.question && <div className="text-[14px] font-medium">{menu.question.replace(/:$/, '')}</div>}
      <div
        ref={list}
        role="listbox"
        aria-label={menu.question ?? 'Choose one'}
        className="space-y-1.5"
        onKeyDown={(e) => {
          const bs = buttons()
          const at = bs.indexOf(document.activeElement as HTMLButtonElement)
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault()
            bs[(at + (e.key === 'ArrowDown' ? 1 : -1) + bs.length) % bs.length]?.focus()
          } else if (/^[1-9]$/.test(e.key) && menu.options.some((o) => o.n === Number(e.key))) {
            e.preventDefault()
            pick(Number(e.key))
          }
        }}
      >
        {menu.options.map((o) => (
          <button
            key={o.n}
            type="button"
            role="option"
            aria-selected={o.selected}
            disabled={picked !== null}
            onClick={() => pick(o.n)}
            className={clsx(
              'flex w-full items-center gap-3 rounded-2xl border px-3.5 py-3 text-left outline-none transition-colors',
              'focus:border-accent focus:bg-accent-soft hover:bg-hover disabled:opacity-60',
              picked === o.n ? 'border-accent bg-accent-soft' : 'border-line',
            )}
          >
            <span className="flex size-6 shrink-0 items-center justify-center rounded-xl bg-sunken font-mono text-[12px] text-fg-3">{o.n}</span>
            <span className="min-w-0 flex-1">
              <span className="block text-[14px] font-medium">{o.label}</span>
              {o.detail && <span className="block text-[12px] text-fg-3">{o.detail}</span>}
            </span>
            {picked === o.n && <Spinner />}
          </button>
        ))}
      </div>
      <div className="text-[12px] text-fg-3">Click one, or use ↑ ↓ and Enter, or press its number.</div>
    </div>
  )
}

function Notice({ tone, children }: { tone: 'ok' | 'error'; children: React.ReactNode }) {
  return (
    <div className={clsx('flex items-start gap-2 rounded-2xl px-3.5 py-3 text-[13.5px]', tone === 'ok' ? 'bg-sunken text-fg' : 'bg-danger-soft text-danger')}>
      {tone === 'ok' ? <Check className="mt-0.5 size-4 shrink-0" /> : <TriangleAlert className="mt-0.5 size-4 shrink-0" />}
      <span className="min-w-0 break-words">{children}</span>
    </div>
  )
}
