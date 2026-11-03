import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import clsx from 'clsx'
import { Check, CheckSquare, Eye, FileText, FolderOpen, ImageIcon, RotateCw, Trash2, X } from 'lucide-react'
import type { HostState } from '@inyon/protocol'
import { api, ApiError, type TempFile, type TempItem, type TempListing, type TempPreview } from '../lib/api.ts'
import { toast, useStore } from '../lib/store.ts'
import { basename, bytes, timeAgo } from '../lib/format.ts'
import { Button, Empty, IconButton, Sheet, Waiting } from '../components/ui/primitives.tsx'
import { Select } from '../components/ui/Select.tsx'
import { contextMenu, copyItem, type MenuItems } from '../components/ui/ContextMenu.tsx'
import { confirmAction } from '../components/Confirm.tsx'
import { Logo } from '../components/AppShell.tsx'
import { PullRefresh } from '../components/ui/PullRefresh.tsx'
import { isPhone } from '../lib/phone.ts'

/**
 * The temp files agents leave on a machine (pasted images, screenshots, scratchpads), listed by the
 * machine's own `inyon temp` through the hub: see them, preview them, delete them. Deleting is
 * permanent, so the confirm says how much goes and warns about anything an agent may still use.
 */

const WEEK = 7 * 86_400_000
/** Thumbnails are small; a bigger image shows a placeholder until it's opened. */
export const THUMB_MAX = 1_500_000

// ---------- thumbnails: fetched when on screen, a few at a time, kept for the session ----------

const thumbs = new Map<string, Promise<string | null>>()
const waiting: (() => void)[] = []
let running = 0
const THUMBS_AT_ONCE = 3

function slot<T>(fn: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const go = () => {
      running++
      fn().then(resolve, reject).finally(() => {
        running--
        waiting.shift()?.()
      })
    }
    if (running < THUMBS_AT_ONCE) go()
    else waiting.push(go)
  })
}

export const dataUrl = (p: Pick<TempPreview, 'mime' | 'base64'>) => (p.base64 ? `data:${p.mime};base64,${p.base64}` : null)

function thumb(host: string, path: string) {
  const key = `${host}\0${path}`
  let t = thumbs.get(key)
  if (!t) {
    t = slot(() => api.tempPreview(host, path, THUMB_MAX)).then((p) => (p?.mime?.startsWith('image/') ? dataUrl(p) : null), () => null)
    thumbs.set(key, t)
  }
  return t
}

/** Forgets thumbnails of what was deleted. */
const forgetThumbs = (host: string, paths: string[]) => {
  for (const k of [...thumbs.keys()]) if (k.startsWith(`${host}\0`) && paths.some((p) => k.slice(host.length + 1).startsWith(p))) thumbs.delete(k)
}

function Thumb({ host, path, className }: { host: string; path: string; className?: string }) {
  const el = useRef<HTMLSpanElement>(null)
  const [src, setSrc] = useState<string | null>(null)
  const [seen, setSeen] = useState(typeof IntersectionObserver === 'undefined')
  useEffect(() => {
    if (seen || !el.current) return
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setSeen(true), { rootMargin: '200px' })
    io.observe(el.current)
    return () => io.disconnect()
  }, [seen])
  useEffect(() => {
    if (!seen) return
    let live = true
    void thumb(host, path).then((s) => live && setSrc(s))
    return () => void (live = false)
  }, [seen, host, path])
  return (
    <span ref={el} className={clsx('flex shrink-0 items-center justify-center overflow-hidden rounded-lg border border-line bg-sunken text-fg-3', className ?? 'size-9')}>
      {src ? <img src={src} alt="" className="size-full object-cover" /> : <ImageIcon className="size-3.5" />}
    </span>
  )
}

// ---------- the screen ----------

function Header({ right }: { right?: React.ReactNode }) {
  return (
    <header className="pt-safe sticky top-0 z-20 border-b border-line bg-bg">
      <div className="flex h-[52px] items-center gap-2 px-3 lg:px-7">
        <span className="md:hidden">
          <Logo />
        </span>
        <h1 className="min-w-0 flex-1 truncate px-1 text-[15px] font-semibold tracking-tight">Temp files</h1>
        {right}
      </div>
    </header>
  )
}

/** `/temp` and `/temp/:host`: one machine's temp files, with a picker when there are several. */
export function TempPage() {
  const param = useParams().host
  const hosts = useStore((s) => s.hosts)
  const nav = useNavigate()
  const id = param ? decodeURIComponent(param) : (hosts.find((h) => h.status === 'online') ?? hosts[0])?.config.id
  const host = hosts.find((h) => h.config.id === id)
  const reload = useRef<() => Promise<unknown>>(async () => {})
  return (
    <PullRefresh onRefresh={() => reload.current()} className="min-h-0 flex-1 overflow-y-auto">
      <Header right={host && <IconButton label="Refresh" onClick={() => void reload.current()}><RotateCw className="size-4" /></IconButton>} />
      <div className="mx-auto max-w-[880px] px-4 py-5 lg:px-7">
        {hosts.length > 1 && (
          <div className="mb-4 sm:max-w-64">
            <Select
              aria-label={isPhone() ? 'Computer' : 'Machine'}
              title={isPhone() ? 'Computer' : 'Machine'}
              value={id ?? ''}
              onChange={(v) => nav(`/temp/${encodeURIComponent(v)}`, { replace: true })}
              options={hosts.map((h) => ({ value: h.config.id, label: h.config.name, hint: h.status === 'online' ? undefined : 'offline' }))}
            />
          </div>
        )}
        {!hosts.length ? (
          <Waiting shape="rows" label={`Loading ${isPhone() ? 'computers' : 'machines'}`} />
        ) : host ? (
          <TempFiles key={host.config.id} host={host} reloadRef={reload} />
        ) : (
          <Empty title={`No ${isPhone() ? 'computer' : 'machine'} called ${id}`} />
        )}
      </div>
    </PullRefresh>
  )
}

type Group = { id: string; label: string; size: number; items: TempItem[] }

/** One machine's temp files: what the machine page's Temp files tab and `/temp` show. */
export function TempFiles({ host, reloadRef }: { host: HostState; reloadRef?: React.MutableRefObject<() => Promise<unknown>> }) {
  const id = host.config.id
  const online = host.status === 'online'
  const [list, setList] = useState<TempListing | null>(null)
  const [error, setError] = useState<{ status: number; message: string } | null>(null)
  const [picked, setPicked] = useState<Set<string>>(() => new Set())
  const [open, setOpen] = useState<TempItem | null>(null)
  const last = useRef<number | null>(null)

  const load = useCallback(async () => {
    try {
      const l = await api.tempList(id)
      setList({ ...l, items: l.items ?? [], roots: l.roots ?? [] })
      setError(null)
      // what went away can't stay selected
      setPicked((p) => new Set([...p].filter((x) => l.items?.some((i) => i.path === x))))
    } catch (e) {
      setError({ status: e instanceof ApiError ? e.status : 0, message: (e as Error).message })
    }
  }, [id])
  if (reloadRef) reloadRef.current = load
  useEffect(() => {
    if (online) void load()
  }, [online, load])

  const groups = useMemo<Group[]>(() => {
    if (!list) return []
    return list.roots
      .filter((r) => r.exists && list.items.some((i) => i.root === r.id))
      .map((r) => ({ id: r.id, label: r.label, size: r.size, items: list.items.filter((i) => i.root === r.id) }))
  }, [list])
  // the order on screen, for shift-click ranges
  const order = useMemo(() => groups.flatMap((g) => g.items), [groups])
  const chosen = order.filter((i) => picked.has(i.path))
  const total = list?.items.reduce((n, i) => n + i.size, 0) ?? 0

  const toggle = (item: TempItem, range: boolean) => {
    const at = order.indexOf(item)
    // read now: the updater below runs later, after `last` has moved on
    const from = last.current
    setPicked((p) => {
      const next = new Set(p)
      if (range && from !== null && order[from]) {
        const [a, b] = [Math.min(from, at), Math.max(from, at)]
        for (const i of order.slice(a, b + 1)) next.add(i.path)
      } else if (next.has(item.path)) next.delete(item.path)
      else next.add(item.path)
      return next
    })
    last.current = at
  }
  const selectOld = () => {
    const old = order.filter((i) => !i.recent && Date.now() - i.modified > WEEK)
    if (!old.length) return toast({ title: 'Nothing older than a week', tone: 'info' })
    setPicked(new Set(old.map((i) => i.path)))
  }

  /** Asks, deletes, says what was freed, and lists again. */
  const remove = async (items: TempItem[]) => {
    if (!(await confirmDelete(items, host.config.name))) return
    await deletePaths(id, items.map((i) => i.path))
    setPicked((p) => new Set([...p].filter((x) => !items.some((i) => i.path === x))))
    if (open && items.some((i) => i.path === open.path)) setOpen(null)
    await load()
  }

  const itemMenu = (item: TempItem): MenuItems => [
    { key: 'open', label: 'Open', icon: <FolderOpen className="size-4" />, onSelect: () => setOpen(item) },
    { key: 'select', label: picked.has(item.path) ? 'Deselect' : 'Select', icon: <CheckSquare className="size-4" />, onSelect: () => toggle(item, false) },
    copyItem('copy', 'Copy path', item.path, 'Path'),
    { key: 'delete', label: 'Delete…', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void remove([item]) },
  ]

  if (!online) return <Empty title="Not connected">Temp files show once the {isPhone() ? 'computer' : 'machine'} is reachable.</Empty>
  if (error && !list)
    return (
      <Empty
        icon={<FileText className="size-5" />}
        title={error.status === 409 ? error.message : "Couldn't list temp files"}
        action={<Button size="sm" onClick={() => void load()}><RotateCw className="size-3.5" /> Try again</Button>}
      >
        {error.status === 409 ? undefined : error.message}
      </Empty>
    )
  if (!list) return <Waiting shape="rows" label={`Looking through temp files on ${host.config.name}`} />
  if (!order.length) return <Empty icon={<FileText className="size-5" />} title="Nothing here">Agents' temp files show up here as they make them.</Empty>

  return (
    <div className="pb-4">
      <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-2">
        <p className="tabular flex-1 text-[13px] whitespace-nowrap text-fg-2">
          {bytes(total)} in {order.length} item{order.length === 1 ? '' : 's'}
        </p>
        <div className="flex flex-wrap gap-1">
          <Button size="xs" variant="ghost" onClick={() => setPicked(new Set(order.map((i) => i.path)))}>Select all</Button>
          <Button size="xs" variant="ghost" onClick={selectOld}>Older than a week</Button>
          {picked.size > 0 && <Button size="xs" variant="ghost" onClick={() => setPicked(new Set())}>Clear</Button>}
        </div>
      </div>
      <div className="space-y-5">
        {groups.map((g) => (
          <section key={g.id} aria-label={g.label}>
            <div className="mb-2 flex items-baseline gap-2 px-1">
              <h2 className="min-w-0 flex-1 truncate text-[13.5px] font-semibold">{g.label}</h2>
              <span className="tabular shrink-0 font-mono text-[12px] text-fg-3">{bytes(g.size)} · {g.items.length}</span>
            </div>
            <div className="divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
              {g.items.map((item) => (
                <ItemRow key={item.path} host={id} item={item} on={picked.has(item.path)} onToggle={(range) => toggle(item, range)} onOpen={() => setOpen(item)} menu={() => itemMenu(item)} />
              ))}
            </div>
          </section>
        ))}
      </div>
      {chosen.length > 0 && (
        <div className="pointer-events-none sticky bottom-3 z-10 mt-4 flex justify-center">
          <div role="region" aria-label="Selection" className="animate-slide-up pointer-events-auto flex w-full max-w-md items-center gap-2 rounded-full border border-line bg-elev py-1.5 pr-1.5 pl-4 shadow-float">
            <span className="tabular min-w-0 flex-1 truncate text-[13px] font-medium">
              {chosen.length} selected · {bytes(chosen.reduce((n, i) => n + i.size, 0))}
            </span>
            <IconButton label="Clear selection" onClick={() => setPicked(new Set())}><X className="size-4" /></IconButton>
            <Button size="sm" variant="danger" onClick={() => void remove(chosen)}><Trash2 className="size-3.5" /> Delete</Button>
          </div>
        </div>
      )}
      <ItemSheet host={host} item={open} onClose={() => setOpen(null)} onChanged={load} />
    </div>
  )
}

function ItemRow({ host, item, on, onToggle, onOpen, menu }: { host: string; item: TempItem; on: boolean; onToggle: (range: boolean) => void; onOpen: () => void; menu: () => MenuItems }) {
  return (
    <div {...contextMenu(menu, { touch: true })} className={clsx('flex items-center gap-1 pr-3 pl-1', on && 'bg-hover/60')}>
      <button
        type="button"
        role="checkbox"
        aria-checked={on}
        aria-label={`Select ${item.name}`}
        onClick={(e) => onToggle(e.shiftKey)}
        className="press flex size-11 shrink-0 items-center justify-center rounded-full"
      >
        <span className={clsx('flex size-[18px] items-center justify-center rounded-md border transition-colors', on ? 'border-fg bg-fg text-bg' : 'border-line-strong bg-elev')}>
          {on && <Check className="size-3" strokeWidth={3} />}
        </span>
      </button>
      <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-3 py-2.5 text-left">
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="truncate text-[14px] font-medium">{item.name}</span>
            {item.recent && <span title="Changed in the last 30 minutes" className="inline-flex h-5 shrink-0 items-center rounded-full border border-line bg-white/45 px-2 font-mono text-[12px] text-fg-2 dark:bg-white/[0.03]">recent</span>}
          </span>
          <span className="tabular block truncate font-mono text-[12px] text-fg-3">
            {bytes(item.size)}
            {item.kind === 'dir' && ` · ${item.files} file${item.files === 1 ? '' : 's'}`} · {timeAgo(item.modified)}
          </span>
        </span>
        {item.images.length > 0 && (
          <span className="flex shrink-0 gap-1" aria-label={`${item.images.length} image${item.images.length === 1 ? '' : 's'}`}>
            {item.images.slice(0, 4).map((p, i) => (
              <Thumb key={p} host={host} path={p} className={clsx('size-9', i >= 2 && 'max-sm:hidden')} />
            ))}
          </span>
        )}
      </button>
    </div>
  )
}

// ---------- deleting ----------

/** The question before deleting: how much goes, that it's for good, and whether an agent may still use it. */
export function deleteQuestion(items: Pick<TempItem, 'name' | 'size' | 'recent'>[], machine: string) {
  const size = items.reduce((n, i) => n + i.size, 0)
  const recent = items.filter((i) => i.recent).length
  const one = items.length === 1
  let body = `Frees ${bytes(size)} on ${machine}. This can't be undone.`
  if (recent) body += one ? ' It changed in the last 30 minutes, so an agent may still be using it.' : ` ${recent} of them changed in the last 30 minutes, so an agent may still be using them.`
  return { title: one ? `Delete ${items[0]!.name}?` : `Delete ${items.length} items?`, body, yes: 'Delete', danger: true }
}

const confirmDelete = (items: Pick<TempItem, 'name' | 'size' | 'recent'>[], machine: string) => confirmAction(deleteQuestion(items, machine))

async function deletePaths(host: string, paths: string[]) {
  try {
    const r = await api.tempDelete(host, paths)
    forgetThumbs(host, r.removed ?? [])
    if (r.removed?.length) toast({ title: `Freed ${bytes(r.freed ?? 0)}`, tone: 'success' })
    const failed = r.failed ?? []
    if (failed.length)
      toast({ title: `Couldn't delete ${failed.length === 1 ? basename(failed[0]![0]) : `${failed.length} items`}`, body: failed.slice(0, 3).map(([p, why]) => (failed.length === 1 ? why : `${basename(p)}: ${why}`)).join('\n'), tone: 'error' })
  } catch (e) {
    toast({ title: "Couldn't delete", body: (e as Error).message, tone: 'error' })
  }
}

// ---------- one item's files, and a file's preview ----------

function ItemSheet({ host, item, onClose, onChanged }: { host: HostState; item: TempItem | null; onClose: () => void; onChanged: () => Promise<unknown> }) {
  const id = host.config.id
  // kept while the sheet slides away
  const [shown, setShown] = useState<TempItem | null>(item)
  const [files, setFiles] = useState<{ files: TempFile[]; truncated: boolean } | null>(null)
  const [error, setError] = useState('')
  const [preview, setPreview] = useState<TempFile | null>(null)
  const path = item?.path
  // each opening, closing and list request has its own number, so a slow answer can't fill in
  // the list after a newer one: an item opened before, or a refresh overtaken by the next delete
  const opening = useRef(0)
  const load = useCallback(async (p: string) => {
    const mine = ++opening.current
    try {
      const r = await api.tempFiles(id, p)
      if (mine !== opening.current) return
      setFiles({ files: r.files ?? [], truncated: !!r.truncated })
      setError('')
    } catch (e) {
      if (mine === opening.current) setError((e as Error).message)
    }
  }, [id])
  useEffect(() => {
    opening.current++
    if (!item) return
    setShown(item)
    setFiles(null)
    setError('')
    if (item.kind === 'dir') void load(item.path)
  }, [path])
  // a single file has nothing inside: it previews straight away
  const single = item?.kind === 'file' ? { path: item.path, rel: item.name, size: item.size, modified: item.modified, kind: 'other' as const } : null

  const removeFile = async (f: TempFile) => {
    if (!(await confirmDelete([{ name: f.rel, size: f.size, recent: Date.now() - f.modified < 30 * 60_000 }], host.config.name))) return
    await deletePaths(id, [f.path])
    if (preview?.path === f.path) setPreview(null)
    if (shown?.kind === 'file') onClose()
    else if (shown) await load(shown.path)
    await onChanged()
  }
  const fileMenu = (f: TempFile): MenuItems => [
    { key: 'preview', label: 'Preview', icon: <Eye className="size-4" />, onSelect: () => setPreview(f) },
    copyItem('copy', 'Copy path', f.path, 'Path'),
    { key: 'delete', label: 'Delete…', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void removeFile(f) },
  ]

  return (
    <>
      <Sheet open={!!item && !single} onClose={onClose} title={shown?.name ?? ''} wide>
        {shown && <p className="-mt-1 mb-3 truncate font-mono text-[12px] text-fg-3" title={shown.path}>{shown.path}</p>}
        {error ? (
          <p className="py-6 text-center text-[13px] text-danger">{error}</p>
        ) : !files ? (
          <Waiting shape="rows" label="Listing files" />
        ) : !files.files.length ? (
          <p className="py-6 text-center text-[13px] text-fg-3">This folder is empty.</p>
        ) : (
          <div className="-mx-2">
            {files.files.map((f) => (
              <button
                key={f.path}
                type="button"
                {...contextMenu(() => fileMenu(f), { touch: true })}
                onClick={() => setPreview(f)}
                className="flex min-h-11 w-full items-center gap-3 rounded-2xl px-2 py-1.5 text-left hover:bg-hover/60"
              >
                {f.kind === 'image' ? (
                  <Thumb host={id} path={f.path} className="size-10" />
                ) : (
                  <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-line bg-sunken text-fg-3"><FileText className="size-4" /></span>
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13.5px]">{f.rel}</span>
                  <span className="tabular block truncate font-mono text-[12px] text-fg-3">{bytes(f.size)} · {timeAgo(f.modified)}</span>
                </span>
              </button>
            ))}
            {files.truncated && <p className="px-2 pt-2 text-[12px] text-fg-3">Showing the newest {files.files.length} files.</p>}
          </div>
        )}
      </Sheet>
      <PreviewSheet host={id} file={single ?? preview} onClose={single ? onClose : () => setPreview(null)} onDelete={single ? undefined : removeFile} />
    </>
  )
}

function PreviewSheet({ host, file, onClose, onDelete }: { host: string; file: TempFile | null; onClose: () => void; onDelete?: (f: TempFile) => void }) {
  const [shown, setShown] = useState<TempFile | null>(file)
  const [p, setP] = useState<TempPreview | null>(null)
  const [error, setError] = useState('')
  useEffect(() => {
    if (!file) return
    setShown(file)
    setP(null)
    setError('')
    let live = true
    api.tempPreview(host, file.path).then((r) => live && setP(r), (e) => live && setError((e as Error).message))
    return () => void (live = false)
  }, [file?.path])
  const src = p && p.mime?.startsWith('image/') ? dataUrl(p) : null
  return (
    <Sheet
      open={!!file}
      onClose={onClose}
      title={shown ? basename(shown.rel) : ''}
      wide
      footer={
        onDelete && shown && (
          <div className="flex items-center gap-2">
            <span className="tabular min-w-0 flex-1 truncate font-mono text-[12px] text-fg-3">{bytes(shown.size)} · {timeAgo(shown.modified)}</span>
            <Button size="sm" variant="danger" onClick={() => onDelete(shown)}><Trash2 className="size-3.5" /> Delete</Button>
          </div>
        )
      }
    >
      {error ? (
        <p className="py-6 text-center text-[13px] text-danger">{error}</p>
      ) : !p ? (
        <Waiting shape="panel" label="Opening" />
      ) : src ? (
        <img src={src} alt={shown?.rel ?? ''} className="mx-auto max-h-[70dvh] w-auto max-w-full rounded-xl border border-line object-contain" />
      ) : typeof p.text === 'string' ? (
        <>
          {/* shown as text, never as a page or markdown: an agent's HTML stays inert here */}
          <pre className="max-h-[70dvh] overflow-auto rounded-xl border border-line bg-code px-3.5 py-3 font-mono text-[12.5px] leading-[1.6] break-words whitespace-pre-wrap text-fg">{p.text}</pre>
          {p.truncated && <p className="mt-2 text-[12px] text-fg-3">Truncated: showing the first 256 KB of {bytes(p.size)}.</p>}
        </>
      ) : (
        <Empty icon={<FileText className="size-5" />} title={p.truncated && p.mime?.startsWith('image/') ? 'Too big to preview' : 'No preview for this kind of file'}>
          <span className="font-mono text-[12px]">{p.mime || 'unknown type'} · {bytes(p.size)}</span>
        </Empty>
      )}
    </Sheet>
  )
}

/** For the tests: the thumbnail cache starts empty. */
export const resetThumbs = () => thumbs.clear()
