import {
  forwardRef, useEffect, useId, useLayoutEffect, useRef, useState,
  type ButtonHTMLAttributes, type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import clsx from 'clsx'
import { Check, Loader2, X } from 'lucide-react'
import { useBackCloses } from '../../lib/back.ts'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'accent'

export const Button = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: 'xs' | 'sm' | 'md' | 'lg'; loading?: boolean }
>(function Button({ variant = 'secondary', size = 'md', loading, className, children, disabled, type = 'button', ...rest }, ref) {
  return (
    <button
      ref={ref}
      // a button inside a form only submits it when asked to
      type={type}
      disabled={disabled || loading}
      className={clsx(
        'press inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-full font-medium disabled:opacity-55',
        size === 'xs' && 'h-7 px-3 text-[12px]',
        size === 'sm' && 'h-8 px-3.5 text-[13px]',
        size === 'md' && 'h-9 px-4 text-[13.5px]',
        size === 'lg' && 'h-11 px-5 text-[15px]',
        variant === 'primary' && 'btn-accent',
        variant === 'accent' && 'btn-accent',
        variant === 'secondary' && 'border border-line bg-elev text-fg shadow-card hover:border-line-strong',
        variant === 'ghost' && 'text-fg-2 hover:bg-hover hover:text-fg',
        variant === 'danger' && 'border border-line-strong text-danger hover:bg-danger-soft',
        className,
      )}
      {...rest}
    >
      {loading && <Loader2 className="size-3.5 animate-spin" />}
      {children}
    </button>
  )
})

export function IconButton({
  label,
  className,
  children,
  active,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean; ref?: React.Ref<HTMLButtonElement> }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={clsx(
        'press relative inline-flex size-8 shrink-0 items-center justify-center rounded-full after:absolute after:-inset-1 disabled:opacity-40',
        active ? 'bg-hover text-fg' : 'text-fg-2 hover:bg-hover hover:text-fg',
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  )
}

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-[12.5px] font-medium text-fg-2">{label}</span>
      {children}
      {hint && <span className="mt-1.5 block text-[12px] leading-relaxed text-fg-3">{hint}</span>}
    </label>
  )
}

export const inputClass =
  'h-10 w-full rounded-xl border border-field bg-elev px-3 text-[14px] text-fg placeholder:text-fg-3 transition-colors focus:border-fg focus:bg-elev'

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={clsx('relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors after:absolute after:-inset-2.5', on ? 'bg-accent' : 'bg-field')}
    >
      <span className={clsx('absolute top-[3px] left-0 size-4 rounded-full shadow-sm transition-transform duration-(--dur-move) ease-spring', on ? 'translate-x-[19px] bg-accent-fg' : 'translate-x-[3px] bg-white')} />
    </button>
  )
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = 'md',
  full,
  label,
}: {
  value: T
  /** `name` is what a screen reader calls an option whose label is an icon */
  options: { v: T; label: ReactNode; title?: string; name?: string }[]
  onChange: (v: T) => void
  size?: 'sm' | 'md'
  full?: boolean
  /** what the group as a whole chooses ("View") */
  label?: string
}) {
  // one pill slides under the chosen option instead of each option lighting up on its own. It moves
  // by transform only: the round ends slide, and a strip between them slides and stretches, so the
  // ends never squash the way a scaled pill would.
  const box = useRef<HTMLDivElement>(null)
  const [pill, setPill] = useState<{ left: number; width: number; height: number } | null>(null)
  const index = options.findIndex((o) => o.v === value)
  useLayoutEffect(() => {
    const measure = () => {
      const el = box.current?.children[index + 1] as HTMLElement | undefined
      setPill(el ? { left: el.offsetLeft, width: el.offsetWidth, height: el.offsetHeight } : null)
    }
    measure()
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : undefined
    if (box.current) ro?.observe(box.current)
    return () => ro?.disconnect()
  }, [index, options.length])
  const moved = useRef(false)
  useEffect(() => {
    if (pill) moved.current = true
  }, [pill?.left])
  const r = pill ? pill.height / 2 : 0
  const part = clsx('absolute inset-y-0 left-0 border-y border-line bg-elev shadow-card', moved.current && 'transition-transform duration-(--dur-move) ease-out-soft')
  return (
    <div ref={box} role="radiogroup" aria-label={label} className={clsx('relative flex gap-0.5 rounded-full border border-line bg-sunken p-0.5', full ? 'w-full' : 'w-max')}>
      <span aria-hidden className={clsx('pointer-events-none absolute inset-x-0 top-0.5 bottom-0.5', !pill && 'hidden')}>
        {pill && (
          <>
            {/* the strip runs 1px under each end so no seam shows; it is 100px wide and scaled to fit */}
            <span data-part="strip" className={clsx(part, 'w-[100px] origin-left')} style={{ transform: `translateX(${pill.left + r - 1}px) scaleX(${Math.max(0, pill.width - 2 * r + 2) / 100})` }} />
            <span data-part="start" className={clsx(part, 'rounded-l-full border-l')} style={{ width: r, transform: `translateX(${pill.left}px)` }} />
            <span data-part="end" className={clsx(part, 'rounded-r-full border-r')} style={{ width: r, transform: `translateX(${pill.left + pill.width - r}px)` }} />
          </>
        )}
      </span>
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          role="radio"
          aria-checked={value === o.v}
          aria-label={o.name}
          title={o.title ?? o.name}
          onClick={() => onChange(o.v)}
          className={clsx(
            'press relative flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-full font-medium after:absolute after:-inset-y-1.5 after:inset-x-0',
            size === 'sm' ? 'h-7 px-2.5 text-[12px]' : 'h-8 px-3 text-[13px]',
            full && 'flex-1',
            value === o.v ? 'text-fg' : 'text-fg-3 hover:text-fg-2',
            value === o.v && !pill && 'bg-elev shadow-soft',
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

/** Open sheets, newest last: Tab stays inside the one on top. */
const openSheets: HTMLElement[] = []
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** Bottom sheet on phones, centered dialog on larger screens. */
export function Sheet({
  open,
  onClose,
  title,
  icon,
  children,
  footer,
  wide,
}: {
  open: boolean
  onClose: () => void
  title: ReactNode
  icon?: ReactNode
  children: ReactNode
  footer?: ReactNode
  wide?: boolean
}) {
  const [shown, setShown] = useState(open)
  const titleId = useId()
  const panel = useRef<HTMLDivElement | null>(null)
  // where focus was before it opened, taken while rendering so a field's autoFocus can't get there first
  const opener = useRef<Element | null>(null)
  const wasOpen = useRef(false)
  if (open && !wasOpen.current) opener.current = document.activeElement
  wasOpen.current = open
  useBackCloses(open, onClose)
  useEffect(() => {
    if (open) setShown(true)
    else {
      const t = setTimeout(() => setShown(false), 200)
      return () => clearTimeout(t)
    }
  }, [open])
  useEffect(() => {
    if (!open) return
    // a sheet opened over another one closes alone
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && (!panel.current || !openSheets.length || openSheets.at(-1) === panel.current) && onClose()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])
  // focus goes into the sheet, Tab and Shift-Tab go round inside it, and closing hands focus back
  useEffect(() => {
    const el = panel.current
    if (!open || !shown || !el) return
    if (!el.contains(document.activeElement)) el.focus({ preventScroll: true })
    openSheets.push(el)
    const onTab = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || openSheets.at(-1) !== el) return
      const all = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((x) => !x.hidden)
      const at = document.activeElement
      const first = all[0]
      const last = all[all.length - 1]
      if (!first || !last) {
        e.preventDefault()
        el.focus({ preventScroll: true })
      } else if (!el.contains(at) || (e.shiftKey ? at === first || at === el : at === last)) {
        e.preventDefault()
        ;(e.shiftKey ? last : first).focus()
      }
    }
    document.addEventListener('keydown', onTab)
    return () => {
      document.removeEventListener('keydown', onTab)
      openSheets.splice(openSheets.indexOf(el), 1)
    }
  }, [open, shown])
  useEffect(() => {
    if (!open) return
    return () => {
      const back = opener.current
      opener.current = null
      const at = document.activeElement
      // unless something else took focus meanwhile
      const free = !at || at === document.body || !at.isConnected || !!panel.current?.contains(at)
      if (free && back instanceof HTMLElement && back.isConnected) back.focus({ preventScroll: true })
    }
  }, [open])
  if (!shown) return null
  return createPortal(
    <div className="fixed inset-0 z-50 flex items-end justify-center sm:items-center sm:p-6" role="dialog" aria-modal aria-labelledby={titleId}>
      <div className={clsx('absolute inset-0 bg-black/45', open ? 'animate-fade-in' : 'animate-fade-out')} onClick={onClose} />
      <div
        ref={panel}
        tabIndex={-1}
        className={clsx(
          'relative flex max-h-[92dvh] w-full flex-col rounded-t-3xl border border-line bg-elev text-fg shadow-float outline-none sm:rounded-3xl',
          wide ? 'sm:max-w-3xl' : 'sm:max-w-lg',
          // phones: slides up on the spring; desktop: settles in from just above
          open ? 'animate-sheet-in sm:animate-pop' : 'animate-sheet-out sm:animate-fade-out',
        )}
      >
        <div className="mx-auto mt-2 h-1 w-9 rounded-full bg-line-strong sm:hidden" />
        <div className="flex items-center gap-2.5 px-5 pt-3 pb-1 sm:pt-4">
          {icon}
          <h2 id={titleId} className="min-w-0 flex-1 truncate text-[16px] font-semibold tracking-tight">{title}</h2>
          <IconButton label="Close" onClick={onClose} className="-mr-2">
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="min-h-0 overflow-y-auto px-5 pt-2 pb-5">{children}</div>
        {footer && <div className="pb-safe-2 border-t border-line px-5 pt-3 sm:pb-4">{footer}</div>}
      </div>
    </div>,
    document.body,
  )
}

export interface MenuItem {
  key: string
  label: ReactNode
  hint?: ReactNode
  icon?: ReactNode
  selected?: boolean
  danger?: boolean
  disabled?: boolean
  /** a hairline above it, starting a new group */
  separated?: boolean
  /** its keyboard shortcut, shown on the right ("Ctrl K") */
  keys?: string
  onSelect: () => void
}

/** Anchored dropdown menu; keyboard navigable; closes on outside click. */
/** Opens the menu registered under `name`; false when none is on screen. */
export const openMenu = (name: string) => !window.dispatchEvent(new CustomEvent('inyon:open-menu', { detail: name, cancelable: true }))

export function Menu({
  trigger,
  items,
  header,
  align = 'start',
  width = 260,
  name,
}: {
  trigger: (props: { open: boolean; toggle: () => void; ref: React.RefObject<HTMLButtonElement | null> }) => ReactNode
  items: MenuItem[]
  header?: ReactNode
  align?: 'start' | 'end'
  width?: number
  /** opened from elsewhere with openMenu(name), e.g. when "/models" is typed in the composer */
  name?: string
}) {
  const [open, setOpen] = useState(false)
  useBackCloses(open, () => setOpen(false))
  useEffect(() => {
    if (!name) return
    const onOpen = (e: Event) => {
      if ((e as CustomEvent<string>).detail !== name) return
      e.preventDefault()
      // after the key that asked for it, or that same Enter would pick an item and close it again
      setTimeout(() => setOpen(true))
    }
    window.addEventListener('inyon:open-menu', onOpen)
    return () => window.removeEventListener('inyon:open-menu', onOpen)
  }, [name])
  const ref = useRef<HTMLButtonElement | null>(null)
  return (
    <>
      {trigger({ open, toggle: () => setOpen((o) => !o), ref })}
      {open && <MenuPopup anchor={() => ref.current?.getBoundingClientRect()} items={items} header={header} align={align} width={width} gap={6} onClose={() => setOpen(false)} />}
    </>
  )
}

/**
 * The open menu: placed next to `anchor` (a button, or the point that was right-clicked) on
 * whichever side has room, shrunk to fit when neither does (a phone on its side, or with the
 * keyboard up), and placed again whenever the viewport changes under it.
 */
export function MenuPopup({
  anchor,
  items,
  header,
  align = 'start',
  width = 260,
  gap = 6,
  onClose,
  onOutsideContextMenu,
}: {
  anchor: () => DOMRect | undefined
  items: MenuItem[]
  header?: ReactNode
  align?: 'start' | 'end'
  width?: number
  gap?: number
  onClose: () => void
  /** a right-click outside: closes it, and whatever was clicked may open its own */
  onOutsideContextMenu?: (e: React.MouseEvent) => void
}) {
  const menu = useRef<HTMLDivElement | null>(null)
  const [pos, setPos] = useState<{ top?: number; bottom?: number; left: number; width: number; maxHeight: number }>({ left: 0, width, maxHeight: 420 })
  const [active, setActive] = useState(() => Math.max(0, items.findIndex((i) => i.selected), items.findIndex((i) => !i.disabled)))
  // the next item the keys can land on, skipping disabled ones
  const step = (from: number, by: 1 | -1) => {
    for (let i = from + by; i >= 0 && i < items.length; i += by) if (!items[i].disabled) return i
    return from
  }

  useLayoutEffect(() => {
    const place = () => {
      const r = anchor()
      if (!r) return
      const vv = window.visualViewport
      const top = (vv?.offsetTop ?? 0) + 8
      const bottom = (vv ? vv.offsetTop + vv.height : window.innerHeight) - 8
      const below = Math.max(0, bottom - r.bottom - gap)
      const above = Math.max(0, r.top - gap - top)
      const wanted = Math.min(420, menu.current?.scrollHeight ?? 0)
      const up = below < wanted && above > below
      const w = Math.min(width, window.innerWidth - 16)
      const left = align === 'end' ? r.right - w : r.left
      setPos({
        ...(up ? { bottom: window.innerHeight - Math.max(top, r.top - gap) } : { top: Math.min(r.bottom + gap, bottom) }),
        left: Math.max(8, Math.min(left, window.innerWidth - w - 8)),
        width: w,
        maxHeight: Math.min(420, up ? above : below),
      })
    }
    place()
    window.addEventListener('resize', place)
    window.visualViewport?.addEventListener('resize', place)
    window.visualViewport?.addEventListener('scroll', place)
    return () => {
      window.removeEventListener('resize', place)
      window.visualViewport?.removeEventListener('resize', place)
      window.visualViewport?.removeEventListener('scroll', place)
    }
  }, [])

  const pick = (it: MenuItem | undefined) => {
    if (!it || it.disabled) return
    onClose()
    it.onSelect()
  }

  useEffect(() => {
    // the menu owns these keys while it's open: caught before they reach the focused field, or
    // the Enter that picks a model would also send the message in the composer
    const onKey = (e: KeyboardEvent) => {
      if (!['Escape', 'ArrowDown', 'ArrowUp', 'Enter'].includes(e.key)) return
      e.preventDefault()
      e.stopImmediatePropagation()
      if (e.key === 'Escape') onClose()
      else if (e.key === 'ArrowDown') setActive((a) => step(a, 1))
      else if (e.key === 'ArrowUp') setActive((a) => step(a, -1))
      else if (items[active] && !items[active].disabled) pick(items[active])
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active, items])

  useEffect(() => {
    menu.current?.querySelectorAll('[role=menuitem]')[active]?.scrollIntoView({ block: 'nearest' })
  }, [active])

  return createPortal(
    <>
      <div
        className="fixed inset-0 z-[70]"
        onClick={onClose}
        onContextMenu={(e) => {
          e.preventDefault()
          if (onOutsideContextMenu) onOutsideContextMenu(e)
          else onClose()
        }}
      />
      <div
        ref={menu}
        role="menu"
        // it grows out of its anchor: down from a button above it, up from one below it
        className={clsx(
          'fixed z-[71] overflow-y-auto rounded-2xl border border-line bg-elev p-1 text-fg shadow-float',
          pos.bottom !== undefined ? 'animate-pop-up' : 'animate-pop',
          pos.bottom !== undefined ? (align === 'end' ? 'origin-bottom-right' : 'origin-bottom-left') : align === 'end' ? 'origin-top-right' : 'origin-top-left',
        )}
        style={pos}
        onContextMenu={(e) => e.preventDefault()}
      >
        {header && <div className="px-2.5 pt-1.5 pb-1 text-[12px] font-medium text-fg-3">{header}</div>}
        {items.map((it, i) => (
          <div key={it.key} className={clsx(it.separated && i > 0 && 'mt-1 border-t border-line pt-1')}>
            <button
              type="button"
              role="menuitem"
              disabled={it.disabled}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(it)}
              className={clsx(
                'flex w-full items-center gap-2.5 rounded-xl px-2.5 py-2 text-left text-[13.5px] disabled:opacity-40',
                i === active && 'bg-hover',
                it.danger ? 'text-danger' : 'text-fg',
              )}
            >
              {it.icon && <span className="flex size-4 shrink-0 items-center justify-center text-fg-2">{it.icon}</span>}
              <span className="min-w-0 flex-1">
                <span className="block truncate">{it.label}</span>
                {it.hint && <span className="block truncate text-[12px] text-fg-3">{it.hint}</span>}
              </span>
              {it.keys && <span className="shrink-0 pl-3 font-mono text-[12px] text-fg-3 pointer-coarse:hidden">{it.keys}</span>}
              {it.selected && <Check className="size-4 shrink-0 text-fg-2" />}
            </button>
          </div>
        ))}
      </div>
    </>,
    document.body,
  )
}

/** For a button that is busy. Content that is still coming gets a `Waiting` instead. */
export function Spinner({ className }: { className?: string }) {
  return <Loader2 className={clsx('animate-spin text-fg-3', className ?? 'size-4')} />
}

/** A dot that pulses next to a sentence about work in progress ("Updating…"). */
export function Pulse({ className }: { className?: string }) {
  return <span aria-hidden className={clsx('pulse-soft inline-block size-2 shrink-0 rounded-full bg-fg-2', className)} />
}

export type WaitingShape = 'lines' | 'cards' | 'rows' | 'panel'

// the line widths of a paragraph, so stacked lines don't look like a table
const LINE_WIDTHS = [60, 90, 75, 85, 55, 95, 70, 80]

/**
 * Skeleton blocks in the shape of what is about to appear, and one line saying what it waits for.
 * `.skeleton` shimmers and stands still under reduced motion.
 */
export function Waiting({ label, shape = 'lines', count, className }: { label: string; shape?: WaitingShape; count?: number; className?: string }) {
  const n = count ?? (shape === 'cards' ? 4 : shape === 'panel' ? 1 : 3)
  const blocks = Array.from({ length: n }, (_, i) => i)
  return (
    <div role="status" className={clsx('animate-fade-in', className)}>
      {shape === 'cards' ? (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {blocks.map((i) => <div key={i} className="skeleton h-[170px] rounded-3xl" />)}
        </div>
      ) : shape === 'rows' ? (
        <div className="space-y-2">
          {blocks.map((i) => <div key={i} className="skeleton h-12 rounded-2xl" />)}
        </div>
      ) : shape === 'panel' ? (
        <div className="space-y-3">
          {blocks.map((i) => <div key={i} className="skeleton h-28 rounded-2xl" />)}
        </div>
      ) : (
        <div className="space-y-4">
          {blocks.map((i) => <div key={i} className="skeleton h-4 rounded" style={{ width: `${LINE_WIDTHS[i % LINE_WIDTHS.length]}%` }} />)}
        </div>
      )}
      <p className="mt-3 text-[12.5px] text-fg-3">{label}</p>
    </div>
  )
}

export function Empty({ icon, title, children, action }: { icon?: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center px-6 py-14 text-center">
      {icon && <div className="mb-4 flex size-11 items-center justify-center rounded-2xl border border-line bg-sunken text-fg-3">{icon}</div>}
      <p className="text-[15px] font-semibold">{title}</p>
      {children && <div className="mt-1.5 max-w-sm text-[13.5px] leading-relaxed text-fg-2">{children}</div>}
      {action && <div className="mt-5">{action}</div>}
    </div>
  )
}

export function SectionLabel({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="mb-2.5 flex items-center gap-2 px-0.5 md:mb-3">
      <h2 className="text-[16px] font-semibold tracking-[-0.025em] text-fg">{children}</h2>
      {right && <div className="ml-auto">{right}</div>}
    </div>
  )
}

export function Meter({ value, tone = 'fg', className }: { value: number; tone?: 'fg' | 'accent' | 'auto'; className?: string }) {
  const v = Math.max(0, Math.min(100, value))
  const color = tone === 'accent' ? 'var(--accent)' : tone === 'auto' ? (v >= 85 ? 'var(--danger)' : v >= 60 ? 'var(--warn)' : 'var(--fg)') : 'var(--fg)'
  return (
    <div className={clsx('h-1.5 overflow-hidden rounded-full bg-line', className)}>
      <div className="h-full origin-left rounded-full transition-transform duration-(--dur-move)" style={{ transform: `scaleX(${v / 100})`, background: color }} />
    </div>
  )
}
