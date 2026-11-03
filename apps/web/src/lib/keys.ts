/** A Mac (or an iPad with a keyboard), where shortcuts are shown with ⌘ and ⌥. */
export const isMac = () => /Mac|iPhone|iPad/.test(navigator.userAgent)

/** How a shortcut is written on this computer: `⌘K` and `⌥N` on a Mac, `Ctrl K` and `Alt N` elsewhere. */
export const keyLabel = (mod: 'cmd' | 'alt', key: string) => (isMac() ? `${mod === 'cmd' ? '⌘' : '⌥'}${key}` : `${mod === 'cmd' ? 'Ctrl' : 'Alt'} ${key}`)

const typingIn = (el: Element | null) =>
  el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName) || !!el.closest('.xterm'))

/**
 * Which app shortcut a key press is, if any. `N` alone only while nothing takes typing: in a
 * session a key with nothing focused starts a message. Alt+N (⌥N) works while typing too, except
 * in the terminal, where Alt keys belong to the shell. `code` is checked for Alt+N, since on a Mac
 * ⌥N types a dead key rather than "n".
 */
export function shortcut(e: Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'target'>, focused: Element | null = document.activeElement): 'search' | 'new' | undefined {
  const t = e.target instanceof Element ? e.target : null
  if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'k') return 'search'
  if (e.altKey && !e.metaKey && !e.ctrlKey && e.code === 'KeyN') return t?.closest('.xterm') || focused?.closest('.xterm') ? undefined : 'new'
  if (!e.metaKey && !e.ctrlKey && !e.altKey && e.key.toLowerCase() === 'n' && !typingIn(t) && !typingIn(focused)) return 'new'
}
