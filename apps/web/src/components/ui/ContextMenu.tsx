import { useEffect, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, ArrowRight, ClipboardPaste, Copy, ExternalLink, Link2, RotateCw, Scissors, TextSelect } from 'lucide-react'
import { MenuPopup, type MenuItem } from './primitives.tsx'
import { useBackCloses } from '../../lib/back.ts'
import { copyText } from '../../lib/clipboard.ts'
import { keyLabel } from '../../lib/keys.ts'
import { isPhone } from '../../lib/phone.ts'
import { toast } from '../../lib/store.ts'

/**
 * Right-click menus. Anything can offer one with `contextMenu()`; it opens where the pointer is,
 * with the same look and keys as the ⋯ menus. Everywhere else the app's own menu replaces the
 * browser's: Cut, Copy and Paste in a text box, Open and Copy link on a link, and a short page
 * menu on anything else. Shift+right-click still opens the browser's menu.
 */

export type MenuItems = (MenuItem | false | null | undefined | '' | 0)[]
type Open = { x: number; y: number; items: MenuItem[]; header?: ReactNode }

let show: ((m: Open | undefined) => void) | undefined
/** how the last press was made: a long press on a touch screen also sends contextmenu */
let pointer = 'mouse'

export function openContextMenu(at: { x: number; y: number }, items: MenuItems, header?: ReactNode) {
  const list = items.filter((i): i is MenuItem => !!i)
  if (!list.length || !show) return false
  show({ x: at.x, y: at.y, items: list, header })
  return true
}

/** An item that copies `text`, saying so (`what` names it: "Session ID copied"). */
export const copyItem = (key: string, label: string, text: string, what: string, icon: ReactNode = <Copy className="size-4" />): MenuItem => ({
  key,
  label,
  icon,
  onSelect: async () => {
    if (await copyText(text)) toast({ title: `${what} copied`, tone: 'info' })
    else toast({ title: 'Could not copy', tone: 'error' })
  },
})

/** What the page menu offers besides Back and Reload: set by the app shell (search, new session). */
let pageItems: () => MenuItems = () => []
export function usePageMenu(items: () => MenuItems) {
  const latest = useRef(items)
  latest.current = items
  useEffect(() => {
    pageItems = () => latest.current()
    return () => void (pageItems = () => [])
  }, [])
}

const icon = (I: typeof Copy) => <I className="size-4" />

type TextBox = HTMLInputElement | HTMLTextAreaElement
const isTextBox = (el: Element): el is TextBox =>
  el instanceof HTMLTextAreaElement || (el instanceof HTMLInputElement && /^(text|search|url|email|tel|password|number|)$/.test(el.type))

/** Cut, Copy, Paste and Select all for the text box that was right-clicked. */
function textBoxItems(el: TextBox | HTMLElement): MenuItems {
  const box = isTextBox(el) ? el : undefined
  // where the selection was when the menu opened: picking an item moves focus to the menu
  const start = box?.selectionStart ?? 0
  const end = box?.selectionEnd ?? 0
  const picked = box ? box.value.slice(start, end) : (window.getSelection()?.toString() ?? '')
  const editable = box ? !box.readOnly && !box.disabled : el.isContentEditable
  const secret = box instanceof HTMLInputElement && box.type === 'password'
  const restore = () => {
    el.focus()
    if (box) box.setSelectionRange(start, end)
  }
  // insertText keeps the box's undo history, and React sees the change
  const insert = (text: string) => {
    restore()
    if (!document.execCommand('insertText', false, text) && box) box.setRangeText(text, start, end, 'end')
  }
  return [
    editable && { key: 'cut', label: 'Cut', icon: icon(Scissors), keys: keyLabel('cmd', 'X'), disabled: !picked || secret, onSelect: () => void copyText(picked).then((ok) => ok && insert('')) },
    { key: 'copy', label: 'Copy', icon: icon(Copy), keys: keyLabel('cmd', 'C'), disabled: !picked || secret, onSelect: () => void copyText(picked) },
    editable && {
      key: 'paste',
      label: 'Paste',
      icon: icon(ClipboardPaste),
      keys: keyLabel('cmd', 'V'),
      onSelect: async () => {
        try {
          insert(await navigator.clipboard.readText())
        } catch {
          // the browser won't let a page read the clipboard: the keys still work
          restore()
          toast({ title: `Press ${keyLabel('cmd', 'V')} to paste`, body: "This browser doesn't let the menu read the clipboard.", tone: 'info' })
        }
      },
    },
    {
      key: 'all',
      label: 'Select all',
      icon: icon(TextSelect),
      keys: keyLabel('cmd', 'A'),
      separated: true,
      onSelect: () => {
        el.focus()
        if (box) box.select()
        else document.execCommand('selectAll')
      },
    },
  ]
}

function linkItems(a: HTMLAnchorElement): MenuItems {
  const outside = a.origin !== location.origin
  return [
    { key: 'open', label: 'Open link', icon: icon(outside ? ExternalLink : ArrowRight), onSelect: () => (outside ? void window.open(a.href, '_blank', 'noopener') : a.click()) },
    !outside && !isPhone() && { key: 'tab', label: 'Open in new tab', icon: icon(ExternalLink), onSelect: () => void window.open(a.href, '_blank', 'noopener') },
    copyItem('copy-link', 'Copy link', a.href, 'Link', icon(Link2)),
  ]
}

/** The page's own menu, for a right-click on nothing in particular. */
function pageMenu(): MenuItems {
  return [
    history.length > 1 && { key: 'back', label: 'Back', icon: icon(ArrowLeft), onSelect: () => history.back() },
    { key: 'reload', label: 'Reload', icon: icon(RotateCw), onSelect: () => location.reload() },
    ...pageItems().map((it, i) => (it && i === 0 ? { ...it, separated: true } : it)),
  ]
}

/** The menu for a right-click that nothing on the page claimed. */
export function fallbackMenu(target: EventTarget | null): MenuItems {
  const el = target instanceof Element ? target : null
  const box = el?.closest('input, textarea, [contenteditable=""], [contenteditable="true"]') as TextBox | HTMLElement | null
  if (box && (isTextBox(box) || box.isContentEditable)) return textBoxItems(box)
  const sel = window.getSelection()?.toString() ?? ''
  const copy: MenuItems = sel.trim() ? [{ key: 'copy-selection', label: 'Copy', icon: icon(Copy), keys: keyLabel('cmd', 'C'), onSelect: () => void copyText(sel) }] : []
  const a = el?.closest('a[href]') as HTMLAnchorElement | null
  if (a) return [...copy, ...linkItems(a).map((it, i) => (it && i === 0 && copy.length ? { ...it, separated: true } : it))]
  const page = pageMenu()
  return copy.length ? [...copy, ...page.map((it, i) => (it && i === 0 ? { ...it, separated: true } : it))] : page
}

/** The one place right-click menus show. Mounted once, around everything. */
export function ContextMenuHost() {
  const [open, setOpen] = useState<Open>()
  useBackCloses(!!open, () => setOpen(undefined))
  useEffect(() => {
    show = setOpen
    const down = (e: PointerEvent) => void (pointer = e.pointerType)
    // runs after the page's own handlers: what they didn't claim gets the app's menu, not the browser's
    const fallback = (e: MouseEvent) => {
      if (e.defaultPrevented || e.shiftKey || pointer === 'touch') return
      if (e.target instanceof Element && e.target.closest('[data-native-menu]')) return
      e.preventDefault()
      // the keyboard's menu key gives no position: open at the focused element
      let at = { x: e.clientX, y: e.clientY }
      if (!at.x && !at.y && e.target instanceof Element) {
        const r = e.target.getBoundingClientRect()
        at = { x: r.left + 12, y: r.top + Math.min(r.height, 24) }
      }
      openContextMenu(at, fallbackMenu(e.target))
    }
    document.addEventListener('pointerdown', down, true)
    document.addEventListener('contextmenu', fallback)
    return () => {
      show = undefined
      document.removeEventListener('pointerdown', down, true)
      document.removeEventListener('contextmenu', fallback)
    }
  }, [])
  if (!open) return null
  return (
    <MenuPopup
      key={`${open.x},${open.y}`}
      anchor={() => new DOMRect(open.x, open.y, 0, 0)}
      items={open.items}
      header={open.header}
      width={240}
      gap={2}
      onClose={() => setOpen(undefined)}
      onOutsideContextMenu={(e) => {
        // right-clicking something else while a menu is open opens that thing's menu instead
        const { clientX: x, clientY: y } = e
        setOpen(undefined)
        requestAnimationFrame(() => document.elementFromPoint(x, y)?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: x, clientY: y })))
      }}
    />
  )
}

/** Where the right-click belongs to what's under the pointer, not the row around it: the text box and link menus. */
function ownMenuFor(t: EventTarget | null) {
  if (!(t instanceof Element)) return false
  return !!t.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"], a[href^="http"], [data-native-menu]')
}

/** The selected text, when it lies inside `el`. */
function selectionIn(el: Element) {
  const sel = window.getSelection()
  const text = sel?.toString() ?? ''
  if (!text.trim() || !sel?.rangeCount) return ''
  const r = sel.getRangeAt(0)
  return el.contains(r.commonAncestorContainer) ? text : ''
}

/**
 * Props that give an element a right-click menu; a plain function, so fine inside a list. `items`
 * is built when it opens, from the event (a click inside a code block can offer to copy it).
 * Selected text inside the element puts Copy first. A long press on a touch screen keeps doing
 * what it did unless `touch` is set.
 */
export function contextMenu(items: (e: React.MouseEvent) => MenuItems, opts: { header?: ReactNode; touch?: boolean; ownsTextBox?: boolean } = {}) {
  return {
    onContextMenu: (e: React.MouseEvent) => {
      // ownsTextBox: the terminal moves its hidden text box under the pointer on a right-click
      if (e.defaultPrevented || e.shiftKey || (!opts.ownsTextBox && ownMenuFor(e.target))) return
      if (pointer === 'touch' && !opts.touch) return
      const picked = selectionIn(e.currentTarget)
      const list = items(e).filter((i): i is MenuItem => !!i)
      const all: MenuItems = picked
        ? [{ key: 'copy-selection', label: 'Copy', icon: <Copy className="size-4" />, onSelect: () => void copyText(picked) }, ...list.map((it, i) => (i === 0 ? { ...it, separated: true } : it))]
        : list
      if (!openContextMenu({ x: e.clientX, y: e.clientY }, all, opts.header)) return
      e.preventDefault()
      e.stopPropagation()
    },
  }
}
