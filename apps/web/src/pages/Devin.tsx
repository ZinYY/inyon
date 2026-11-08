import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Link, Navigate, useNavigate, useParams } from 'react-router-dom'
import { ArrowLeft, ArrowDown, ChevronLeft, History, MessageSquare, SquareTerminal } from 'lucide-react'
import { api } from '../lib/api.ts'
import { DEVIN_BUSY, devinRow, mergeDevin, type DevinMessage, type DevinSession, type DevinUpdate } from '../lib/devin.ts'
import { paneHref } from '../lib/overview.ts'
import { modelName, shortPath } from '../lib/format.ts'
import { AgentLogo, SkinScope, useReplyFont } from '../lib/skin.tsx'
import { toast, useStore } from '../lib/store.ts'
import { Button, Empty, IconButton, Segmented, Waiting } from '../components/ui/primitives.tsx'
import { StateDot, StatePill } from '../components/ui/status.tsx'
import { ReplyFont } from '../components/chat/Markdown.tsx'
import { AssistantTurn, ChatAgent, UserMessage, WorkingIndicator } from '../components/chat/Messages.tsx'
import { ChatHost } from '../components/chat/agent.ts'
import { ComposerDock, ConversationColumn, PlainComposer, SessionHeaderBar, SessionHeading } from '../components/chat/ChatChrome.tsx'

export function DevinPaneChat({ host, pane }: { host: string; pane: string }) {
  const [session, setSession] = useState('')
  const [error, setError] = useState('')
  useEffect(() => {
    let active = true
    setSession(''); setError('')
    api.devinResolve(host, pane).then((r) => { if (active) setSession(r.session) }).catch((e) => { if (active) setError(e.message) })
    return () => { active = false }
  }, [host, pane])
  if (session) return <Navigate replace to={`/m/${encodeURIComponent(host)}/devin/${encodeURIComponent(session)}`} />
  if (error) return <Empty title="Choose a Devin conversation"><p>{error}</p><Link className="text-accent-text" to={`/m/${encodeURIComponent(host)}/devin`}>Devin conversations</Link></Empty>
  return <Waiting shape="lines" label="Opening the Devin conversation" />
}

export function DevinPage() {
  const params = useParams()
  const host = decodeURIComponent(params.host!)
  const sid = params.session ? decodeURIComponent(params.session) : undefined
  return sid ? <DevinChat key={`${host}:${sid}`} host={host} sid={sid} /> : <DevinList key={host} host={host} />
}

function DevinList({ host }: { host: string }) {
  const name = useStore((s) => s.hosts.find((h) => h.config.id === host)?.config.name ?? host)
  const [sessions, setSessions] = useState<DevinSession[] | null>(null)
  const [error, setError] = useState('')
  const [query, setQuery] = useState('')
  useEffect(() => {
    let active = true
    const load = () => api.devinSessions(host).then((r) => { if (active) { setSessions(r.sessions); setError('') } }).catch((e) => { if (active) setError(e.message) })
    void load()
    const timer = setInterval(() => { if (!document.hidden) void load() }, 15_000)
    return () => { active = false; clearInterval(timer) }
  }, [host])
  const shown = sessions?.filter((s) => `${s.title} ${s.cwd} ${s.id}`.toLowerCase().includes(query.toLowerCase()))
  return <div className="min-h-0 flex-1 overflow-auto p-4 sm:p-6">
    <div className="mx-auto max-w-[900px] space-y-4">
      <Link to={`/machines/${encodeURIComponent(host)}`} className="inline-flex items-center gap-2 text-sm text-fg-2"><ArrowLeft size={16} /> {name}</Link>
      <h1 className="text-xl font-semibold">Devin conversations</h1>
      <p className="text-sm text-fg-2">Read saved App and CLI conversations. Sending a message resumes the same conversation in a visible tmux session.</p>
      <input aria-label="Search Devin conversations" className="w-full rounded-xl border border-line bg-elev px-3 py-2" placeholder="Search title, folder or session ID" value={query} onChange={(e) => setQuery(e.target.value)} />
      {error && <p role="alert" className="text-danger">{error}</p>}
      {!sessions && !error && <Waiting shape="rows" label="Reading Devin conversations" />}
      {shown?.length === 0 && <Empty title="No Devin conversations">Saved conversations will appear here.</Empty>}
      <div className="divide-y divide-line overflow-hidden rounded-2xl border border-line">
        {shown?.map((s) => <Link key={s.id} to={`/m/${encodeURIComponent(host)}/devin/${encodeURIComponent(s.id)}`} className="block px-4 py-3 hover:bg-hover">
          <div className="truncate font-medium">{s.title || s.id}</div>
          <div className="truncate text-sm text-fg-2">{s.cwd}</div>
          <div className="mt-1 text-xs text-fg-3">{s.id} · {s.model}</div>
        </Link>)}
      </div>
    </div>
  </div>
}

const SavedMessage = memo(function SavedMessage({ message }: { message: DevinMessage }) {
  const row = useMemo(() => devinRow(message), [message])
  return <div className="[content-visibility:auto] [contain-intrinsic-size:auto_160px]">
    {row.type === 'user' ? <UserMessage row={row} /> : <AssistantTurn row={row} live={false} foldText />}
  </div>
})

function DevinChat({ host, sid }: { host: string; sid: string }) {
  const nav = useNavigate()
  const font = useReplyFont('codex')
  const machine = useStore((s) => s.hosts.find((h) => h.config.id === host))
  const [atBottom, setAtBottom] = useState(true)
  const [update, setUpdate] = useState<DevinUpdate | null>(null)
  const [messages, setMessages] = useState(new Map<string, DevinMessage>())
  const [error, setError] = useState('')
  const [text, setText] = useState('')
  const [sending, setSending] = useState(false)
  const [limit, setLimit] = useState(80)
  const receipt = useRef<{ text: string; id: string } | null>(null)
  const scroll = useRef<HTMLDivElement>(null)
  const follows = useRef(true)
  const version = useRef(0)
  useEffect(() => {
    let active = true
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        if (!document.hidden) {
          const next = await api.devinRead(host, sid, version.current)
          if (!active) return
          if (next.version !== version.current || next.reset) setMessages((previous) => mergeDevin(previous, next))
          version.current = next.version
          setUpdate(next)
          setError(next.error ?? '')
        }
      } catch (e) { if (active) setError((e as Error).message) }
      if (active) timer = setTimeout(poll, document.hidden ? 5000 : 750)
    }
    void poll()
    return () => { active = false; clearTimeout(timer) }
  }, [host, sid])
  useEffect(() => {
    if (follows.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [messages, update?.state?.live])
  const rows = (update?.order ?? []).slice(-limit).flatMap((id) => { const m = messages.get(id); return m ? [m] : [] })
  const state = update?.state
  const send = async () => {
    if (sending || !text.trim()) return
    if (state?.status !== 'idle' || error) {
      toast({ title: 'Devin is not ready', body: error || state?.reason || DEVIN_BUSY, tone: 'error' })
      return
    }
    if (!receipt.current || receipt.current.text !== text) receipt.current = { text, id: crypto.randomUUID() }
    const attempt = receipt.current
    setSending(true)
    try {
      await api.devinSend(host, sid, attempt.text, attempt.id)
      setText('')
      receipt.current = null
      follows.current = true
    } catch (e) {
      toast({ title: 'Message not confirmed', body: (e as Error).message, tone: 'error' })
    } finally { setSending(false) }
  }
  const busy = sending || state?.status === 'busy'
  const shown = busy ? 'working' : state?.status === 'idle' ? 'idle' : 'waiting'
  const source = state?.source === 'cli' ? `CLI in tmux: ${state.tmux}` : 'App history'
  return <SkinScope agent="codex" className="flex min-h-0 flex-1 flex-col bg-bg text-fg">
    <ChatAgent.Provider value="devin"><ChatHost.Provider value={host}><ReplyFont.Provider value={font}>
      <header className="pt-safe z-10 shrink-0 border-b border-line bg-bg">
        <SessionHeaderBar>
          <Link to={`/m/${encodeURIComponent(host)}/devin`} aria-label="Back" className="inline-flex size-8 shrink-0 items-center justify-center rounded-full text-fg-2 hover:bg-hover relative after:absolute after:-inset-1 md:hidden"><ChevronLeft className="size-5" /></Link>
          <AgentLogo kind="devin" size={26} active={busy} className="max-sm:hidden" />
          <SessionHeading title={<><StateDot state={shown} className="sm:hidden" /><span className="truncate" title={sid}>{update?.session?.title || sid}</span></>}
            subtitle={['Devin', machine?.config.name || host, source, shortPath(update?.session?.cwd, 28)].filter(Boolean).join(' · ')} />
          <span className="hidden sm:block"><StatePill state={shown} /></span>
          {state?.pane && <Segmented size="sm" label="View" value="chat" onChange={(view) => { if (view === 'terminal') nav(`${paneHref(host, state.pane!)}?view=terminal`) }} options={[
            { v: 'chat', name: 'Chat', label: <><MessageSquare className="size-3.5" /><span className="hidden lg:inline">Chat</span></> },
            { v: 'terminal', name: 'Terminal', label: <><SquareTerminal className="size-3.5" /><span className="hidden lg:inline">Terminal</span></> },
          ]} />}
          <IconButton label="Devin conversations" onClick={() => nav(`/m/${encodeURIComponent(host)}/devin`)}><History className="size-[17px]" /></IconButton>
        </SessionHeaderBar>
      </header>
      <div className="sr-only" role="status">{source} · {sending ? 'Sending…' : state?.status === 'idle' ? 'Ready' : 'Waiting for Devin to finish'}</div>
      <div ref={scroll} className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain [overflow-anchor:none]" onScroll={() => {
        const e = scroll.current
        if (e) { follows.current = e.scrollHeight - e.scrollTop - e.clientHeight < 100; setAtBottom(follows.current) }
      }}>
        <ConversationColumn>
          {(!update || update.loading) && !error && <Waiting shape="lines" label="Reading saved history" />}
          {(update?.order.length ?? 0) > limit && <Button size="sm" onClick={() => { follows.current = false; setLimit(limit + 80) }}>Load earlier messages</Button>}
          <div className="space-y-5">{rows.map((m) => <SavedMessage key={m.id} message={m} />)}</div>
          {state?.status === 'busy' && <div className="mt-5 space-y-2"><WorkingIndicator activity="Working…" />
            {state.live && <details className="ml-9 text-[13px] text-fg-3"><summary className="cursor-pointer py-1.5">Live terminal preview</summary><pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words border-l border-line-strong pl-3 text-[12px]">{state.live}</pre></details>}
          </div>}
        </ConversationColumn>
      </div>
      <div className="relative flex min-h-0 flex-col">
        {!atBottom && <button type="button" aria-label="Jump to latest" className="absolute -top-12 right-4 z-10 flex size-9 items-center justify-center rounded-full border border-line bg-elev text-fg-2 shadow-float hover:text-fg sm:right-6" onClick={() => { follows.current = true; if (scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight }}><ArrowDown className="size-4" /></button>}
        <ComposerDock>
          {error && <p role="alert" className="mb-2 text-center text-[12px] text-danger">{error}</p>}
          <PlainComposer value={text} onChange={setText} onSend={() => { void send() }} disabled={sending} label="Message Devin"
            controls={<span className="inline-flex h-8 min-w-0 items-center truncate px-2.5 font-mono text-[12px] text-fg-2">{modelName(update?.session?.model)}{sending && ' · Sending…'}</span>} />
        </ComposerDock>
      </div>
    </ReplyFont.Provider></ChatHost.Provider></ChatAgent.Provider>
  </SkinScope>
}
