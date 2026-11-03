import { useEffect, useRef, useState } from 'react'
import { MoreHorizontal, Pencil, Plus, Trash2 } from 'lucide-react'
import { agentDef, AGENTS, type AgentKind } from '@inyon/protocol'
import { api, type CustomAgent, type HubSettings } from '../lib/api.ts'
import { toast } from '../lib/store.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { useOpened } from '../lib/useOpened.ts'
import { Button, Field, Menu, Sheet, inputClass, type MenuItem } from './ui/primitives.tsx'
import { Select } from './ui/Select.tsx'
import { contextMenu } from './ui/ContextMenu.tsx'

type Draft = Pick<CustomAgent, 'name' | 'command' | 'like'>

export const OLD_HUB = 'This hub runs on Bun, which keeps no custom agents. Run the default Rust hub to add them.'

/**
 * Your own agents at the end of Settings › Agents: a row each, with the same menu on right-click
 * and on ⋯, and a row that adds one. The hub checks the list and gives each an id.
 */
export function CustomAgentRows({ hub, onSaved }: { hub: HubSettings; onSaved: (h: HubSettings) => void }) {
  const list = hub.customAgents ?? []
  const [editing, setEditing] = useState<CustomAgent | 'new' | null>(null)
  // an Undo after another save must not put back the settings from before it
  const latest = useRef(hub)
  useEffect(() => {
    latest.current = hub
  }, [hub])
  const save = async (next: Draft[]) => {
    const saved = await api.saveSettings({ ...latest.current, customAgents: next as CustomAgent[] })
    // the Bun hub (INYON_HUB_IMPL=bun) doesn't keep them
    if (!saved.customAgents) throw new Error(OLD_HUB)
    onSaved(saved)
  }

  const remove = async (a: CustomAgent) => {
    const before = list
    try {
      await save(list.filter((x) => x.id !== a.id))
      toast({ title: `Removed ${a.name}`, tone: 'info', action: { label: 'Undo', onSelect: () => void save(before).catch((e) => toast({ title: 'Could not undo', body: (e as Error).message, tone: 'error' })) } })
    } catch (e) {
      toast({ title: 'Could not remove', body: (e as Error).message, tone: 'error' })
    }
  }
  const menu = (a: CustomAgent): MenuItem[] => [
    { key: 'edit', label: 'Edit', icon: <Pencil className="size-4" />, onSelect: () => setEditing(a) },
    { key: 'remove', label: 'Remove', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void remove(a) },
  ]

  return (
    <>
      {list.map((a) => (
        <div key={a.id} className="flex items-start gap-3 px-4 py-3.5 text-[14px] sm:items-center" {...contextMenu(() => menu(a), { header: a.name })}>
          <AgentLogo kind={a.like} size={28} />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium">{a.name}</span>
            <span className="block truncate text-[12px] text-fg-3">Works like {agentDef(a.like).name}</span>
            {/* phones: under the name, as the built-in agents' chips wrap */}
            <code className="mt-2 inline-block max-w-full truncate rounded-full border border-line bg-code px-2 py-1 font-mono text-[12px] text-fg-2 sm:hidden" title={a.command}>{a.command}</code>
          </span>
          <code className="hidden min-w-0 max-w-[45%] truncate rounded-full border border-line bg-code px-2 py-1 font-mono text-[12px] text-fg-2 sm:block" title={a.command}>{a.command}</code>
          <Menu
            align="end"
            width={200}
            header={a.name}
            items={menu(a)}
            trigger={({ toggle, ref }) => (
              <button ref={ref} type="button" onClick={toggle} aria-label={`Options for ${a.name}`} className="-mr-1.5 flex size-8 shrink-0 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg sm:-my-1">
                <MoreHorizontal className="size-4" />
              </button>
            )}
          />
        </div>
      ))}
      <button type="button" onClick={() => setEditing('new')} className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium transition-colors hover:bg-hover">
        <span className="flex size-7 items-center justify-center rounded-full border border-dashed border-line-strong text-fg-3">
          <Plus className="size-4" />
        </span>
        Add an agent
      </button>
      <CustomAgentSheet
        open={editing !== null}
        agent={editing === 'new' ? undefined : (editing ?? undefined)}
        onClose={() => setEditing(null)}
        onSave={async (d) => {
          const old = editing === 'new' ? undefined : editing
          await save(old ? list.map((x) => (x.id === old.id ? d : x)) : [...list, d])
          toast({ title: old ? `Saved ${d.name}` : `Added ${d.name}`, body: old ? undefined : `New sessions can start it, and Inyon reads it as ${agentDef(d.like).name}.`, tone: 'success' })
        }}
      />
    </>
  )
}

// what a custom agent can work like: the agents whose conversation Inyon reads
const KINDS = (Object.keys(AGENTS) as AgentKind[]).filter((k) => agentDef(k).chat)

export function CustomAgentSheet({ open, agent, onClose, onSave }: { open: boolean; agent?: CustomAgent; onClose: () => void; onSave: (d: Draft) => Promise<void> }) {
  const [name, setName] = useState('')
  const [command, setCommand] = useState('')
  const [like, setLike] = useState<AgentKind>('claude')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  useOpened(open, agent?.id ?? 'new', () => {
    setName(agent?.name ?? '')
    setCommand(agent?.command ?? '')
    setLike(agent?.like ?? 'claude')
    setError('')
  })

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim() || !command.trim()) return setError('Give it a name and a command.')
    setBusy(true)
    setError('')
    try {
      await onSave({ name: name.trim(), command: command.trim(), like })
      onClose()
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={agent ? `Edit ${agent.name}` : 'Add an agent'}
      footer={
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" form="custom-agent-form" loading={busy}>{agent ? 'Save' : 'Add'}</Button>
        </div>
      }
    >
      <form id="custom-agent-form" onSubmit={submit} className="space-y-4">
        <Field label="Name">
          <input value={name} onChange={(e) => setName(e.target.value)} maxLength={40} placeholder="Claude YOLO" className={inputClass} autoFocus />
        </Field>
        <Field label="Command" hint="What a new session types. Inyon also spots it running in any pane.">
          <input value={command} onChange={(e) => setCommand(e.target.value)} maxLength={300} placeholder="claude --dangerously-skip-permissions" spellCheck={false} autoCapitalize="off" autoCorrect="off" className={`${inputClass} font-mono text-[13px]`} />
        </Field>
        <div>
          <span className="mb-1.5 block text-[12.5px] font-medium text-fg-2">Works like</span>
          <Select
            title="Works like"
            aria-label="Works like"
            value={like}
            onChange={setLike}
            options={KINDS.map((k) => ({ value: k, label: <span className="flex items-center gap-2"><AgentLogo kind={k} size={18} /> {agentDef(k).name}</span> }))}
          />
          <span className="mt-1.5 block text-[12px] leading-relaxed text-fg-3">Its logo, chat view, prompts and hooks come from this agent.</span>
        </div>
        {error && <p role="alert" className="text-[13px] text-danger">{error}</p>}
      </form>
    </Sheet>
  )
}
