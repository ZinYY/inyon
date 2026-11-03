import { api } from './api.ts'
import { clearNativeNotification, nativeNotify, type NotificationAnswer } from './native.ts'
import { findPane, useStore } from './store.ts'

/**
 * How old a tap may be when it reaches the web app. Android freezes a background app, and a tap
 * waits for it to thaw; the phone gives up after 15 s and says the answer wasn't sent, so a
 * tap past this is dropped rather than sent minutes later.
 */
export const ANSWER_TTL_MS = 12_000

/**
 * Sends an answer tapped on a notification. Only while the session still asks that question with
 * that answer at that number: a tap that comes late never lands on the next question. The
 * notification goes once the agent has it, or says why it didn't.
 */
export async function answerFromNotification(a: NotificationAnswer, now = Date.now()) {
  if (a.at !== undefined && now - a.at > ANSWER_TTL_MS) return
  const href = `/m/${encodeURIComponent(a.host)}/p/${a.pane.replace(/^%/, '')}`
  const agent = findPane(useStore.getState().hosts, a.host, a.pane)?.agent
  if (agent?.state !== 'waiting' || !agent.options?.some((o) => o.n === a.option && o.label === a.label)) {
    await nativeNotify('That question changed', `“${a.label}” wasn't sent. Open the session to see what it asks now.`, href, a.tag, 'done')
    return
  }
  try {
    await api.answer(a.host, a.pane, a.option)
    clearNativeNotification(a.tag)
  } catch (e) {
    await nativeNotify("Couldn't send your answer", (e as Error).message, href, a.tag, 'waiting')
  }
}
