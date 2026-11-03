import { useState } from 'react'
import clsx from 'clsx'
import { ArrowDown, ArrowUp, CircleAlert, Hourglass, Pencil, Send, X } from 'lucide-react'
import type { HeldPrompt } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { haptic } from '../../lib/haptics.ts'
import { contextMenu, copyItem } from '../ui/ContextMenu.tsx'
import { Button, Sheet } from '../ui/primitives.tsx'

function failed(title: string, e: unknown) {
  haptic('error')
  toast({ title, body: (e as Error).message, tone: 'error' })
}

/** A message the hub holds until the agent is free: it can still be sent now, edited, moved or dropped. */
export function HeldBubble({ host, pane, p, index, count, agentName }: { host: string; pane: string; p: HeldPrompt; index: number; count: number; agentName: string }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(p.text)
  const [busy, setBusy] = useState(false)
  const sendNow = async () => {
    setBusy(true)
    try {
      await api.sendHeld(host, pane, p.id)
    } catch (e) {
      failed("Couldn't send it", e)
    } finally {
      setBusy(false)
    }
  }
  const move = (to: number) => void api.editHeld(host, pane, p.id, { to }).catch((e) => failed("Couldn't move it", e))
  const drop = async () => {
    try {
      await api.dropHeld(host, pane, p.id)
      toast({
        title: 'Message dropped',
        tone: 'success',
        // held again in the same place
        action: { label: 'Undo', onSelect: () => void api.hold(host, pane, p.text).then((q) => api.editHeld(host, pane, q.id, { to: index })).catch((e) => failed("Couldn't put it back", e)) },
      })
    } catch (e) {
      failed("Couldn't drop it", e)
    }
  }
  const save = async () => {
    try {
      await api.editHeld(host, pane, p.id, { text: draft })
      setEditing(false)
    } catch (e) {
      failed("Couldn't save it", e)
    }
  }
  const openEdit = () => {
    setDraft(p.text)
    setEditing(true)
  }
  return (
    <div className="flex flex-col items-end">
      <div
        {...contextMenu(() => [
          { key: 'now', label: 'Send now', icon: <Send className="size-4" />, disabled: busy, onSelect: () => void sendNow() },
          { key: 'edit', label: 'Edit', icon: <Pencil className="size-4" />, onSelect: openEdit },
          index > 0 && { key: 'up', label: 'Move up', icon: <ArrowUp className="size-4" />, onSelect: () => move(index - 1) },
          index < count - 1 && { key: 'down', label: 'Move down', icon: <ArrowDown className="size-4" />, onSelect: () => move(index + 1) },
          copyItem('copy', 'Copy message', p.text, 'Message'),
          { key: 'drop', label: 'Drop', icon: <X className="size-4" />, separated: true, danger: true, onSelect: () => void drop() },
        ])}
        className={clsx('max-w-[88%] whitespace-pre-wrap break-words rounded-3xl border border-dashed px-4 py-2.5 text-[15px] leading-[1.55] text-fg-2 sm:max-w-[78%]', p.error ? 'border-danger/60' : 'border-line-strong')}
      >
        {p.text}
      </div>
      <div className="mt-1 flex h-6 items-center gap-1.5 px-1 text-[12px] text-fg-3">
        {p.error ? (
          <span className="flex items-center gap-1 text-danger">
            <CircleAlert className="size-3" /> {p.error}
          </span>
        ) : (
          <span className="flex items-center gap-1">
            <Hourglass className="size-3" /> {index === 0 ? `Held · goes when ${agentName} is free` : `Held · ${index + 1} of ${count}`}
          </span>
        )}
        <button type="button" onClick={() => void sendNow()} disabled={busy} title="Send now" aria-label="Send now" className="flex size-6 items-center justify-center rounded-md hover:bg-hover hover:text-fg disabled:opacity-50">
          <Send className="size-3.5" />
        </button>
        <button type="button" onClick={() => void drop()} title="Drop" aria-label="Drop" className="flex size-6 items-center justify-center rounded-md hover:bg-hover hover:text-fg">
          <X className="size-3.5" />
        </button>
      </div>
      <Sheet
        open={editing}
        onClose={() => setEditing(false)}
        title="Edit the held message"
        icon={<Pencil className="size-5" />}
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void save()} disabled={!draft.trim()}>
              Save
            </Button>
          </div>
        }
      >
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          aria-label="Message"
          rows={6}
          className="block w-full resize-y rounded-2xl border border-line bg-raised p-3 text-[15px] leading-[1.5] text-fg"
        />
      </Sheet>
    </div>
  )
}
