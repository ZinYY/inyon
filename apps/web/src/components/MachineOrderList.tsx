import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Menu } from 'lucide-react'
import clsx from 'clsx'
import type { HostState } from '@inyon/protocol'
import { api } from '../lib/api.ts'
import { splitHostId } from '../lib/hubs.ts'
import { toast, useStore } from '../lib/store.ts'

const sameHub = (a: string, b: string) => splitHostId(a).hub?.id === splitHostId(b).hub?.id
const move = (ids: string[], id: string, at: number) => {
  const next = ids.filter((x) => x !== id)
  next.splice(at, 0, id)
  return next
}

/** The handle owns touch gestures; the rest of each row remains a normal link and scroll area. */
export function MachineOrderList({ hosts, allHosts, renderRow }: { hosts: HostState[]; allHosts: HostState[]; renderRow: (h: HostState, i: number) => ReactNode }) {
  const box = useRef<HTMLDivElement>(null)
  const drag = useRef<{ id: string; pointer: number; start: number; y: number; moved: boolean; ids: string[]; frame: number } | null>(null)
  const [preview, setPreview] = useState<string[] | null>(null)
  const [grabbed, setGrabbed] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [announcement, announce] = useState('')
  const ids = hosts.map((h) => h.config.id)
  const shown = preview ? [...hosts].sort((a, b) => preview.indexOf(a.config.id) - preview.indexOf(b.config.id)) : hosts
  const cancel = () => {
    if (drag.current) cancelAnimationFrame(drag.current.frame)
    drag.current = null
    setGrabbed(null)
    setPreview(null)
  }
  useEffect(() => () => { if (drag.current) cancelAnimationFrame(drag.current.frame) }, [])

  const save = async (id: string, order: string[]) => {
    // Merge a filtered view into the complete list so hidden machines are not lost.
    const visible = new Set(order)
    let i = 0
    const full = allHosts.map((h) => visible.has(h.config.id) ? order[i++]! : h.config.id)
    const before = full.slice(full.indexOf(id) + 1).find((x) => sameHub(id, x)) ?? null
    setSaving(true)
    setPreview(order)
    try {
      await api.orderHost(id, before)
      // Keep the current session data: it may have changed while this request was in flight.
      useStore.setState((s) => {
        const item = s.hosts.find((h) => h.config.id === id)
        if (!item) return {}
        const next = s.hosts.filter((h) => h !== item)
        const at = before === null ? next.findLastIndex((h) => sameHub(id, h.config.id)) + 1 : next.findIndex((h) => h.config.id === before)
        next.splice(at < 0 ? next.length : at, 0, item)
        return { hosts: next }
      })
      announce('Machine order saved')
    } catch (e) {
      toast({ title: 'Could not save machine order', body: (e as Error).message, tone: 'error' })
      announce('Machine order was not changed')
    } finally {
      setSaving(false)
      setPreview(null)
    }
  }

  const update = () => {
    const d = drag.current
    if (!d || !box.current) return
    if (!d.moved && Math.abs(d.y - d.start) < 6) return
    d.moved = true
    const rows = [...box.current.children].filter((el) => el instanceof HTMLElement && el.dataset.machineId && sameHub(d.id, el.dataset.machineId)) as HTMLElement[]
    const others = rows.filter((el) => el.dataset.machineId !== d.id)
    const at = others.filter((el) => { const r = el.getBoundingClientRect(); return d.y > r.top + r.height / 2 }).length
    const own = d.ids.filter((id) => sameHub(d.id, id))
    const reordered = move(own, d.id, at)
    let i = 0
    const next = d.ids.map((id) => sameHub(d.id, id) ? reordered[i++]! : id)
    if (next.some((id, i) => id !== d.ids[i])) { d.ids = next; setPreview(next) }
  }

  return <div ref={box} className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card" aria-busy={saving}
        onPointerMove={(e) => { if (drag.current?.pointer === e.pointerId) { drag.current.y = e.clientY; update() } }}
        onPointerUp={(e) => {
          const d = drag.current
          if (!d || d.pointer !== e.pointerId) return
          const changed = d.moved && d.ids.some((id, i) => id !== ids[i])
          cancel()
          if (changed) void save(d.id, d.ids)
        }}
        onPointerCancel={cancel} onLostPointerCapture={() => { if (drag.current) cancel() }}
    >
    {shown.map((h, i) => <div key={h.config.id} data-machine-id={h.config.id} className={clsx('flex items-center', grabbed === h.config.id && 'bg-hover ring-1 ring-inset ring-line-strong')}>
      {renderRow(h, i)}
      <button type="button" data-machine-drag aria-label={`Reorder ${h.config.name}`} aria-describedby="machine-order-help" title="Drag to reorder; use arrow keys to move" disabled={saving}
        className="mr-2 flex size-10 shrink-0 touch-none select-none items-center justify-center rounded-xl text-fg-3 hover:bg-hover hover:text-fg focus-visible:outline-2 focus-visible:outline-fg-2 disabled:opacity-40 cursor-grab active:cursor-grabbing"
        onClick={(e) => { e.preventDefault(); e.stopPropagation() }}
        onContextMenu={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          if (e.button !== 0 || saving || drag.current) return
          e.preventDefault(); e.stopPropagation(); e.currentTarget.focus()
          box.current?.setPointerCapture?.(e.pointerId)
          const d = { id: h.config.id, pointer: e.pointerId, start: e.clientY, y: e.clientY, moved: false, ids, frame: 0 }
          drag.current = d; setGrabbed(d.id)
          let scroll = box.current?.parentElement
          while (scroll && !/(auto|scroll)/.test(getComputedStyle(scroll).overflowY)) scroll = scroll.parentElement
          const tick = () => {
            if (drag.current !== d) return
            if (d.moved && scroll) {
              const r = scroll.getBoundingClientRect()
              const dy = d.y < r.top + 48 ? -10 : d.y > r.bottom - 48 ? 10 : 0
              if (dy) { scroll.scrollTop += dy; update() }
            }
            d.frame = requestAnimationFrame(tick)
          }
          d.frame = requestAnimationFrame(tick)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') { cancel(); return }
          if (saving || drag.current || !['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return
          e.preventDefault()
          const own = ids.filter((id) => sameHub(id, h.config.id))
          const from = own.indexOf(h.config.id)
          const at = e.key === 'Home' ? 0 : e.key === 'End' ? own.length - 1 : Math.max(0, Math.min(own.length - 1, from + (e.key === 'ArrowUp' ? -1 : 1)))
          if (from === at) return
          const next = move(own, h.config.id, at)
          let i = 0
          void save(h.config.id, ids.map((id) => sameHub(id, h.config.id) ? next[i++]! : id))
        }}><Menu size={18} aria-hidden="true" /></button>
    </div>)}
    <span id="machine-order-help" className="sr-only">Drag to change the order in Overview. Arrow keys also move a machine.</span>
    <span className="sr-only" role="status" aria-live="polite">{announcement}</span>
  </div>
}
