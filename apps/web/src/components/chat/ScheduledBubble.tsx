import { useState } from 'react'
import clsx from 'clsx'
import { CalendarClock, CircleAlert, Send, X } from 'lucide-react'
import type { DelayedSend } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { haptic } from '../../lib/haptics.ts'
import { timeText } from '../../lib/organize.ts'
import { contextMenu, copyItem } from '../ui/ContextMenu.tsx'

function failed(title: string, e: unknown) {
  haptic('error')
  toast({ title, body: (e as Error).message, tone: 'error' })
}

/** A message the hub sends at a set time (from 0.24): it can still go now or be cancelled. */
export function ScheduledBubble({ host, pane, s }: { host: string; pane: string; s: DelayedSend }) {
  const [busy, setBusy] = useState(false)
  // now: off the schedule and into the held queue, in one step on the hub
  const sendNow = async () => {
    setBusy(true)
    try {
      await api.sendNow(host, s.id)
    } catch (e) {
      failed("Couldn't send it", e)
    } finally {
      setBusy(false)
    }
  }
  const cancel = async () => {
    try {
      await api.cancelSend(host, s.id)
      if (!s.error)
        toast({ title: 'Message cancelled', tone: 'success', action: { label: 'Undo', onSelect: () => void api.sendLater(host, pane, s.text, s.at, s.conv ?? undefined).catch((e) => failed("Couldn't put it back", e)) } })
    } catch (e) {
      failed("Couldn't cancel it", e)
    }
  }
  return (
    <div className="flex flex-col items-end">
      <div
        {...contextMenu(() => [
          { key: 'now', label: 'Send now', icon: <Send className="size-4" />, disabled: busy, onSelect: () => void sendNow() },
          copyItem('copy', 'Copy message', s.text, 'Message'),
          { key: 'cancel', label: s.error ? 'Dismiss' : 'Cancel', icon: <X className="size-4" />, separated: true, danger: !s.error, onSelect: () => void cancel() },
        ])}
        className={clsx('max-w-[88%] whitespace-pre-wrap break-words rounded-3xl border border-dashed px-4 py-2.5 text-[15px] leading-[1.55] text-fg-2 sm:max-w-[78%]', s.error ? 'border-danger/60' : 'border-line-strong')}
      >
        {s.text}
      </div>
      <div className="mt-1 flex h-6 items-center gap-1.5 px-1 text-[12px] text-fg-3">
        {s.error ? (
          <span className="flex items-center gap-1 text-danger">
            <CircleAlert className="size-3" /> {s.error}
          </span>
        ) : (
          <span className="flex items-center gap-1">
            <CalendarClock className="size-3" /> Sends at {timeText(s.at)}
          </span>
        )}
        <button type="button" onClick={() => void sendNow()} disabled={busy} title="Send now" aria-label="Send now" className="flex size-6 items-center justify-center rounded-md hover:bg-hover hover:text-fg disabled:opacity-50">
          <Send className="size-3.5" />
        </button>
        <button type="button" onClick={() => void cancel()} title={s.error ? 'Dismiss' : 'Cancel'} aria-label={s.error ? 'Dismiss' : 'Cancel'} className="flex size-6 items-center justify-center rounded-md hover:bg-hover hover:text-fg">
          <X className="size-3.5" />
        </button>
      </div>
    </div>
  )
}
