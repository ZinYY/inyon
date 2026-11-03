import { useRef, useState } from 'react'
import clsx from 'clsx'
import { ArrowRight, History } from 'lucide-react'
import { agentDef, type AgentKind, type PastConversation } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { shortPath, timeAgo } from '../../lib/format.ts'
import { useOpened } from '../../lib/useOpened.ts'
import { confirmAction } from '../Confirm.tsx'
import { Button, Empty, Pulse, Sheet, Spinner, Waiting } from '../ui/primitives.tsx'
import { contextMenu, copyItem } from '../ui/ContextMenu.tsx'

type Loaded = { state: 'loading' } | { state: 'error'; error: string } | { state: 'ready'; cwd: string; list: PastConversation[] }

/**
 * The agent's past conversations in this session's folder, and switching to one: what /resume
 * does in the agent's terminal, as a list the app can show. The agent saves the open conversation
 * as it exits, then starts again on the one picked; the chat follows by itself.
 */
export function ConversationsSheet({ host, pane, agent, working, open, onClose }: { host: string; pane: string; agent: AgentKind; working: boolean; open: boolean; onClose: () => void }) {
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  const [switching, setSwitching] = useState<string | null>(null)
  const list = useRef<HTMLDivElement>(null)
  const name = agentDef(agent).name

  const load = async () => {
    setLoaded({ state: 'loading' })
    try {
      const r = await api.conversations(host, pane, agent)
      setLoaded({ state: 'ready', cwd: r.cwd, list: r.conversations })
    } catch (e) {
      setLoaded({ state: 'error', error: (e as Error).message })
    }
  }
  useOpened(open, `${host} ${pane} ${agent}`, load)

  const pick = async (c: PastConversation) => {
    if (c.current || switching) return
    if (working && !(await confirmAction({ title: 'Switch conversation?', body: 'The current one is saved and can be resumed later.', yes: 'Switch' }))) return
    setSwitching(c.id)
    try {
      await api.switchConversation(host, pane, agent, c.id)
      onClose()
      toast({ title: 'Conversation switched', body: c.title, tone: 'success' })
    } catch (e) {
      toast({ title: 'Could not switch', body: (e as Error).message, tone: 'error' })
    } finally {
      setSwitching(null)
    }
  }

  // ↑ and ↓ move between conversations, as in the agent's own picker
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    const rows = [...(list.current?.querySelectorAll<HTMLButtonElement>('button[data-row]') ?? [])]
    if (!rows.length) return
    e.preventDefault()
    const at = rows.indexOf(document.activeElement as HTMLButtonElement)
    const next = at < 0 ? 0 : Math.max(0, Math.min(rows.length - 1, at + (e.key === 'ArrowDown' ? 1 : -1)))
    rows[next].focus()
  }

  return (
    <Sheet open={open} onClose={onClose} title="Past conversations" icon={<History className="size-4 text-fg-3" />}>
      {loaded.state === 'loading' && <Waiting shape="rows" count={4} label={`Reading ${name}'s conversations…`} />}
      {loaded.state === 'error' && (
        <Empty title="Couldn't load conversations" action={<Button onClick={load}>Try again</Button>}>
          {loaded.error}
        </Empty>
      )}
      {loaded.state === 'ready' && !loaded.list.length && (
        <Empty icon={<History className="size-5" />} title="No past conversations">
          Conversations with {name} in {shortPath(loaded.cwd, 32)} show up here.
        </Empty>
      )}
      {loaded.state === 'ready' && loaded.list.length > 0 && (
        <>
          <p className="mb-2 truncate font-mono text-[12px] text-fg-3">
            {switching ? (
              <span role="status" className="flex items-center gap-2 font-sans text-[13px] text-fg-2">
                <Pulse /> Saving this conversation and opening the other…
              </span>
            ) : (
              `${name} · ${shortPath(loaded.cwd, 40)}`
            )}
          </p>
          <div ref={list} onKeyDown={onKey} className="stagger -mx-2 space-y-0.5" aria-label="Conversations">
            {loaded.list.map((c, i) => (
              <button
                key={c.id}
                type="button"
                data-row
                aria-current={c.current || undefined}
                disabled={!!switching && switching !== c.id}
                onClick={() => pick(c)}
                {...contextMenu(
                  () => [
                    !c.current && !switching && { key: 'open', label: 'Switch to it', icon: <ArrowRight className="size-4" />, onSelect: () => void pick(c) },
                    copyItem('copy', 'Copy conversation ID', c.id, 'Conversation ID'),
                  ],
                  { touch: true },
                )}
                style={{ '--i': Math.min(i, 8) } as React.CSSProperties}
                className={clsx(
                  'press flex min-h-12 w-full items-center gap-3 rounded-2xl px-3 py-2.5 text-left disabled:opacity-50',
                  c.current ? 'cursor-default bg-sunken' : 'hover:bg-hover',
                )}
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[14.5px] font-medium">{c.title}</span>
                  <span className="mt-0.5 block font-mono text-[12px] text-fg-3">
                    {switching === c.id ? 'Switching…' : c.current ? `Open now · ${timeAgo(c.at)}` : timeAgo(c.at)}
                  </span>
                </span>
                {switching === c.id && <Spinner />}
              </button>
            ))}
          </div>
        </>
      )}
    </Sheet>
  )
}
