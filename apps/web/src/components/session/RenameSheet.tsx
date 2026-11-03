import { useState } from 'react'
import { Pencil } from 'lucide-react'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { useOpened } from '../../lib/useOpened.ts'
import { Button, Field, Sheet, inputClass } from '../ui/primitives.tsx'

/**
 * Names a session (or a plain terminal's tmux window). Empty goes back to the automatic name.
 * With `session` (tmux's id, `$52`) it renames that tmux session instead, which has no automatic name.
 */
export function RenameSheet({ host, pane, current, agent, session, open, onClose }: { host: string; pane: string; current: string; agent: boolean; session?: string; open: boolean; onClose: () => void }) {
  const [name, setName] = useState(current)
  const [busy, setBusy] = useState(false)
  useOpened(open, pane, () => setName(current))
  const save = async (value: string) => {
    setBusy(true)
    try {
      if (session) await api.renameTmuxSession(host, session, value)
      else await api.rename(host, pane, value)
      onClose()
    } catch (e) {
      toast({ title: 'Could not rename', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={session ? 'Rename tmux session' : agent ? 'Rename session' : 'Rename terminal'}
      icon={<Pencil className="size-4 text-fg-3" />}
      footer={
        <div className="flex items-center gap-2">
          {!session && (
            <Button variant="ghost" onClick={() => save('')} disabled={busy}>
              Use automatic name
            </Button>
          )}
          <Button className="ml-auto" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="rename-form" loading={busy} disabled={!name.trim()}>
            Save
          </Button>
        </div>
      }
    >
      <form id="rename-form" onSubmit={(e) => (e.preventDefault(), name.trim() && save(name))}>
        <Field label="Name" hint={
            session
              ? 'tmux on that machine shows it too. Dots and colons become underscores.'
              : agent
                ? 'Shown in Inyon and kept with the conversation, so it survives a resume.'
                : 'Renames the tmux window, so tmux shows it too.'
          }>
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} maxLength={80} autoFocus onFocus={(e) => e.currentTarget.select()} />
        </Field>
      </form>
    </Sheet>
  )
}
