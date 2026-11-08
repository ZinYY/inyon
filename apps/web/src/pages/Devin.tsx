import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { ArrowLeft, Send, SquareTerminal } from 'lucide-react'
import { api } from '../lib/api.ts'
import { DEVIN_BUSY, mergeDevin, type DevinMessage, type DevinSession, type DevinUpdate } from '../lib/devin.ts'
import { paneHref } from '../lib/overview.ts'
import { toast, useStore } from '../lib/store.ts'
import { Button, Empty, Waiting } from '../components/ui/primitives.tsx'
import { CopyButton, Markdown } from '../components/chat/Markdown.tsx'
import { messagePreview } from '../components/chat/Messages.tsx'

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
  const [expanded, setExpanded] = useState(false)
  const preview = useMemo(() => messagePreview(message.text), [message.text])
  const long = preview.length < message.text.length
  const text = expanded ? message.text : preview
  return <article className={`min-w-0 space-y-2 [overflow-wrap:anywhere] [content-visibility:auto] [contain-intrinsic-size:auto_160px] ${message.role === 'user' ? 'ml-auto max-w-[90%] rounded-2xl bg-bubble px-4 py-3' : ''}`}>
    <div className="flex items-center gap-2 text-xs text-fg-3"><span>{message.role === 'user' ? 'You' : message.role === 'tool' ? 'Tool result' : 'Devin'}</span><CopyButton text={message.text} /></div>
    {message.role === 'tool' ? <details><summary className="cursor-pointer text-sm text-fg-2">Tool output</summary><pre className="max-h-80 overflow-auto whitespace-pre-wrap text-xs">{text}</pre></details>
      : message.role === 'user' ? <div className="whitespace-pre-wrap">{text}{long && !expanded && '…'}</div>
      : <Markdown text={text + (long && !expanded ? '\n\n…' : '')} sans />}
    {long && <button className="text-sm text-accent-text" onClick={() => setExpanded(!expanded)}>{expanded ? 'Show less' : 'Show more'}</button>}
    {message.thinking && <details><summary className="cursor-pointer text-sm text-fg-2">Thinking</summary><pre className="max-h-64 overflow-auto whitespace-pre-wrap text-xs">{message.thinking.slice(0, 12000)}</pre></details>}
    {message.tools.map((tool, i) => <details key={i} className="rounded-lg border border-line p-2 text-sm"><summary className="cursor-pointer">{tool.name}</summary><pre className="max-h-64 overflow-auto whitespace-pre-wrap text-xs">{JSON.stringify(tool.input, null, 2).slice(0, 12000)}</pre></details>)}
  </article>
})

function DevinChat({ host, sid }: { host: string; sid: string }) {
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
  return <div className="flex min-h-0 flex-1 flex-col">
    <header className="pt-safe flex shrink-0 items-center gap-3 border-b border-line px-4 py-3">
      <Link aria-label="Devin conversations" to={`/m/${encodeURIComponent(host)}/devin`}><ArrowLeft size={18} /></Link>
      <div className="min-w-0 flex-1"><h1 className="truncate font-semibold">{update?.session?.title || sid}</h1><p className="truncate text-xs text-fg-3">{update?.session?.model} · {sid}</p></div>
      {state?.pane && <Link className="inline-flex items-center gap-1 text-sm" to={`${paneHref(host, state.pane)}?view=terminal`}><SquareTerminal size={16} /> Terminal</Link>}
    </header>
    <div className="shrink-0 border-b border-line px-4 py-2 text-xs text-fg-2" role="status">
      {state?.source === 'cli' ? `CLI in tmux: ${state.tmux}` : 'Reading Devin App history'}
      {' · '}{sending ? 'Sending…' : state?.status === 'idle' ? 'Ready' : 'Waiting for Devin to finish'}
    </div>
    {error && <p role="alert" className="shrink-0 px-4 py-2 text-sm text-danger">{error}</p>}
    <div ref={scroll} className="min-h-0 flex-1 overflow-auto overscroll-contain" onScroll={() => { const e = scroll.current; if (e) follows.current = e.scrollHeight - e.scrollTop - e.clientHeight < 100 }}>
      <div className="mx-auto max-w-[800px] space-y-6 px-4 py-5 sm:px-6">
        {(!update || update.loading) && !error && <Waiting shape="lines" label="Reading saved history" />}
        {(update?.order.length ?? 0) > limit && <Button size="sm" onClick={() => { follows.current = false; setLimit(limit + 80) }}>Load earlier messages</Button>}
        {rows.map((m) => <SavedMessage key={m.id} message={m} />)}
        {state?.live && <section className="rounded-xl border border-line bg-elev p-3"><p className="mb-2 text-xs font-medium text-fg-2">Live terminal preview</p><pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap break-words text-xs">{state.live}</pre></section>}
      </div>
    </div>
    <form className="shrink-0 border-t border-line p-3 pb-safe" onSubmit={(e) => { e.preventDefault(); void send() }}>
      <div className="mx-auto flex max-w-[800px] items-end gap-2">
        <textarea aria-label="Message Devin" placeholder="Message Devin…" className="max-h-48 min-h-24 min-w-0 flex-1 resize-y rounded-xl border border-line bg-elev p-3" value={text} onChange={(e) => setText(e.target.value)} disabled={sending} onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); void send() } }} />
        <Button type="submit" variant="primary" loading={sending} disabled={!text.trim()}><Send size={16} /> Send</Button>
      </div>
    </form>
  </div>
}
