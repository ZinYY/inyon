import { useEffect, useRef, useState } from 'react'
import clsx from 'clsx'
import { ArrowDown, ArrowRight, ArrowUp, Check, Copy, CornerDownLeft, ExternalLink, Send } from 'lucide-react'
import { CONFIRM_OPTION, promptKey, type ScreenPrompt } from '@inyon/protocol'
import { haptic } from '../../lib/haptics.ts'
import { api } from '../../lib/api.ts'
import { toast } from '../../lib/store.ts'
import { AgentMark } from './Messages.tsx'
import { copyText } from '../../lib/clipboard.ts'

interface Block {
  kind: 'text' | 'code'
  lines: string[]
}

/** Splits the prompt body into prose and the command/file framed by dashed separators. */
function parseBody(body: string[]) {
  // drop hints that only make sense inside a terminal
  const lines = body.filter((l) => !/^\s*Tip:/.test(l) && !/Hold Shift|native copy|iTerm2|Terminal\.app/.test(l)).map((l) => l.replace(/\s*\(c to copy\)/, ''))
  const title = lines[0]?.trim() ?? ''
  const blocks: Block[] = []
  let inCode = false
  for (const l of lines.slice(1)) {
    if (l === '╌') {
      inCode = !inCode
      continue
    }
    const kind = inCode ? 'code' : 'text'
    const last = blocks[blocks.length - 1]
    if (last?.kind === kind) last.lines.push(l)
    else blocks.push({ kind, lines: [l] })
  }
  return { title, blocks: blocks.filter((b) => b.lines.some((l) => l.trim())) }
}

/** Re-joins lines the terminal wrapped, keeping real line starts (bullets, arrows, numbered items). */
function joinWrapped(lines: string[]) {
  let out = ''
  for (const raw of lines) {
    const l = raw.trim()
    if (!l) continue
    const newLine = !out || /^([●•◦→\-*]|\d+[.)])\s/.test(l) || /[?:.!]$/.test(out)
    out += newLine ? (out ? '\n' : '') + l : ` ${l}`
  }
  return out
}

type DiffKind = 'add' | 'del' | 'same'

/**
 * Claude's edit prompt frames a diff: "12  text" (unchanged), "14 -  text" (removed), "14 +  text"
 * (added), with the terminal's wrapped tails on lines of their own. Each line's kind, or undefined
 * when the block isn't a diff (a command, a file's content).
 */
export function diffKinds(lines: string[]): DiffKind[] | undefined {
  if (!lines.some((l) => /^\s*\d+ [-+]/.test(l))) return undefined
  let last: DiffKind = 'same'
  return lines.map((l) => {
    const m = /^\s*\d+(?: ([-+ ])|$)/.exec(l)
    if (m) last = m[1] === '+' ? 'add' : m[1] === '-' ? 'del' : 'same'
    // a line without a number is the rest of the one above, wrapped
    return last
  })
}

/** A command or a file's text; an edit's diff in the Changes view's colours. */
function CodeBlock({ lines }: { lines: string[] }) {
  const text = lines.map((l) => l.replace(/^ /, ''))
  while (text.length && !text[0]!.trim()) text.shift()
  while (text.length && !text[text.length - 1]!.trim()) text.pop()
  const kinds = diffKinds(text)
  return (
    <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-xl border border-line bg-code px-3 py-2 font-mono text-[12.5px] leading-relaxed">
      {kinds
        ? text.map((l, i) => {
            const sign = /^(\s*\d+ )([-+])(.*)$/.exec(l)
            return (
              <span key={i} data-diff={kinds[i]} className={clsx('-mx-3 block px-3', kinds[i] === 'add' ? 'bg-[var(--diff-add)]' : kinds[i] === 'del' && 'bg-[var(--diff-del)]')}>
                {sign ? (
                  <>
                    {sign[1]}
                    <span className={sign[2] === '+' ? 'text-ok' : 'text-danger'}>{sign[2]}</span>
                    {sign[3]}
                  </>
                ) : (
                  l || ' '
                )}
              </span>
            )
          })
        : text.join('\n').trim()}
    </pre>
  )
}

/** `disabled`: shown as last seen while the machine reconnects; nothing can be answered yet */
export function PromptCard({ prompt, host, pane, disabled }: { prompt: ScreenPrompt; host: string; pane: string; disabled?: boolean }) {
  const [busy, setBusy] = useState<number | string | null>(null)
  const [freeText, setFreeText] = useState<string | null>(null)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const { title, blocks } = parseBody(prompt.body)
  // the hub sends Claude's prompts as whole lines, so their line breaks are the agent's own
  const wholeLines = (prompt as { wholeLines?: boolean }).wholeLines === true
  const isQuestion = !!prompt.tabs?.length || /\?$/.test(title)
  const locked = busy !== null || !!disabled

  useEffect(() => setBusy(null), [prompt])
  useEffect(() => {
    if (freeText !== null) inputRef.current?.focus()
  }, [freeText !== null])

  const run = async (id: number | string, fn: () => Promise<unknown>) => {
    setBusy(id)
    try {
      await fn()
      haptic('select')
    } catch (e) {
      toast({ title: 'Could not answer', body: (e as Error).message, tone: 'error' })
      setBusy(null)
    }
    // the screen poll replaces this card once Claude moves on
    setTimeout(() => setBusy(null), 2500)
  }

  const pick = (n: number) => {
    if (n === prompt.freeTextOption) {
      setFreeText('')
      return
    }
    run(n, () => api.answer(host, pane, n, undefined, promptKey(prompt)))
  }
  const keys = (k: string[]) => run(k.join(), () => api.keys(host, pane, k))

  return (
    // never taller than the space it has, so a long permission list can't push the answers off a
    // phone's screen: the question shrinks and scrolls well before the choices do (shrink-[1000]),
    // and Cancel stays put
    <div className="animate-slide-up flex min-h-0 flex-col overflow-hidden rounded-3xl border border-line-strong bg-elev shadow-float">
      {prompt.tabs && prompt.tabs.length > 1 && (
        <div className="flex shrink-0 gap-1.5 overflow-x-auto border-b border-line px-4 py-2.5">
          {prompt.tabs.map((t) => (
            <span
              key={t.label}
              className={clsx(
                'inline-flex h-6 shrink-0 items-center gap-1 rounded-full px-2.5 text-xs font-medium',
                t.current ? 'bg-accent text-accent-fg' : t.done ? 'bg-accent-soft text-accent-text' : 'bg-sunken text-fg-3',
              )}
            >
              {t.done && !t.current && <Check className="size-3" strokeWidth={3} />}
              {t.label}
            </span>
          ))}
        </div>
      )}
      <div className="min-h-12 shrink-[1000] overflow-y-auto overscroll-contain">
        <div className="px-4 pt-3.5 pb-2 sm:px-5">
          <div className="flex items-start gap-2.5">
            <span className="mt-0.5"><AgentMark /></span>
            <div className="min-w-0 flex-1">
              <div className={clsx('font-semibold leading-snug', isQuestion ? 'text-[16px]' : 'text-[15px]')}>{title}</div>
              {blocks.map((b, i) =>
                b.kind === 'code' ? (
                  <CodeBlock key={i} lines={b.lines} />
                ) : (
                  <div key={i} className="mt-1.5 whitespace-pre-line text-sm leading-relaxed text-fg-2">
                    {wholeLines ? b.lines.map((l) => l.trimEnd()).join('\n').trim() : joinWrapped(b.lines)}
                  </div>
                ),
              )}
            </div>
          </div>
        </div>

        {prompt.links && prompt.links.length > 0 && (
          <div className="space-y-2 px-3 pb-2 sm:px-4">
            {prompt.links.map((u) => (
              <div key={u} className="rounded-2xl border border-line bg-bg p-2.5">
                <div className="line-clamp-2 break-all font-mono text-[12px] leading-relaxed text-fg-2">{u}</div>
                <div className="mt-2 flex gap-1.5">
                  <a href={u} target="_blank" rel="noreferrer" className="inline-flex h-8 items-center gap-1.5 rounded-full bg-fg px-3 text-[12.5px] font-medium text-bg">
                    <ExternalLink className="size-3.5" /> Open
                  </a>
                  <button
                    type="button"
                    onClick={async () => {
                      await copyText(u)
                      toast({ title: 'Link copied', tone: 'success' })
                    }}
                    className="inline-flex h-8 items-center gap-1.5 rounded-full border border-line bg-elev px-3.5 shadow-card text-[12.5px] font-medium hover:bg-hover"
                  >
                    <Copy className="size-3.5" /> Copy link
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {prompt.input ? (
        <form
          className="flex shrink-0 items-end gap-2 px-3 pb-3 sm:px-4"
          onSubmit={(e) => {
            e.preventDefault()
            const v = (e.currentTarget.elements.namedItem('answer') as HTMLInputElement).value
            if (v.trim()) run('input', () => api.input(host, pane, v.trim()))
          }}
        >
          <label className="min-w-0 flex-1">
            <span className="mb-1 block text-[12px] text-fg-3">{prompt.input.label}</span>
            <input name="answer" autoComplete="off" autoCapitalize="off" spellCheck={false} className="h-11 w-full rounded-xl border border-field bg-bg px-3 font-mono text-[14px] transition-colors focus:border-fg" placeholder="Paste here" />
          </label>
          <button type="submit" disabled={locked} className="press flex size-11 shrink-0 items-center justify-center rounded-full bg-accent text-accent-fg disabled:opacity-40" aria-label="Submit">
            <Send className="size-4" />
          </button>
        </form>
      ) : freeText !== null ? (
        <form
          className="flex shrink-0 items-end gap-2 px-3 pb-3 sm:px-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (freeText.trim()) run('free', () => api.answer(host, pane, prompt.freeTextOption!, freeText, promptKey(prompt)))
          }}
        >
          <textarea
            ref={inputRef}
            value={freeText}
            rows={2}
            onChange={(e) => setFreeText(e.target.value)}
            placeholder="Type your answer…"
            aria-label="Your answer"
            className="min-h-[52px] flex-1 resize-none rounded-2xl border border-field bg-bg px-3.5 py-2.5 text-[15px] outline-none transition-colors focus:border-fg"
          />
          <button
            type="submit"
            disabled={!freeText.trim() || locked}
            className="press flex size-11 items-center justify-center rounded-full bg-accent text-accent-fg disabled:opacity-40"
            aria-label="Send answer"
          >
            <Send className="size-4.5" />
          </button>
        </form>
      ) : (
        <div className="min-h-0 space-y-1.5 overflow-y-auto overscroll-contain px-3 pb-2 sm:px-4">
          {prompt.options.map((o) => (
            <button
              key={o.n}
              type="button"
              disabled={locked}
              onClick={() => pick(o.n)}
              className={clsx(
                'flex w-full items-start gap-3 rounded-2xl border px-3.5 py-3 text-left transition-[background-color,border-color,transform] active:scale-[0.99] disabled:opacity-60',
                o.selected ? 'border-accent/50 bg-accent-soft' : 'border-line bg-bg hover:bg-hover',
                busy === o.n && 'border-accent bg-accent-soft',
              )}
            >
              {o.checked !== undefined ? (
                <span
                  className={clsx(
                    'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md border',
                    o.checked ? 'border-accent bg-accent text-accent-fg' : 'border-line-strong',
                  )}
                >
                  {o.checked && <Check className="size-3.5" strokeWidth={3} />}
                </span>
              ) : (
                <span
                  className={clsx(
                    'mt-px flex size-[22px] shrink-0 items-center justify-center rounded-full font-mono text-xs font-semibold',
                    o.selected ? 'bg-accent text-accent-fg' : 'bg-sunken text-fg-2',
                  )}
                >
                  {prompt.navigate ? <span className="size-1.5 rounded-full bg-current" /> : o.n}
                </span>
              )}
              <span className="min-w-0 flex-1">
                <span className="block text-[15px] font-medium leading-snug">{o.label}</span>
                {o.detail && <span className="mt-0.5 block text-[13px] leading-snug text-fg-3">{o.detail}</span>}
              </span>
            </button>
          ))}
        </div>
      )}

      <div className="flex shrink-0 items-center gap-1 border-t border-line px-2 py-1.5">
        <button
          type="button"
          disabled={disabled}
          onClick={() => (freeText !== null ? setFreeText(null) : keys(['Escape']))}
          className="press h-9 rounded-full px-4 text-sm font-medium text-fg-2 hover:bg-hover hover:text-fg disabled:opacity-50"
        >
          {freeText !== null ? 'Back' : 'Cancel'}
        </button>
        <div className="ml-auto flex items-center">
          {prompt.confirm ? (
            // the choices above are toggled one by one; this sends them (OpenCode's questions)
            <button
              type="button"
              disabled={locked}
              onClick={() => run('confirm', () => api.answer(host, pane, CONFIRM_OPTION, undefined, promptKey(prompt)))}
              className="press ml-1 inline-flex h-9 items-center gap-1.5 rounded-full bg-accent px-4 text-sm font-medium text-accent-fg hover:opacity-90 disabled:opacity-50"
            >
              {prompt.confirm} <ArrowRight className="size-4" />
            </button>
          ) : (
            <>
              {/* raw keys only when the choices couldn't be read as buttons, or for multi-select steps */}
              {(!prompt.options.length || prompt.multi) && [
                { k: 'Up', icon: ArrowUp, label: 'Up' },
                { k: 'Down', icon: ArrowDown, label: 'Down' },
                { k: 'Enter', icon: CornerDownLeft, label: 'Enter' },
              ].map(({ k, icon: Icon, label }) => (
                <button key={k} type="button" aria-label={label} title={label} disabled={disabled} onClick={() => keys([k])} className="press relative flex size-9 items-center justify-center rounded-full text-fg-3 after:absolute after:-inset-0.5 hover:bg-hover hover:text-fg disabled:opacity-50">
                  <Icon className="size-4" />
                </button>
              ))}
              {prompt.multi && (
                <button
                  type="button"
                  disabled={disabled}
                  onClick={() => keys(['Right'])}
                  className="press ml-1 inline-flex h-9 items-center gap-1.5 rounded-full bg-accent px-4 text-sm font-medium text-accent-fg hover:opacity-90 disabled:opacity-50"
                >
                  Next <ArrowRight className="size-4" />
                </button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  )
}
