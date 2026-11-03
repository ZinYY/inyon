import { useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import clsx from 'clsx'
import { AlertCircle, CheckCircle2, Info, MessageCircleQuestion, X } from 'lucide-react'
import { useStore, type Toast } from '../lib/store.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { AnimatedList } from './ui/AnimatedList.tsx'

const icons = { info: Info, success: CheckCircle2, warn: MessageCircleQuestion, error: AlertCircle }
const tones = { info: 'text-fg-2', success: 'text-fg', warn: 'text-warn', error: 'text-danger' }
/** how far a toast has to be flicked sideways or up before it goes */
const FLING = 70

export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  // a stack: a new one drops in at the top, one that goes (dismissed or timed out) fades where it
  // was while the others glide into its place. One column, as wide as a toast, at the right on a
  // wide screen
  return (
    <AnimatedList
      enter="animate-toast-in"
      className="pt-safe pointer-events-none fixed inset-x-0 top-0 z-[60] grid grid-cols-[minmax(0,24rem)] justify-center gap-2 px-3 pt-3 sm:justify-end sm:px-5"
      items={toasts.map((t) => ({ key: String(t.id), node: <ToastCard t={t} /> }))}
    />
  )
}

function ToastCard({ t }: { t: Toast }) {
  const dismiss = useStore((s) => s.dismiss)
  const nav = useNavigate()
  const Icon = icons[t.tone]
  // swipe it away: follows the finger, then flies off or springs back
  const start = useRef<{ x: number; y: number } | null>(null)
  const moved = useRef(false)
  const [drag, setDrag] = useState({ x: 0, y: 0 })
  const [leaving, setLeaving] = useState<{ x: number; y: number } | null>(null)
  // it flies while the list fades it out
  const go = (x: number, y: number) => {
    setLeaving({ x, y })
    dismiss(t.id)
  }
  const offset = leaving ?? drag
  return (
    <div
      role="status"
      onPointerDown={(e) => {
        if (e.pointerType !== 'touch') return
        start.current = { x: e.clientX, y: e.clientY }
        moved.current = false
      }}
      onPointerMove={(e) => {
        if (!start.current) return
        const x = e.clientX - start.current.x
        const y = Math.min(0, e.clientY - start.current.y)
        if (Math.abs(x) > 6 || y < -6) moved.current = true
        setDrag({ x, y })
      }}
      onPointerUp={() => {
        if (!start.current) return
        start.current = null
        if (Math.abs(drag.x) > FLING) go(Math.sign(drag.x) * 420, 0)
        else if (drag.y < -FLING / 2) go(0, -160)
        else setDrag({ x: 0, y: 0 })
      }}
      onPointerCancel={() => ((start.current = null), setDrag({ x: 0, y: 0 }))}
      onClick={() => {
        if (moved.current) return
        if (t.href) nav(t.href)
        dismiss(t.id)
      }}
      style={{
        transform: offset.x || offset.y ? `translate(${offset.x}px, ${offset.y}px)` : undefined,
        opacity: leaving ? 0 : 1 - Math.min(0.6, Math.abs(offset.x) / 300),
        transition: start.current
          ? 'none'
          : leaving
            ? 'transform var(--dur-exit) ease-in, opacity var(--dur-exit) ease-in'
            : 'transform var(--dur-move) var(--ease-spring), opacity var(--dur-small) var(--ease-out-soft)',
        touchAction: 'pan-y',
      }}
      className={clsx(
        'pointer-events-auto flex w-full items-start gap-3 rounded-2xl border border-line bg-elev px-3.5 py-3 shadow-float select-none',
        t.href && 'cursor-pointer',
      )}
    >
      {t.agent ? <AgentLogo kind={t.agent as any} size={22} /> : <Icon className={clsx('mt-0.5 size-[18px] shrink-0', tones[t.tone])} />}
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-semibold">{t.title}</p>
        {t.body && <p className="mt-0.5 line-clamp-2 text-[13px] leading-snug text-fg-2">{t.body}</p>}
      </div>
      {t.action && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            t.action!.onSelect()
            go(0, -40)
          }}
          className="press -my-0.5 shrink-0 rounded-full border border-line px-3 py-1 text-[13px] font-medium text-fg hover:bg-hover"
        >
          {t.action.label}
        </button>
      )}
      <button
        type="button"
        aria-label="Dismiss"
        onClick={(e) => {
          e.stopPropagation()
          go(0, -40)
        }}
        className="press -mr-1 rounded-md p-0.5 text-fg-3 hover:text-fg"
      >
        <X className="size-4" />
      </button>
    </div>
  )
}
