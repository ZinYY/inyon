const EVENT = 'inyon:compose'

export interface Compose {
  host: string
  pane: string
  text: string
}

/**
 * Adds text to a session's message box, where you can still change it before sending: a file
 * mention from the Files tab, or review comments from a diff. False when that chat isn't open.
 */
export const compose = (c: Compose) => !window.dispatchEvent(new CustomEvent(EVENT, { detail: c, cancelable: true }))

/** For the message box: calls `add` with text meant for this session. */
export function onCompose(host: string, pane: string, add: (text: string) => void) {
  const on = (e: Event) => {
    const c = (e as CustomEvent<Compose>).detail
    if (c.host !== host || c.pane !== pane) return
    e.preventDefault()
    add(c.text)
  }
  window.addEventListener(EVENT, on)
  return () => window.removeEventListener(EVENT, on)
}

/** `text` added after what's already in the box: on its own paragraph when either has several lines. */
export const appendTo = (draft: string, text: string) =>
  !draft.trim() ? text : `${draft.replace(/\s+$/, '')}${draft.includes('\n') || text.includes('\n') ? '\n\n' : ' '}${text}`
