import { afterEach, describe, expect, setSystemTime, test } from 'bun:test'
import type { ChatItem, SendState } from '@inyon/protocol'
import { msgKey, norm, othersSends, reconcile, settle, type PendingMessage } from './pending.ts'
import { item } from '../test/fixtures.ts'

const T = Date.UTC(2026, 9, 11, 12, 0, 0)
const iso = (ms: number) => new Date(ms).toISOString()
let id = 0
const msg = (text: string, at = T): PendingMessage => ({ id: `p${++id}`, text, status: 'sent', at })
const said = (text: string, at = T + 1000, more: Partial<Extract<ChatItem, { kind: 'user' }>> = {}) => item.user(text, { ts: iso(at), ...more })
const left = (pending: PendingMessage[], items: ChatItem[]) => reconcile(pending, items).map((p) => p.text)

describe('reconcile', () => {
  test('the transcript entry confirms the bubble', () => {
    expect(left([msg('fix the login bug')], [said('fix the login bug')])).toEqual([])
  })

  test('different text stays pending', () => {
    expect(left([msg('fix the login bug')], [said('fix the signup bug')])).toEqual(['fix the login bug'])
  })

  test('an older identical message does not confirm a new send', () => {
    expect(left([msg('yes')], [said('yes', T - 60_000)])).toEqual(['yes'])
  })

  test('clocks a few seconds apart still match', () => {
    expect(left([msg('yes')], [said('yes', T - 20_000)])).toEqual([])
  })

  test('each receipt confirms one bubble only', () => {
    const p = [msg('continue'), msg('continue')]
    expect(reconcile(p, [said('continue')]).map((m) => m.id)).toEqual([p[1].id])
    expect(reconcile(p, [said('continue'), said('continue', T + 2000)])).toEqual([])
  })

  test('spacing, case, quotes and paste markup do not matter', () => {
    const items = [said('Don’t   touch\n\n"main"'), said('<pasted_content id=1>a long paste</pasted_content>')]
    expect(left([msg("don't touch “main”"), msg('a long paste')], items)).toEqual([])
  })

  test('shell commands match with or without the space after !', () => {
    const items = [said('ls -la', T + 1000, { shell: true }), said('pwd', T + 1000, { shell: true })]
    expect(left([msg('!ls -la'), msg('! pwd')], items)).toEqual([])
    // the same words without the shell flag are a different message
    expect(left([msg('!ls')], [said('ls')])).toEqual(['!ls'])
  })

  test('slash commands are confirmed by command entries', () => {
    const items = [item.command('model', 'opus', undefined, iso(T + 500)), item.command('/compact', '', undefined, iso(T + 500))]
    expect(left([msg('/model opus'), msg('/compact')], items)).toEqual([])
  })

  test('nameless command output confirms nothing', () => {
    expect(left([msg('/')], [item.command('', '', 'out', iso(T + 500))])).toEqual(['/'])
  })

  test('assistant text that repeats the message confirms nothing', () => {
    const echo: ChatItem = { ...item.text('repeat after me'), ts: iso(T + 1000) }
    expect(left([msg('repeat after me')], [echo])).toEqual(['repeat after me'])
  })

  test('a very long message cut short in the transcript still matches', () => {
    const long = 'word '.repeat(10_000)
    expect(left([msg(long)], [said(long.slice(0, 30_000))])).toEqual([])
    // a short prefix is not enough (the cut is at 20k letters, spaces aside)
    expect(left([msg(long)], [said(long.slice(0, 500))])).toEqual([long])
  })

  test("Codex's image envelopes are put back to the paths that were sent", () => {
    const sent = 'look at this\n\nAttached files:\n/tmp/uploads/a.png\n/tmp/uploads/b.txt'
    const codex = '<image name=[Image #1] path="/tmp/uploads/a.png">\n</image>look at this\n\nAttached files:\n[Image #1]\n/tmp/uploads/b.txt'
    expect(left([msg(sent)], [said(codex)])).toEqual([])
    const other = codex.replace('a.png">', 'z.png">')
    expect(left([msg(sent)], [said(other)])).toEqual([sent])
  })

  test('nothing pending, nothing to do', () => {
    expect(reconcile([], [said('hi')])).toEqual([])
  })
})

describe('norm and msgKey', () => {
  test('norm collapses whitespace', () => {
    expect(norm('  a \n\t b  ')).toBe('a b')
  })

  test('msgKey ignores paste markup and shell spacing, and caps the length', () => {
    expect(msgKey('<pasted_content>hi\nthere</pasted_content>')).toBe('hi there')
    expect(msgKey('!  ls')).toBe('!ls')
    expect(msgKey('x'.repeat(1000))).toHaveLength(400)
  })
})

const ledger = (p: PendingMessage, state: SendState['state'], more: Partial<SendState> = {}): SendState => ({ id: p.id, text: p.text, at: p.at, state, ...more })

describe('settle', () => {
  test('delivered drops the bubble, even with no transcript entry yet', () => {
    const p = msg('ship it')
    expect(settle([p], [], [ledger(p, 'delivered', { uuid: 'x' })])).toEqual([])
  })

  test("a delivered send drops a bubble that had given up (the reply arrived late)", () => {
    const p: PendingMessage = { ...msg('ship it'), status: 'failed', error: 'timed out' }
    expect(settle([p], [], [ledger(p, 'delivered')])).toEqual([])
  })

  test('queued marks the bubble as queued', () => {
    const p: PendingMessage = { ...msg('next'), status: 'sending' }
    expect(settle([p], [], [ledger(p, 'queued')])).toEqual([{ ...p, status: 'sent', queued: true }])
  })

  test('leaving the queue unmarks it', () => {
    const p: PendingMessage = { ...msg('next'), queued: true }
    expect(settle([p], [], [ledger(p, 'sent')])).toEqual([{ ...p, queued: false }])
  })

  test('failed in the ledger fails a bubble that thought it was sent, with the reason', () => {
    const p = msg('do it')
    expect(settle([p], [], [ledger(p, 'failed', { error: 'pane is gone' })])).toEqual([{ ...p, status: 'failed', error: 'pane is gone' }])
  })

  test('a send still in flight keeps its bubble as it is', () => {
    const p: PendingMessage = { ...msg('do it'), status: 'sending' }
    expect(settle([p], [], [ledger(p, 'sending')])).toEqual([p])
    expect(settle([p], [], [ledger(p, 'failed', { error: 'x' })])).toEqual([p])
  })

  test('the ledger wins over matching text: a tracked send stays until it says delivered', () => {
    const p = msg('yes')
    expect(settle([p], [said('yes')], [ledger(p, 'sent')])).toEqual([p])
  })

  test('a send the ledger does not track falls back to matching text', () => {
    const tracked = msg('one')
    const old = msg('two')
    const other = msg('three')
    expect(settle([tracked, old, other], [said('two')], [ledger(tracked, 'sent')]).map((p) => p.text)).toEqual(['one', 'three'])
    // no ledger at all (an older hub)
    expect(settle([old], [said('two')])).toEqual([])
  })

  test("a tracked send does not use up another's transcript entry", () => {
    const a = msg('same')
    const b = msg('same')
    // a is delivered by id; b is untracked and has its own receipt
    expect(settle([a, b], [said('same')], [ledger(a, 'delivered')])).toEqual([])
  })
})

describe('othersSends', () => {
  afterEach(() => setSystemTime())
  const send = (id: string, state: SendState['state'], ageMs = 0): SendState => ({ id, text: `text ${id}`, at: T - ageMs, state })

  test("shows other devices' sends that haven't arrived, marked remote", () => {
    setSystemTime(T)
    expect(othersSends([send('a', 'sending'), send('b', 'sent', 60_000)], [])).toEqual([
      { id: 'a', text: 'text a', at: T, status: 'sending', remote: true },
      { id: 'b', text: 'text b', at: T - 60_000, status: 'sent', remote: true },
    ])
  })

  test('not this device\'s own, nor queued, delivered or failed ones', () => {
    setSystemTime(T)
    const mine = { ...msg('mine'), id: 'm' }
    const sends = [send('m', 'sending'), send('q', 'queued'), send('d', 'delivered'), send('f', 'failed')]
    expect(othersSends(sends, [mine])).toEqual([])
  })

  test("a setting changed with the app's controls is never shown as a message", () => {
    setSystemTime(T)
    expect(othersSends([{ ...send('s', 'sending'), text: '/model gpt-6.1-sol', setting: true }, send('a', 'sending')], []).map((p) => p.id)).toEqual(['a'])
  })

  test('forgets sends older than five minutes', () => {
    setSystemTime(T)
    expect(othersSends([send('old', 'sent', 5 * 60_000), send('new', 'sent', 5 * 60_000 - 1)], []).map((p) => p.id)).toEqual(['new'])
  })

  test('no ledger, nothing to show', () => {
    expect(othersSends(undefined, [])).toEqual([])
  })
})
