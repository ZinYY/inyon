import { createContext, useState } from 'react'
import clsx from 'clsx'
import { Check, Copy } from 'lucide-react'
import { copyText } from '../../lib/clipboard.ts'
import { toast } from '../../lib/store.ts'

/** Reply font for the agent whose session is on screen. */
export const ReplyFont = createContext<'serif' | 'sans' | 'mono'>('sans')

export function CopyButton({ text, className, label = 'Copy' }: { text: string; className?: string; label?: string }) {
  const [done, setDone] = useState(false)
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={clsx('inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-fg-3 transition-colors hover:bg-hover hover:text-fg', className)}
      onClick={async (e) => {
        e.stopPropagation()
        if (!await copyText(text)) {
          toast({ title: 'Could not copy', body: 'Select the message text to copy it manually.', tone: 'error' })
          return
        }
        setDone(true)
        setTimeout(() => setDone(false), 1200)
      }}
    >
      {done ? <Check className="animate-fade-in size-3.5 text-fg" /> : <Copy className="size-3.5" />}
    </button>
  )
}
