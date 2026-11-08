import { useLayoutEffect, useRef, type ComponentProps, type ReactNode, type RefObject } from 'react'
import clsx from 'clsx'
import { ArrowUp } from 'lucide-react'

/** Shared presentation for native and saved-session chats. Transport stays outside. */
export function ConversationColumn({ className, ...props }: ComponentProps<'div'>) {
  return <div {...props} className={clsx('mx-auto w-full max-w-[760px] px-4 pt-5 pb-6 sm:px-6', className)} />
}

export function ComposerDock({ children }: { children: ReactNode }) {
  return <div className="pb-safe-2 mx-auto flex min-h-0 w-full max-w-[760px] flex-col px-3 pt-1 sm:px-6 sm:pb-4">{children}</div>
}

export function ComposerSurface({ children, dragging, disabled, ultracode }: { children: ReactNode; dragging?: boolean; disabled?: boolean; ultracode?: boolean }) {
  return <div className={clsx('rounded-3xl border bg-elev shadow-card transition-colors', dragging ? 'border-accent' : 'border-line focus-within:border-line-strong', disabled && 'opacity-60', ultracode && 'ultracode-box')}>{children}</div>
}

export function ComposerTextArea({ shellMode, ...props }: ComponentProps<'textarea'> & { shellMode?: boolean }) {
  return <textarea {...props} className={clsx('block max-h-[40vh] w-full resize-none bg-transparent px-4 pb-1 text-[15.5px] leading-[1.5] text-fg placeholder:text-fg-3', shellMode ? 'pt-1' : 'pt-3')} />
}

export function ComposerSendButton({ active, stop, ...props }: ComponentProps<'button'> & { active: boolean; stop?: boolean }) {
  return <button {...props} className={clsx('press flex size-9 shrink-0 items-center justify-center rounded-full', stop ? 'bg-fg text-bg hover:opacity-85 disabled:opacity-50' : active ? 'bg-accent text-accent-fg hover:opacity-90' : 'bg-sunken text-fg-3')} />
}

export function SessionHeaderBar({ children }: { children: ReactNode }) {
  return <div className="flex h-[60px] items-center gap-1.5 px-2 sm:px-5">{children}</div>
}

export function SessionHeading({ title, subtitle }: { title: ReactNode; subtitle: ReactNode }) {
  return <div className="min-w-0 flex-1 pl-1"><h1 className="flex items-center gap-1.5 text-[16px] font-semibold leading-tight tracking-[-0.03em]">{title}</h1><p className="mt-0.5 truncate font-mono text-[12px] text-fg-3">{subtitle}</p></div>
}

export function useComposerHeight(ref: RefObject<HTMLTextAreaElement | null>, text: string) {
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const max = window.innerHeight * 0.4
    el.style.height = '0px'
    const height = el.scrollHeight
    el.style.height = `${Math.min(height, max)}px`
    el.style.overflowY = height > max + 1 ? 'auto' : 'hidden'
  }, [text])
}

export function PlainComposer({ value, onChange, onSend, disabled, label, controls }: {
  value: string; onChange: (text: string) => void; onSend: () => void; disabled?: boolean; label: string; controls?: ReactNode
}) {
  const ref = useRef<HTMLTextAreaElement>(null)
  useComposerHeight(ref, value)
  const canSend = !!value.trim() && !disabled
  return <form className="animate-fade-in relative" onSubmit={(e) => { e.preventDefault(); if (canSend) onSend() }}>
    <ComposerSurface disabled={disabled}>
      <ComposerTextArea ref={ref} rows={1} value={value} disabled={disabled} aria-label={label} placeholder={`${label}…`}
        onChange={(e) => onChange(e.target.value)} onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing && !matchMedia('(pointer: coarse)').matches) {
            e.preventDefault(); if (canSend) onSend()
          }
        }} />
      <div className="@container flex items-center gap-0.5 px-1.5 pb-1.5">
        <div className="no-scrollbar flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">{controls}</div>
        <ComposerSendButton type="submit" active={canSend} disabled={!canSend} aria-label="Send" title="Send" onMouseDown={(e) => e.preventDefault()}>
          <span className="animate-fade-in flex"><ArrowUp className="size-[17px]" strokeWidth={2.4} /></span>
        </ComposerSendButton>
      </div>
    </ComposerSurface>
  </form>
}
