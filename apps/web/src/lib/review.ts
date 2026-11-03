import { useSyncExternalStore } from 'react'

/** A comment on one line of a diff, waiting to go to the agent with the others. */
export interface ReviewNote {
  file: string
  line: number
  /** the line was taken out (its number is in the old file) */
  removed: boolean
  code: string
  text: string
}

const notes = new Map<string, ReviewNote[]>()
const listeners = new Set<() => void>()
const EMPTY: ReviewNote[] = []
const changed = (key: string, list: ReviewNote[]) => {
  if (list.length) notes.set(key, list)
  else notes.delete(key)
  for (const l of listeners) l()
}

/** Comments for one session, kept until they're added to a message or cleared; not saved anywhere. */
export const review = {
  add: (key: string, n: ReviewNote) => changed(key, [...(notes.get(key) ?? []).filter((x) => !same(x, n)), n]),
  remove: (key: string, n: ReviewNote) => changed(key, (notes.get(key) ?? []).filter((x) => !same(x, n))),
  clear: (key: string) => changed(key, []),
  find: (key: string, file: string, line: number, removed: boolean) => notes.get(key)?.find((x) => same(x, { file, line, removed })),
}

const same = (a: Pick<ReviewNote, 'file' | 'line' | 'removed'>, b: Pick<ReviewNote, 'file' | 'line' | 'removed'>) =>
  a.file === b.file && a.line === b.line && a.removed === b.removed

export function useReview(key: string) {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => void listeners.delete(l)),
    () => notes.get(key) ?? EMPTY,
  )
}

/** The comments as one message, file by file, each under the line it's about. */
export function reviewMessage(list: ReviewNote[]) {
  const byFile = new Map<string, ReviewNote[]>()
  for (const n of list) byFile.set(n.file, [...(byFile.get(n.file) ?? []), n])
  const parts = ['Comments on your changes:']
  for (const [file, ns] of byFile) {
    for (const n of ns.sort((a, b) => a.line - b.line)) {
      parts.push(`${file}:${n.line}${n.removed ? ' (removed line)' : ''}\n> ${n.code.trim() || '(empty line)'}\n${n.text.trim()}`)
    }
  }
  return parts.join('\n\n')
}
