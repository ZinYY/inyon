import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import { ArrowDown, History, Play, SquareTerminal } from 'lucide-react'
import { agentDef, chatKey, type AgentKind, type HeldPrompt, type HostStatus, type LiveItem } from '@inyon/protocol'
import { lastAsk } from '../../lib/lastAsk.ts'
import { socket } from '../../lib/socket.ts'
import { msgKey, notifyTag, patchChat, PENDING_MS, toast, useStore, type PendingMessage } from '../../lib/store.ts'
import { clearNativeNotification } from '../../lib/native.ts'
import { othersSends, settle as settleSends } from '../../lib/pending.ts'
import { buildRows } from '../../lib/chat.ts'
import { api, isTransient, withRetries } from '../../lib/api.ts'
import { AgentLogo, useReplyFont } from '../../lib/skin.tsx'
import { Button, Empty, openMenu, Pulse, Waiting } from '../ui/primitives.tsx'
import { AssistantTurn, ChatAgent, ChatRewind, CommandRow, GoalRow, LiveTurn, NoticeRow, PendingBubble, QueuedBubble, UserMessage, WorkingIndicator } from './Messages.tsx'
import { ChatHost } from './agent.ts'
import { ReplyFont } from './copy.tsx'
import { PromptCard } from './PromptCard.tsx'
import { ChatJobs } from './Tools.tsx'
import { Composer } from './Composer.tsx'
import { FailedScreen } from './FailedScreen.tsx'
import { HeldBubble } from './HeldBubble.tsx'
import { ScheduledBubble } from './ScheduledBubble.tsx'
import { sendsFor } from '../../lib/organize.ts'
import { backgroundText, failureText } from '../ui/status.tsx'
import { confirmAction } from '../Confirm.tsx'
import { HelpLink } from '../ConnectSheet.tsx'
import { isPhone } from '../../lib/phone.ts'

const NO_AGENTS: AgentKind[] = []
/** away longer than this, coming back jumps to the latest message */
const AWAY_MS = 5 * 60_000
const NO_QUEUE: string[] = []
const NO_HELD: HeldPrompt[] = []
const NO_PENDING: PendingMessage[] = []
const NO_LIVE: LiveItem[] = []

export function ChatView({ host, pane, agent, onTerminal, controls, ultracode }: { host: string; pane: string; agent?: AgentKind; onTerminal: () => void; controls?: React.ReactNode; ultracode?: boolean }) {
  const key = chatKey(host, pane)
  const chat = useStore((s) => s.chats[key])
  const hubUp = useStore((s) => s.connected)
  const machine = useStore((s) => s.hosts.find((h) => h.config.id === host))
  const machineDown = !!machine && machine.status !== 'online'
  const connected = hubUp && !machineDown
  const installed = useStore((s) => s.hosts.find((h) => h.config.id === host)?.agents) ?? NO_AGENTS
  // why the last turn stopped, when it ended on an error (a rate limit, signed out)
  const failure = useStore((s) => {
    const f = s.hosts.find((h) => h.config.id === host)?.sessions.flatMap((x) => x.windows).flatMap((w) => w.panes).find((p) => p.id === pane)?.agent?.failure
    return f && failureText(f)
  })
  const scroller = useRef<HTMLDivElement>(null)
  const list = useRef<HTMLDivElement>(null)
  const [screenOpen, setScreenOpen] = useState(false)
  // follow new output only while the reader is at the bottom; any move up turns it off
  const stick = useRef(true)
  // the first rows on screen and where they were, so content changing above them doesn't move what's
  // read (several: the top one is rebuilt when older messages join its turn)
  const anchor = useRef<{ el: Element; top: number }[]>([])
  const lastTop = useRef(0)
  const touchY = useRef<number | null>(null)
  // when the reader last touched, wheeled or keyed: only then is a move up theirs (content shrinking
  // below also moves the scroll position up, and that mustn't stop following)
  const inputAt = useRef(0)
  const [atBottom, setAtBottom] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const font = useReplyFont(agent)

  useEffect(() => socket.subscribeChat(host, pane), [host, pane])

  const rows = useMemo(() => buildRows(chat?.items ?? []), [chat?.items])
  const screen = chat?.screen
  const chatCapable = agent ? agentDef(agent).chat : true
  // a terminal-first agent's chat info says nothing until the hub looks: its pane says whether it runs
  const paneAgent = machine?.sessions.flatMap((s) => s.windows).flatMap((w) => w.panes).find((p) => p.id === pane)?.agent
  const running = chatCapable ? (chat?.info?.running ?? true) : !machine || !!paneAgent
  const working = !!screen?.working
  const prompt = screen?.prompt
  // a question asked before the connection dropped (opened from its alert, say) keeps its buttons,
  // held until the machine is back and its own screen takes over
  const shownPrompt = prompt ?? (connected ? undefined : lastAsk(host, pane))
  const reconnecting = useReconnecting(machine?.status)
  const queue = chat?.info?.queue ?? NO_QUEUE
  const held = chat?.info?.held ?? NO_HELD
  const organize = useStore((s) => s.organize)
  const scheduled = useMemo(() => sendsFor(organize, host, pane), [organize, host, pane])
  // where the chat has no list to put them in (nothing running, or a terminal-first agent), the
  // messages waiting to go still show, with their Send now and Cancel
  const scheduledList = scheduled.length > 0 && (
    <div className="mt-6 space-y-5">
      {scheduled.map((s) => (
        <ScheduledBubble key={s.id} host={host} pane={pane} s={s} />
      ))}
    </div>
  )
  const queued = useMemo(() => new Set(queue.map(msgKey)), [queue])
  // what the agent is writing right now, and the running command's output for the working line
  const live = chat?.live ?? NO_LIVE
  const liveOutput = working ? live.findLast((l) => l.kind === 'output')?.text : undefined
  const liveShown = live.some((l) => l.kind !== 'output' && l.text.trim())
  // when the turn started: the agent's own record first, else read off its screen
  const since = working ? (chat?.info?.turn?.startedAt ?? screen?.since) : undefined
  // the hub follows every send into the agent's record (info.sends); a bubble lasts until it's there.
  // One waiting in the agent's queue shows once, as a queued bubble, until the agent takes it.
  const stored = chat?.pending ?? NO_PENDING
  const sends = chat?.info?.sends
  const unconfirmed = useMemo(() => (stored.length ? settleSends(stored, chat?.items ?? [], sends) : NO_PENDING), [stored, chat?.items, sends])
  const pending = useMemo(
    () => [...unconfirmed.filter((p) => !p.queued && !queued.has(msgKey(p.text))), ...othersSends(sends, stored).filter((p) => !queued.has(msgKey(p.text)))],
    [unconfirmed, queued, sends, stored],
  )
  useEffect(() => {
    if (JSON.stringify(unconfirmed) !== JSON.stringify(stored)) patchChat(key, (c) => ({ pending: settleSends(c.pending, c.items, c.info?.sends) }))
  }, [key, stored, unconfirmed])
  const tasks = chat?.info?.tasks
  const subagents = chat?.info?.subagents
  const jobs = useMemo(
    () => ({ tasks: new Map((tasks ?? []).map((t) => [t.id, t])), subagents: new Map((subagents ?? []).map((a) => [a.toolUseId, a])) }),
    [tasks, subagents],
  )

  const saveAnchor = () => {
    const el = scroller.current
    if (!el || !list.current) return
    const top = el.getBoundingClientRect().top
    const rows = [...list.current.children]
    const first = rows.findIndex((row) => row.getBoundingClientRect().bottom > top)
    anchor.current = first < 0 ? [] : rows.slice(first, first + 3).map((row) => ({ el: row, top: row.getBoundingClientRect().top - top }))
  }

  // the latest loadMore, for the scroll handler (which is made once)
  const more = useRef<() => void>(() => {})

  const onScroll = useCallback(() => {
    const el = scroller.current
    if (!el) return
    const away = el.scrollHeight - el.scrollTop - el.clientHeight
    if (away < 24) stick.current = true
    else if (el.scrollTop < lastTop.current - 2 && Date.now() - inputAt.current < 1200) stick.current = false
    // scrolling up towards the top fetches earlier messages before the reader gets there
    if (!stick.current && el.scrollTop < lastTop.current && el.scrollTop < Math.max(400, el.clientHeight)) more.current()
    lastTop.current = el.scrollTop
    setAtBottom(away < 80)
    saveAnchor()
  }, [])

  const toBottom = (smooth = false) => {
    const el = scroller.current
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
    lastTop.current = el.scrollTop
  }

  /** After content changes: follow the bottom, or keep the row being read exactly where it was. */
  const settle = () => {
    const el = scroller.current
    if (!el) return
    if (stick.current) return toBottom()
    const a = anchor.current.find((x) => x.el.isConnected)
    if (!a) return saveAnchor()
    const delta = a.el.getBoundingClientRect().top - el.getBoundingClientRect().top - a.top
    if (Math.abs(delta) >= 1) {
      el.scrollTop += delta
      lastTop.current = el.scrollTop
    }
  }

  // sending a message yourself always goes to the bottom and follows again, wherever you were
  const sentIds = useRef<Set<string> | null>(null)
  useLayoutEffect(() => {
    const ids = new Set(stored.map((p) => p.id))
    const fresh = sentIds.current && [...ids].some((id) => !sentIds.current!.has(id))
    sentIds.current = ids
    if (fresh) stick.current = true
  }, [stored])

  useLayoutEffect(settle, [rows, working, prompt, pending.length, queue.length, stored, live])

  // older messages arrived (they're prepended; settle() kept the reading position)
  const firstSeq = chat?.items[0]?.seq
  useEffect(() => setLoadingMore(false), [firstSeq, chat?.moreAt])
  // a reply lost with the connection shouldn't leave the top spinning
  useEffect(() => {
    if (!loadingMore) return
    const t = setTimeout(() => setLoadingMore(false), 30_000)
    return () => clearTimeout(t)
  }, [loadingMore])

  useEffect(() => {
    stick.current = true
    requestAnimationFrame(() => toBottom())
  }, [key, chat?.loaded])

  // back after a while (the phone in a pocket, another tab): show the latest, not where you left off.
  // This session's notification is read now, too.
  useEffect(() => {
    if (!document.hidden) clearNativeNotification(notifyTag(host, pane))
    let hiddenAt = document.hidden ? Date.now() : 0
    const onVisible = () => {
      if (document.hidden) return void (hiddenAt = Date.now())
      clearNativeNotification(notifyTag(host, pane))
      if (hiddenAt && Date.now() - hiddenAt > AWAY_MS) {
        stick.current = true
        requestAnimationFrame(() => toBottom())
      }
      hiddenAt = 0
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => document.removeEventListener('visibilitychange', onVisible)
  }, [host, pane])

  // images, highlighted code and opened tool rows change heights after render
  useEffect(() => {
    const el = scroller.current
    if (!el) return
    const ro = new ResizeObserver(() => settle())
    ro.observe(el)
    if (list.current) ro.observe(list.current)
    // a finger or wheel moving the conversation down (reading back) stops following at once,
    // before the first scroll event, so streaming output can't pull the reader back
    const wheel = (e: WheelEvent) => {
      inputAt.current = Date.now()
      if (e.deltaY < 0) stick.current = false
    }
    const start = (e: TouchEvent) => {
      inputAt.current = Date.now()
      touchY.current = e.touches[0]?.clientY ?? null
    }
    const move = (e: TouchEvent) => {
      inputAt.current = Date.now()
      const y = e.touches[0]?.clientY
      if (touchY.current !== null && y !== undefined && y > touchY.current + 4) stick.current = false
    }
    // keys (PageUp, arrows) and dragging the scrollbar
    const other = () => void (inputAt.current = Date.now())
    el.addEventListener('wheel', wheel, { passive: true })
    el.addEventListener('touchstart', start, { passive: true })
    el.addEventListener('touchmove', move, { passive: true })
    el.addEventListener('keydown', other)
    el.addEventListener('pointerdown', other)
    return () => {
      ro.disconnect()
      el.removeEventListener('wheel', wheel)
      el.removeEventListener('touchstart', start)
      el.removeEventListener('touchmove', move)
      el.removeEventListener('keydown', other)
      el.removeEventListener('pointerdown', other)
    }
  }, [chat?.loaded])

  // a sent message the agent hasn't written yet: keep the bubble, but say so after a while
  useEffect(() => {
    if (!pending.some((p) => p.status === 'sent' && !p.remote)) return
    const t = setInterval(() => patchChat(key, (c) => ({ pending: c.pending.map((p) => p.status === 'sent' && !p.queued && Date.now() - p.at >= PENDING_MS
      ? { ...p, status: 'failed', error: "Still not in the conversation after 10 minutes. Check the terminal before retrying." }
      : p) })), 5000)
    return () => clearInterval(t)
  }, [key, pending])

  const loadMore = () => {
    const first = chat?.items[0]
    if (!first || loadingMore || !chat?.hasMore) return
    stick.current = false
    saveAnchor()
    setLoadingMore(true)
    socket.send({ t: 'chat.more', host, pane, before: first.seq })
  }
  more.current = loadMore

  /** `auto`: the resend when the connection is back, which is a retry already: one go, and if it fails too the message waits for a tap */
  const retry = async (id: string, text: string, auto = false) => {
    patchChat(key, (c) => ({ pending: c.pending.map((p) => (p.id === id ? { ...p, status: 'sending', at: Date.now(), retryable: false } : p)) }))
    try {
      await (auto ? api.send(host, pane, text, id) : withRetries(() => api.send(host, pane, text, id)))
      patchChat(key, (c) => ({ pending: c.pending.map((p) => (p.id === id ? { ...p, status: 'sent' } : p)) }))
    } catch (e) {
      patchChat(key, (c) => ({ pending: c.pending.map((p) => (p.id === id ? { ...p, status: 'failed', error: (e as Error).message, retryable: !auto && isTransient(e) } : p)) }))
    }
  }

  // messages that failed only because the connection dropped go out by themselves once it's back,
  // once: a failure that only looks like a dropped connection would otherwise resend on every reconnect
  useEffect(() => {
    if (!connected) return
    for (const p of useStore.getState().chats[key]?.pending ?? []) if (p.status === 'failed' && p.retryable && !p.remote) void retry(p.id, p.text, true)
  }, [connected, key])

  const rewind = useCallback(async (text: string) => {
    const short = text.length > 60 ? `${text.slice(0, 60)}…` : text
    if (!(await confirmAction({ title: 'Rewind the conversation?', body: `Codex forgets "${short}" and everything after it.`, yes: 'Rewind', danger: true }))) return
    try {
      await api.rewind(host, pane, text)
      toast({ title: 'Conversation rewound', tone: 'success' })
    } catch (e) {
      toast({ title: 'Could not rewind', body: (e as Error).message, tone: 'error' })
    }
  }, [host, pane])

  const lastOpen = (() => {
    const last = rows[rows.length - 1]
    return last?.type === 'assistant' && last.open
  })()

  // which rows come in rising: the ones added after the conversation was on screen. The first
  // batch is drawn still, so are earlier messages joining at the top, and a message that was a
  // bubble a moment ago (sent from here, or queued) takes the bubble's place without moving.
  // Each row's answer is kept, so a render while it rises can't take the class off it
  const arrivals = useRef<Map<string, boolean>>(new Map())
  const lastSeq = useRef<number | undefined>(undefined)
  const bubbles = useRef<Set<string>>(new Set())
  const arriving = useMemo(() => {
    const now = new Map<string, boolean>()
    if (!chat?.loaded) return now
    const seq = (k: string) => Number(k.slice(1))
    const settled = lastSeq.current !== undefined
    const decide = (k: string, fresh: boolean) => now.set(k, arrivals.current.get(k) ?? (settled && fresh))
    for (const r of rows) decide(r.key, seq(r.key) > (lastSeq.current ?? Infinity) && !(r.type === 'user' && bubbles.current.has(r.item.text)))
    for (const p of pending) decide(`p${p.id}`, true)
    for (const q of queue) decide(`q${q}`, true)
    lastSeq.current = Math.max(lastSeq.current ?? 0, ...rows.map((r) => seq(r.key)))
    bubbles.current = new Set([...pending.map((p) => p.text), ...queue])
    arrivals.current = now
    return now
  }, [rows, pending, queue, chat?.loaded])
  const rowView = (r: (typeof rows)[number]) => {
    switch (r.type) {
      case 'user':
        return <UserMessage row={r} />
      case 'assistant':
        return <AssistantTurn row={r} live={working} />
      case 'command':
        return <CommandRow row={r} />
      case 'goal':
        return <GoalRow row={r} />
      case 'notice':
        return <NoticeRow row={r} />
    }
  }

  return (
    <ChatAgent.Provider value={agent}>
    <ChatHost.Provider value={host}>
    <ChatRewind.Provider value={agent === 'codex' && running && !working ? rewind : undefined}>
    <ChatJobs.Provider value={jobs}>
      <ReplyFont.Provider value={font}>
        <div className="flex min-h-0 flex-1 flex-col">
          <div ref={scroller} onScroll={onScroll} className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain [overflow-anchor:none]">
            <div className="mx-auto w-full max-w-[760px] px-4 pt-5 pb-6 sm:px-6">
              {/* a terminal-first agent has no chat for the hub to load, so nothing waits on one */}
              {!chatCapable && !running ? (
                <>
                  <NotRunning host={host} pane={pane} agent={agent} installed={installed} onTerminal={onTerminal} />
                  {scheduledList}
                </>
              ) : !chatCapable ? (
                <>
                  <Empty icon={<AgentLogo kind={agent} size={28} />} title={`${agentDef(agent!).name} runs in the terminal`} action={<Button onClick={onTerminal}><SquareTerminal className="size-4" /> Open terminal</Button>}>
                    Inyon doesn't read {agentDef(agent!).name}'s conversation files yet, so use the terminal view. Status and notifications still work.
                  </Empty>
                  {scheduledList}
                </>
              ) : !chat?.loaded ? (
                <Waiting shape="lines" className="pt-4" label={chat?.catchingUp !== undefined ? `Loading the conversation · ${Math.round(chat.catchingUp * 100)}%` : 'Loading the conversation'} />
              ) : !running && !rows.length ? (
                <>
                  <NotRunning host={host} pane={pane} agent={agent} installed={installed} sessionId={chat.info?.sessionId} onTerminal={onTerminal} />
                  {scheduledList}
                </>
              ) : (
                <>
                  {chat.hasMore && (
                    <div className="mb-4 min-h-8">
                      {loadingMore ? (
                        // taller than the button it replaces: settle() keeps the reading position as the top grows
                        <Waiting shape="lines" count={2} label="Loading earlier messages" className="pb-2" />
                      ) : (
                        <div className="flex justify-center">
                          <Button size="sm" variant="ghost" onClick={loadMore}>
                            <History className="size-4" /> Earlier messages
                          </Button>
                        </div>
                      )}
                    </div>
                  )}
                  {!rows.length && !pending.length && (
                    <Empty icon={<AgentLogo kind={agent} size={28} />} title="New conversation">
                      Messages you send here are typed into the terminal, exactly as if you typed them there.
                    </Empty>
                  )}
                  <div ref={list} className="space-y-5">
                    {rows.map((r) => (
                      <div key={r.key} className={arriving.get(r.key) ? 'animate-slide-up' : undefined}>{rowView(r)}</div>
                    ))}
                    {pending.map((p) => (
                      <div key={p.id} className={arriving.get(`p${p.id}`) ? 'animate-slide-up' : undefined}>
                        <PendingBubble p={p} onRetry={() => retry(p.id, p.text)} onDismiss={() => patchChat(key, (c) => ({ pending: c.pending.filter((x) => x.id !== p.id) }))} onScreen={() => setScreenOpen(true)} />
                      </div>
                    ))}
                    {queue.map((q, i) => (
                      <div key={`${i}:${q}`} className={arriving.get(`q${q}`) ? 'animate-slide-up' : undefined}>
                        <QueuedBubble text={q} agent={agent} />
                      </div>
                    ))}
                    {held.map((p, i) => (
                      <div key={p.id} className="animate-slide-up">
                        <HeldBubble host={host} pane={pane} p={p} index={i} count={held.length} agentName={agent ? agentDef(agent).name : 'the agent'} />
                      </div>
                    ))}
                    {scheduled.map((s) => (
                      <div key={s.id} className="animate-slide-up">
                        <ScheduledBubble host={host} pane={pane} s={s} />
                      </div>
                    ))}
                    {chat.catchingUp !== undefined && <Reading label="Catching up on what you missed" done={chat.catchingUp} />}
                    {liveShown && chat.catchingUp === undefined && <LiveTurn items={live} />}
                    {working && !prompt && chat.catchingUp === undefined && (
                      <div className={lastOpen || liveShown ? '-mt-3' : ''}>
                        <WorkingIndicator activity={screen?.activity} since={since} output={liveOutput} />
                      </div>
                    )}
                    {!working && !prompt && screen?.background && (
                      <div className="animate-fade-in flex items-center gap-3 py-1 text-[13px] text-fg-3">
                        <div className="flex size-6 shrink-0 items-center justify-center">
                          <span className="size-1.5 rounded-full bg-fg-3 pulse-soft" />
                        </div>
                        {backgroundText(screen.background)}
                      </div>
                    )}
                    {!working && !prompt && !screen?.background && failure && (
                      <div role="status" className="animate-fade-in flex items-center gap-3 py-1 text-[13px] text-fg-2">
                        <div className="flex size-6 shrink-0 items-center justify-center">
                          <span className="size-1.5 rounded-full bg-danger" />
                        </div>
                        {failure}
                      </div>
                    )}
                  </div>
                  {!running && (
                    <div className="mt-8">
                      <NotRunning host={host} pane={pane} agent={agent} installed={installed} sessionId={chat.info?.sessionId} onTerminal={onTerminal} compact />
                    </div>
                  )}
                </>
              )}
            </div>
          </div>

          <div className="relative flex min-h-0 flex-col">
            {/* always there, so it can rise in from the composer and sink back out instead of appearing and vanishing */}
            <button
              type="button"
              inert={atBottom}
              aria-hidden={atBottom}
              onClick={() => {
                // straight there in one tap: a glide is slow over a long chat and new output cuts it short
                stick.current = true
                toBottom()
              }}
              // at the side, over the end of a line rather than the middle of one (on a wide
              // screen, beside the conversation)
              className={clsx(
                'absolute -top-12 right-4 z-10 flex size-9 items-center justify-center rounded-full border border-line bg-elev text-fg-2 shadow-float transition-[opacity,transform] hover:text-fg sm:right-6',
                atBottom ? 'translate-y-2 opacity-0 duration-(--dur-exit) ease-in' : 'translate-y-0 opacity-100',
              )}
              aria-label="Jump to latest"
            >
              <ArrowDown className="size-4" />
            </button>
            {running && chatCapable && (
              <div className="pb-safe-2 mx-auto flex min-h-0 w-full max-w-[760px] flex-col px-3 pt-1 sm:px-6 sm:pb-4">
                {!hubUp ? (
                  <div className="animate-fade-in mb-2 text-center text-[12px] text-fg-3" role="status">Reconnecting…{!isPhone() && <HelpLink id="self" />}</div>
                ) : machineDown && reconnecting ? (
                  <div className="animate-fade-in mb-2 text-center text-[12px] text-fg-3" role="status">Reconnecting to {machine!.config.name}…<HelpLink id={host} /></div>
                ) : machineDown ? (
                  <div className="animate-fade-in mb-2 text-center text-[12px] text-fg-3">{machine!.config.name} is offline. This is the conversation as last seen; you can send again once it reconnects.<HelpLink id={host} /></div>
                ) : null}
                {shownPrompt ? <PromptCard prompt={shownPrompt} host={host} pane={pane} disabled={!connected} /> : <Composer host={host} pane={pane} agent={agent} working={working} disabled={!connected} controls={controls} ultracode={ultracode} />}
              </div>
            )}
          </div>
        </div>
        <FailedScreen host={host} pane={pane} open={screenOpen} onClose={() => setScreenOpen(false)} />
      </ReplyFont.Provider>
    </ChatJobs.Provider>
    </ChatRewind.Provider>
    </ChatHost.Provider>
    </ChatAgent.Provider>
  )
}

/** How long a machine that isn't answering counts as reconnecting rather than offline. */
export const reconnect = { graceMs: 15_000 }

/**
 * True for a while after the chat opens, the app comes back to the front, or the machine drops:
 * the link is most likely coming back by itself (an app opened from an alert reconnects in a
 * few seconds), so the chat says it's reconnecting before it calls the machine offline.
 */
function useReconnecting(status: HostStatus | undefined) {
  const [fresh, setFresh] = useState(true)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const restart = useCallback(() => {
    clearTimeout(timer.current)
    setFresh(true)
    timer.current = setTimeout(() => setFresh(false), reconnect.graceMs)
  }, [])
  useEffect(() => {
    restart()
    const onVisible = () => !document.hidden && restart()
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      clearTimeout(timer.current)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }, [restart])
  const was = useRef(status)
  useEffect(() => {
    if (was.current === 'online' && status !== 'online') restart()
    was.current = status
  }, [status, restart])
  return fresh
}

function NotRunning({
  host,
  pane,
  agent,
  installed,
  sessionId,
  onTerminal,
  compact,
}: {
  host: string
  pane: string
  agent?: AgentKind
  installed: AgentKind[]
  sessionId?: string
  onTerminal: () => void
  compact?: boolean
}) {
  const [busy, setBusy] = useState<string | null>(null)
  const kinds = (installed.length ? installed : (['claude'] as AgentKind[])).filter((k, i, a) => a.indexOf(k) === i)
  const start = async (kind: AgentKind, resume?: string) => {
    setBusy(resume ? 'resume' : kind)
    try {
      await api.start(host, pane, kind, resume)
    } catch (e) {
      toast({ title: 'Could not start', body: (e as Error).message, tone: 'error' })
    } finally {
      setTimeout(() => setBusy(null), 2500)
    }
  }
  return (
    <div className={compact ? 'rounded-3xl border border-line bg-elev shadow-card p-5 text-center' : 'py-12 text-center'}>
      <p className="text-[15px] font-semibold">No agent is running in this pane</p>
      <p className="mx-auto mt-1 max-w-sm text-[13.5px] text-fg-2">The session ended or the pane is at a shell prompt.</p>
      <div className="mt-5 flex flex-wrap justify-center gap-2">
        {sessionId && agent && (
          <Button variant="accent" onClick={() => start(agent, sessionId)} loading={busy === 'resume'}>
            <History className="size-4" /> Resume conversation
          </Button>
        )}
        {!!agent && !!agentDef(agent).conversations?.length && (
          <Button variant="ghost" onClick={() => openMenu('conversations')}>
            <History className="size-4" /> Past conversations
          </Button>
        )}
        {kinds.map((k) => (
          <Button key={k} onClick={() => start(k)} loading={busy === k}>
            <AgentLogo kind={k} size={16} /> Start {agentDef(k).name}
          </Button>
        ))}
        <Button variant="ghost" onClick={onTerminal}>
          <SquareTerminal className="size-4" /> Terminal
        </Button>
      </div>
      {!kinds.length && <Play className="hidden" />}
    </div>
  )
}

/** A read the hub is doing out of sight; the chat updates all at once when it's done. */
function Reading({ label, done }: { label: string; done: number }) {
  return (
    <div className="animate-fade-in flex items-center gap-3 py-1 text-[13px] text-fg-3" role="status">
      <Pulse className="mx-[3px]" />
      <span>{label}…</span>
      <span className="tabular-nums">{Math.round(done * 100)}%</span>
    </div>
  )
}
