import { useMemo } from 'react'
import { structuredPatch } from 'diff'
import type { ToolResultMeta } from '@inyon/protocol'

type Hunk = NonNullable<ToolResultMeta['patch']>[number]

/** Hunks from the tool result when Claude Code recorded them, otherwise computed from the edit. */
export function useHunks(meta: ToolResultMeta | undefined, input: any): Hunk[] {
  return useMemo(() => {
    if (meta?.patch?.length) return meta.patch
    const edits: { old_string?: string; new_string?: string }[] =
      Array.isArray(input?.edits) ? input.edits : input?.old_string !== undefined ? [input] : []
    if (edits.length) {
      return edits.flatMap((e) => structuredPatch('a', 'b', e.old_string ?? '', e.new_string ?? '', '', '', { context: 3 }).hunks)
    }
    if (typeof input?.content === 'string') {
      const lines = input.content.split('\n')
      return [{ oldStart: 0, oldLines: 0, newStart: 1, newLines: lines.length, lines: lines.map((l: string) => `+${l}`) }]
    }
    return []
  }, [meta, input])
}

/** A diff line someone can comment on: its number (in the old file for a removed line) and its text. */
export interface DiffLine {
  no: number
  removed: boolean
  text: string
}

export function Diff({ hunks, maxLines = 400, onLine, marked }: {
  hunks: Hunk[]
  maxLines?: number
  /** makes each line number a button that comments on that line */
  onLine?: (l: DiffLine) => void
  /** lines that already have a comment */
  marked?: (l: DiffLine) => boolean
}) {
  let shown = 0
  let hidden = 0
  return (
    <div className="overflow-x-auto font-mono text-[12.5px] leading-[1.55]">
      <table className="w-full border-collapse">
        <tbody>
          {hunks.map((h, hi) => {
            let o = h.oldStart
            let n = h.newStart
            const rows = []
            if (hi > 0) {
              rows.push(
                <tr key={`gap${hi}`}>
                  <td colSpan={3} className="select-none bg-sunken/60 px-3 py-0.5 text-center text-[12px] text-fg-3">
                    ⋯
                  </td>
                </tr>,
              )
            }
            for (const [li, line] of h.lines.entries()) {
              const sign = line[0]
              // "\ No newline at end of file" is not a line of the file: it neither shows nor counts
              if (sign === '\\') continue
              if (shown >= maxLines) {
                hidden++
                continue
              }
              shown++
              const text = line.slice(1)
              const add = sign === '+'
              const del = sign === '-'
              const oldNo = add ? '' : o++
              const newNo = del ? '' : n++
              const l: DiffLine = { no: Number(del ? oldNo : newNo), removed: del, text }
              const on = marked?.(l)
              rows.push(
                <tr key={`${hi}-${li}`} className={add ? 'bg-[var(--diff-add)]' : del ? 'bg-[var(--diff-del)]' : ''}>
                  <td className="w-[1%] select-none whitespace-nowrap p-0 text-right text-fg-3/80">
                    {onLine ? (
                      <button
                        type="button"
                        onClick={() => onLine(l)}
                        title="Comment on this line"
                        aria-label={`Comment on ${l.removed ? 'removed ' : ''}line ${l.no}`}
                        className={`block w-full px-2 text-right hover:bg-accent-soft hover:text-fg ${on ? 'bg-accent-soft font-semibold text-accent-text' : ''}`}
                      >
                        {l.no}
                      </button>
                    ) : (
                      <span className="px-2">{l.no}</span>
                    )}
                  </td>
                  <td className={`w-[1%] select-none pl-1 ${add ? 'text-ok' : del ? 'text-danger' : 'text-fg-3'}`}>{add ? '+' : del ? '−' : ' '}</td>
                  <td className="whitespace-pre pr-4 pl-1.5">{text || ' '}</td>
                </tr>,
              )
            }
            return rows
          })}
        </tbody>
      </table>
      {hidden > 0 && <div className="border-t border-line px-3 py-1.5 font-sans text-xs text-fg-3">{hidden} more lines</div>}
    </div>
  )
}
