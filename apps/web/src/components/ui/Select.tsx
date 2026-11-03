import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import clsx from 'clsx'
import { Check, ChevronDown } from 'lucide-react'
import { haptic } from '../../lib/haptics.ts'
import { useBackCloses } from '../../lib/back.ts'

export interface SelectOption<T extends string> {
  value: T
  label: ReactNode
  hint?: ReactNode
}

/** Phones get a sheet from the bottom; a mouse gets a list under the field. */
const touchLike = () => typeof window !== 'undefined' && (window.matchMedia?.('(pointer: coarse)').matches || window.innerWidth < 640)

/**
 * A select that looks like the rest of the app. The browser's own picker is a full-screen system
 * dialog on Android, so this draws its own: a bottom sheet on phones, a list on desktop.
 */
export function Select<T extends string>({
  value,
  onChange,
  options,
  title,
  className,
  'aria-label': ariaLabel,
}: {
  value: T
  onChange: (v: T) => void
  options: SelectOption<T>[]
  /** heading of the phone sheet, usually the field's label */
  title?: string
  className?: string
  'aria-label'?: string
}) {
  const [open, setOpen] = useState(false)
  const [sheet, setSheet] = useState(false)
  const [active, setActive] = useState(0)
  const button = useRef<HTMLButtonElement>(null)
  const list = useRef<HTMLDivElement>(null)
  const id = useId()
  const current = options.find((o) => o.value === value)

  const show = () => {
    setSheet(touchLike())
    setActive(Math.max(0, options.findIndex((o) => o.value === value)))
    setOpen(true)
  }
  const close = (refocus = true) => {
    setOpen(false)
    if (refocus) button.current?.focus({ preventScroll: true })
  }
  useBackCloses(open, () => close())
  const pick = (o: SelectOption<T>) => {
    if (o.value !== value) {
      haptic('select')
      onChange(o.value)
    }
    close()
  }

  // keys go to the list while it's open; Escape closes only the list, not the sheet underneath
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Tab') {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopImmediatePropagation()
        }
        close(e.key === 'Escape')
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((a) => Math.max(0, Math.min(options.length - 1, a + (e.key === 'ArrowDown' ? 1 : -1))))
      } else if (e.key === 'Home' || e.key === 'End') {
        e.preventDefault()
        setActive(e.key === 'Home' ? 0 : options.length - 1)
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        e.stopImmediatePropagation()
        const o = options[active]
        if (o) pick(o)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open, active, options, value])

  useEffect(() => {
    if (open) list.current?.querySelector<HTMLElement>(`[data-i="${active}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [open, active])

  return (
    <>
      <button
        ref={button}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        aria-label={ariaLabel ?? title}
        onClick={() => (open ? close() : show())}
        onKeyDown={(e) => {
          if (!open && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
            e.preventDefault()
            show()
          }
        }}
        className={clsx(
          'press flex h-10 w-full items-center gap-2 rounded-xl border border-field bg-elev px-3 text-left text-[14px] text-fg transition-colors focus:border-fg',
          open && 'border-line-strong',
          className,
        )}
      >
        <span className="min-w-0 flex-1 truncate">{current?.label ?? ''}</span>
        <ChevronDown className={clsx('size-4 shrink-0 text-fg-3 transition-transform', open && 'rotate-180')} />
      </button>
      {open &&
        (sheet ? (
          <PickerSheet id={id} title={title} options={options} value={value} active={active} onPick={pick} onClose={() => close()} listRef={list} />
        ) : (
          <PickerList id={id} anchor={button} options={options} value={value} active={active} setActive={setActive} onPick={pick} onClose={() => close()} listRef={list} />
        ))}
    </>
  )
}

type ListProps<T extends string> = {
  id: string
  options: SelectOption<T>[]
  value: T
  active: number
  onPick: (o: SelectOption<T>) => void
  onClose: () => void
  listRef: React.RefObject<HTMLDivElement | null>
}

/** `stagger`: the rows come in one after another (the phone sheet; a list by a mouse is up at once) */
function Options<T extends string>({ id, options, value, active, onPick, listRef, setActive, roomy, stagger }: ListProps<T> & { setActive?: (i: number) => void; roomy?: boolean; stagger?: boolean }) {
  return (
    <div ref={listRef} id={id} role="listbox" className={clsx(stagger && 'stagger')}>
      {options.map((o, i) => (
        <button
          key={o.value}
          data-i={i}
          type="button"
          role="option"
          aria-selected={o.value === value}
          onMouseEnter={() => setActive?.(i)}
          onClick={() => onPick(o)}
          style={{ '--i': Math.min(i, 8) } as React.CSSProperties}
          className={clsx(
            'press flex w-full items-center gap-3 rounded-xl text-left text-fg',
            roomy ? 'min-h-12 px-3.5 py-2.5 text-[15px]' : 'px-2.5 py-2 text-[13.5px]',
            i === active && 'bg-hover',
          )}
        >
          <span className="min-w-0 flex-1">
            <span className="block truncate">{o.label}</span>
            {o.hint && <span className="block truncate text-[12px] text-fg-3">{o.hint}</span>}
          </span>
          <Check className={clsx('size-4 shrink-0 transition-[opacity,transform]', o.value === value ? 'scale-100 opacity-100' : 'scale-[0.97] opacity-0')} />
        </button>
      ))}
    </div>
  )
}

function PickerSheet<T extends string>(props: ListProps<T> & { title?: string }) {
  const [closing, setClosing] = useState(false)
  const leave = (then: () => void) => {
    setClosing(true)
    setTimeout(then, 190)
  }
  return createPortal(
    <div className="fixed inset-0 z-[80] flex items-end justify-center" role="dialog" aria-modal aria-label={props.title}>
      <div className={clsx('absolute inset-0 bg-black/45', closing ? 'animate-fade-out' : 'animate-fade-in')} onClick={() => leave(props.onClose)} />
      <div className={clsx('pb-safe-2 relative max-h-[70dvh] w-full overflow-y-auto rounded-t-3xl border border-line bg-elev px-2 pt-2 shadow-float', closing ? 'animate-sheet-out' : 'animate-sheet-in')}>
        <div className="mx-auto mb-1 h-1 w-9 rounded-full bg-line-strong" />
        {props.title && <p className="px-3.5 pt-2 pb-1.5 text-[12.5px] font-medium text-fg-3">{props.title}</p>}
        <Options {...props} roomy stagger onPick={(o) => leave(() => props.onPick(o))} />
      </div>
    </div>,
    document.body,
  )
}

function PickerList<T extends string>(props: ListProps<T> & { anchor: React.RefObject<HTMLButtonElement | null>; setActive: (i: number) => void }) {
  const [pos, setPos] = useState<{ top?: number; bottom?: number; left: number; width: number }>({ left: 0, width: 0 })
  useLayoutEffect(() => {
    const r = props.anchor.current?.getBoundingClientRect()
    if (!r) return
    const up = r.bottom + 300 > window.innerHeight && r.top > 300
    setPos({ left: r.left, width: r.width, ...(up ? { bottom: window.innerHeight - r.top + 6 } : { top: r.bottom + 6 }) })
  }, [])
  return createPortal(
    <>
      <div className="fixed inset-0 z-[80]" onClick={props.onClose} />
      {/* it opens out of the field: down from under it, or up from over it when there is no room below */}
      <div
        className={clsx('fixed z-[81] max-h-[min(320px,60dvh)] overflow-y-auto rounded-2xl border border-line bg-elev p-1 shadow-float', pos.bottom !== undefined ? 'animate-pop-up origin-bottom' : 'animate-pop origin-top')}
        style={pos}
      >
        <Options {...props} />
      </div>
    </>,
    document.body,
  )
}
