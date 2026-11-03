import { useState } from 'react'
import clsx from 'clsx'
import { AlarmClock, CalendarClock, StickyNote, Tag } from 'lucide-react'
import { api, type SessionPatch } from '../../lib/api.ts'
import { toast, useStore } from '../../lib/store.ts'
import { useOpened } from '../../lib/useOpened.ts'
import { markFor, parseTags, SEND_CHOICES, type SendFor, sendAt, SNOOZE_CHOICES, type SnoozeFor, snoozeUntil, timeText, wakeText } from '../../lib/organize.ts'
import type { PaneRef } from '../../lib/overview.ts'
import { Button, Field, Sheet, Toggle, inputClass } from '../ui/primitives.tsx'

/** The hub keeps these (inyon-protocol's organize.rs); a sheet stops there instead of failing. */
const MAX_TAGS = 8
const NOTE_LEN = 500

/** Changes how a session is organised; true when the hub took it. */
/** Changes how a session is organised; true when the hub took it (with the mark it keeps now, if any). */
export async function organizePane(r: PaneRef, patch: SessionPatch, failed = 'Could not change the session') {
  return (await organizeMark(r, patch, failed)) !== false
}

/** The same, giving back the mark the hub keeps now (null when nothing's left), or false when it failed. */
export async function organizeMark(r: PaneRef, patch: SessionPatch, failed = 'Could not change the session') {
  try {
    return (await api.organize(r.host.config.id, r.pane.id, patch)).session
  } catch (e) {
    toast({ title: failed, body: (e as Error).message, tone: 'error' })
    return false
  }
}

/** The mark on a pane as the screens have it now (menus are built when they open). */
export const currentMark = (r: PaneRef) => markFor(useStore.getState().organize, r.host.config.id, r.pane.id)

/** Hides a session until a time this device picks, and can raise a notice when it's back. */
export function SnoozeSheet({ r, open, onClose }: { r: PaneRef; open: boolean; onClose: () => void }) {
  const [notice, setNotice] = useState(false)
  const [busy, setBusy] = useState<string>()
  useOpened(open, r.host.config.id + r.pane.id, () => setNotice(!!currentMark(r)?.snoozeNotice))
  const finished = useStore((s) => s.settings.notifyDone)
  const now = new Date()
  // the time is worked out on the tap, so a sheet left open a while doesn't snooze for less
  const pick = async (v: SnoozeFor) => {
    setBusy(v)
    if (await organizePane(r, { snoozedUntil: snoozeUntil(v), snoozeNotice: notice || null }, 'Could not snooze')) onClose()
    setBusy(undefined)
  }
  return (
    <Sheet open={open} onClose={onClose} title="Snooze" icon={<AlarmClock className="size-4 text-fg-3" />}>
      <p className="mb-3 text-[13px] leading-relaxed text-fg-2">It moves to Parked and snoozed on every device and comes back on its own. The agent keeps running.</p>
      <div className="-mx-2 flex flex-col">
        {SNOOZE_CHOICES.map((c) => {
          const until = snoozeUntil(c.v, now)
          return (
            <button
              key={c.v}
              type="button"
              disabled={!!busy}
              onClick={() => void pick(c.v)}
              className={clsx('flex h-11 items-center gap-3 rounded-xl px-2 text-left text-[14px] hover:bg-hover disabled:opacity-60', busy === c.v && 'bg-hover')}
            >
              <span className="min-w-0 flex-1 truncate">{c.label}</span>
              <span className="tabular shrink-0 font-mono text-[12px] text-fg-3">{wakeText(until, now)}</span>
            </button>
          )
        })}
      </div>
      <div className="mt-3 flex items-center gap-3 border-t border-line pt-3">
        <span className="min-w-0 flex-1 text-[13px] text-fg-2">Tell me when it's back</span>
        <Toggle on={notice} onChange={setNotice} label="Tell me when it's back" />
      </div>
      {notice && !finished && (
        <p className="mt-2 text-[12px] leading-relaxed text-fg-3">It comes with the finished notices, which are off on this device. Turn on “When an agent finishes” in Settings to get it.</p>
      )}
    </Sheet>
  )
}

/** Picks when the hub sends a message: it waits then for the agent to be free, like a held one. */
export function SendLaterSheet({ host, pane, text, open, onClose, onSent }: { host: string; pane: string; text: string; open: boolean; onClose: () => void; onSent: () => void }) {
  const [busy, setBusy] = useState<string>()
  const now = new Date()
  // worked out on the tap, like a snooze
  const pick = async (v: SendFor) => {
    setBusy(v)
    const at = sendAt(v)
    try {
      const s = await api.sendLater(host, pane, text.trim(), at)
      onSent()
      onClose()
      toast({ title: `Sends at ${timeText(at)}`, body: 'It waits for the agent to be free then, on every device.', tone: 'success', action: { label: 'Undo', onSelect: () => void api.cancelSend(host, s.id).catch((e) => toast({ title: 'Could not undo', body: (e as Error).message, tone: 'error' })) } })
    } catch (e) {
      toast({ title: 'Could not schedule it', body: (e as Error).message, tone: 'error' })
    } finally {
      setBusy(undefined)
    }
  }
  return (
    <Sheet open={open} onClose={onClose} title="Send later" icon={<CalendarClock className="size-4 text-fg-3" />}>
      <p className="mb-3 line-clamp-3 whitespace-pre-wrap break-words text-[13px] leading-relaxed text-fg-2">{text.trim()}</p>
      <div className="-mx-2 flex flex-col">
        {SEND_CHOICES.map((c) => {
          const at = sendAt(c.v, now)
          return (
            <button
              key={c.v}
              type="button"
              disabled={!!busy}
              onClick={() => void pick(c.v)}
              className={clsx('flex h-11 items-center gap-3 rounded-xl px-2 text-left text-[14px] hover:bg-hover disabled:opacity-60', busy === c.v && 'bg-hover')}
            >
              <span className="min-w-0 flex-1 truncate">{c.label}</span>
              <span className="tabular shrink-0 font-mono text-[12px] text-fg-3">{timeText(at, now)}</span>
            </button>
          )
        })}
      </div>
    </Sheet>
  )
}

/** A session's tags, typed in one box with commas between them. */
export function TagsSheet({ r, open, onClose }: { r: PaneRef; open: boolean; onClose: () => void }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  useOpened(open, r.host.config.id + r.pane.id, () => setText((currentMark(r)?.tags ?? []).join(', ')))
  const tags = parseTags(text)
  const save = async () => {
    setBusy(true)
    if (await organizePane(r, { tags: tags.length ? tags : null }, 'Could not save the tags')) onClose()
    setBusy(false)
  }
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Tags"
      icon={<Tag className="size-4 text-fg-3" />}
      footer={
        <div className="flex items-center gap-2">
          <Button className="ml-auto" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="tags-form" loading={busy} disabled={tags.length > MAX_TAGS}>
            Save
          </Button>
        </div>
      }
    >
      <form id="tags-form" onSubmit={(e) => (e.preventDefault(), void save())}>
        <Field label="Tags" hint={tags.length > MAX_TAGS ? `A session can have ${MAX_TAGS} tags at most.` : 'Commas between them. The search finds sessions by their tags.'}>
          <input className={inputClass} value={text} onChange={(e) => setText(e.target.value)} placeholder="review, docs" autoFocus autoCapitalize="none" />
        </Field>
      </form>
    </Sheet>
  )
}

/** A few lines kept with a session, for whoever looks at it next. */
export function NoteSheet({ r, open, onClose }: { r: PaneRef; open: boolean; onClose: () => void }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  useOpened(open, r.host.config.id + r.pane.id, () => setText(currentMark(r)?.note ?? ''))
  const save = async (note: string) => {
    setBusy(true)
    if (await organizePane(r, { note: note.trim() || null }, 'Could not save the note')) onClose()
    setBusy(false)
  }
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Note"
      icon={<StickyNote className="size-4 text-fg-3" />}
      footer={
        <div className="flex items-center gap-2">
          {!!currentMark(r)?.note && (
            <Button variant="ghost" onClick={() => void save('')} disabled={busy}>
              Remove
            </Button>
          )}
          <Button className="ml-auto" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="note-form" loading={busy}>
            Save
          </Button>
        </div>
      }
    >
      <form id="note-form" onSubmit={(e) => (e.preventDefault(), void save(text))}>
        <Field label="Note" hint={`Kept with the session on every device, and found by the search. ${text.length}/${NOTE_LEN}`}>
          <textarea rows={4} value={text} onChange={(e) => setText(e.target.value)} maxLength={NOTE_LEN} autoFocus className={clsx(inputClass, 'h-auto resize-none py-2.5 leading-relaxed')} />
        </Field>
      </form>
    </Sheet>
  )
}
