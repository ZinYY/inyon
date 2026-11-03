import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { LEAVE_MS } from '../components/ui/AnimatedList.tsx'
import { socket } from '../lib/socket.ts'
import { useParams, useSearchParams } from 'react-router-dom'
import { LogIn } from 'lucide-react'
import { agentDef, chatKey, type BackgroundTask, type SubagentInfo } from '@inyon/protocol'
import { paneFromSlug } from '../lib/api.ts'
import { hydrateChat, useStore } from '../lib/store.ts'
import { paneTitle } from '../lib/overview.ts'
import { SkinScope } from '../lib/skin.tsx'
import { ChatActionsContext } from '../lib/chatActions.ts'
import { Button, Empty, Waiting } from '../components/ui/primitives.tsx'
import { SignInSheet } from '../components/SignInSheet.tsx'
import { useSession } from '../components/session/useSession.ts'
import { SessionHeader } from '../components/session/SessionHeader.tsx'
import { ComposerControls } from '../components/session/ComposerControls.tsx'
import type { PanelTab } from '../components/session/Panels.tsx'

// the side panel and its sheets bring the markdown renderer: load them with the chat, not the app
const SidePanel = lazy(() => import('../components/session/Panels.tsx').then((m) => ({ default: m.SidePanel })))
const SubagentSheet = lazy(() => import('../components/session/Panels.tsx').then((m) => ({ default: m.SubagentSheet })))
const TaskOutputSheet = lazy(() => import('../components/session/Panels.tsx').then((m) => ({ default: m.TaskOutputSheet })))
import { ActivitySheet, ControlsSheet } from '../components/session/MobileSheets.tsx'

// split out: markdown/highlighting and xterm are the heaviest parts of the app
const ChatView = lazy(() => import('../components/chat/ChatView.tsx').then((m) => ({ default: m.ChatView })))
const TerminalView = lazy(() => import('../components/terminal/TerminalView.tsx').then((m) => ({ default: m.TerminalView })))

// the chat column's shape, whether the chat or the terminal is what opens
const Loading = ({ machine }: { machine?: string }) => (
  <div className="mx-auto w-full max-w-[760px] flex-1 px-4 pt-5 sm:px-6">
    <Waiting shape="lines" count={4} label={machine ? `Opening the session on ${machine}` : 'Opening the session'} />
  </div>
)

const useWide = () => {
  const [wide, setWide] = useState(() => matchMedia('(min-width: 1180px)').matches)
  useEffect(() => {
    const mq = matchMedia('(min-width: 1180px)')
    const on = () => setWide(mq.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  return wide
}

export function PanePage() {
  const params = useParams()
  const host = decodeURIComponent(params.host!)
  const pane = paneFromSlug(params.pane!)
  const [search, setSearch] = useSearchParams()
  const loaded = useStore((s) => s.hostsLoaded)
  const sidePanel = useStore((s) => s.settings.sidePanel)
  const setSettings = useStore((s) => s.setSettings)
  const s = useSession(host, pane)
  // asked for at once, whichever view shows: a reopened app has the chat's saved copy (and so
  // knows the agent) before the machine has answered with its panes
  useEffect(() => {
    hydrateChat(chatKey(host, pane))
    return socket.subscribeChat(host, pane)
  }, [host, pane])
  const { ref, info, agent } = s
  // a terminal-first agent opens on its terminal: its chat tab only points there
  const home = agent && agentDef(agent).chat ? 'chat' : 'terminal'
  const view = (search.get('view') as 'chat' | 'terminal' | null) ?? home
  const wide = useWide()
  const [tab, setTab] = useState<PanelTab>('changes')
  const [sheet, setSheet] = useState<'controls' | 'activity' | null>(null)
  const [signIn, setSignIn] = useState(false)
  const [task, setTask] = useState<BackgroundTask | null>(null)
  const [sub, setSub] = useState<SubagentInfo | null>(null)
  const items = s.chat?.items ?? []

  const title = (ref?.pane.companion && 'Companion') || ref?.pane.label || info?.title || (ref ? paneTitle(ref) : 'Session')
  useEffect(() => {
    document.title = `${title} · Inyon`
    return () => {
      document.title = 'Inyon'
    }
  }, [title])

  const setView = (v: 'chat' | 'terminal') => setSearch(v === home ? {} : { view: v }, { replace: true })
  const actions = useMemo(
    () => ({
      openSubagent: (id: string) => {
        const a = s.info?.subagents.find((x) => x.toolUseId === id)
        setSub(a ?? { toolUseId: id, description: 'Subagent', status: 'done' })
      },
      openTask: setTask,
    }),
    [s.info?.subagents],
  )

  if (!ref && loaded && !info) {
    return (
      <div className="flex flex-1 flex-col">
        <Empty title="This pane is gone">The tmux pane was closed or its machine is unreachable.</Empty>
      </div>
    )
  }

  const showPanel = !!agent && wide && sidePanel
  // the panel stays a moment after it is closed, fading, before it goes (as a sheet does)
  const [panelShown, setPanelShown] = useState(showPanel)
  useEffect(() => {
    if (showPanel) return setPanelShown(true)
    const t = setTimeout(() => setPanelShown(false), LEAVE_MS)
    return () => clearTimeout(t)
  }, [showPanel])

  return (
    <ChatActionsContext.Provider value={actions}>
      <SkinScope agent={agent} className="flex min-h-0 flex-1 flex-col bg-bg text-fg">
        <SessionHeader
          s={s}
          view={view}
          setView={setView}
          panelOpen={sidePanel}
          togglePanel={() => setSettings({ sidePanel: !sidePanel })}
          openSheet={(which) => {
            if (which === 'activity' && showPanel) setTab('activity')
            else setSheet(which)
          }}
        />
        {/* an agent that stays in its terminal asks you to sign in there itself */}
        {agent && agentDef(agent).chat && ref?.host.signedIn && !ref.host.signedIn.includes(agent) && (
          <div className="flex shrink-0 items-center gap-3 border-b border-line bg-warn-soft/60 px-4 py-2 text-[13px]">
            <LogIn className="size-4 shrink-0 text-warn" />
            <span className="min-w-0 flex-1">{agentDef(agent).name} isn't signed in on {ref.host.config.name}.</span>
            <Button size="xs" variant="primary" onClick={() => setSignIn(true)}>Sign in</Button>
          </div>
        )}
        {ref && <SignInSheet hostId={ref.host.config.id} hostName={ref.host.config.name} agent={signIn && agent ? agent : null} onClose={() => setSignIn(false)} />}
        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <Suspense fallback={<Loading machine={ref?.host.config.name} />}>
              {!ref && !info ? (
                <Loading />
              ) : view === 'chat' ? (
                // each view rises in over the one it replaces; another session's chat comes in the same way
                <div key={`chat:${host}${pane}`} className="animate-page flex min-h-0 flex-1 flex-col">
                  <ChatView host={host} pane={pane} agent={agent} onTerminal={() => setView('terminal')} controls={agent ? <ComposerControls s={s} /> : undefined} ultracode={s.ultracode} />
                </div>
              ) : (
                <div key="terminal" className="animate-page flex min-h-0 flex-1 flex-col">
                  <TerminalView key={`${host}${pane}`} host={host} pane={pane} agent={agent} />
                </div>
              )}
            </Suspense>
          </div>
          {panelShown && (
            // an empty panel holds the room while the real one loads, so the chat doesn't get wider and narrower again
            <Suspense fallback={<aside className="w-[420px] shrink-0 border-l border-line bg-sidebar 2xl:w-[480px]" />}>
              <SidePanel s={s} items={items} tab={tab} setTab={setTab} onTask={setTask} onSubagent={setSub} closing={!showPanel} />
            </Suspense>
          )}
        </div>
        <ControlsSheet s={s} open={sheet === 'controls'} onClose={() => setSheet(null)} />
        <ActivitySheet s={s} items={items} open={sheet === 'activity'} onClose={() => setSheet(null)} onTask={setTask} onSubagent={setSub} />
        <Suspense fallback={null}>
          {task && <TaskOutputSheet host={host} task={task} onClose={() => setTask(null)} />}
          {sub && <SubagentSheet s={s} sub={sub} onClose={() => setSub(null)} />}
        </Suspense>
      </SkinScope>
    </ChatActionsContext.Provider>
  )
}
