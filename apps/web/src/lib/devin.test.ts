import { expect, test } from 'bun:test'
import { devinRow, devinTurn, devinTurns, mergeDevin, parseDevinUpdate, type DevinUpdate, type DevinMessage } from './devin.ts'

test('incremental replies preserve large unchanged messages and drop obsolete branch entries', () => {
  const user: DevinMessage = { id: 'u', role: 'user', text: 'long text '.repeat(10000), tools: [] }
  const old: DevinMessage = { id: 'old', role: 'assistant', text: 'old branch', tools: [] }
  const reply: DevinMessage = { id: 'new', role: 'assistant', text: 'new reply', tools: [] }
  const update: DevinUpdate = { version: 2, reset: false, loading: false, session: null, state: null, order: ['u', 'new'], messages: [reply] }
  const next = mergeDevin(new Map([['u', user], ['old', old]]), update)
  expect(next.get('u')).toBe(user)
  expect(next.get('new')).toBe(reply)
  expect(next.has('old')).toBe(false)
  expect(mergeDevin(next, { ...update, reset: true }).has('u')).toBe(false)
})

test('saved Devin messages preserve full content in the shared chat renderer', () => {
  const text = '中文 multiline\n'.repeat(10000)
  const user = devinRow({ id: 'u', role: 'user', text, tools: [] })
  expect(user.type === 'user' && user.item.text).toBe(text)
  const reply = devinRow({ id: 'a', role: 'assistant', text, thinking: 'reasoning', tools: [{ name: 'Read', input: { path: '/tmp/example' } }] })
  expect(reply.type === 'assistant' && reply.parts.map((p) => p.type)).toEqual(['thinking', 'text', 'tools'])
  expect(reply.type === 'assistant' && reply.parts.find((p) => p.type === 'text')?.text).toBe(text)
})

test('one native reply shares a turn and uses only its final timestamp', () => {
  const messages: DevinMessage[] = [
    { id: 'u', role: 'user', text: 'question', tools: [] },
    { id: 'a', role: 'assistant', text: 'checking', tools: [{ name: 'Read', input: {} }], time: 'first' },
    { id: 't', role: 'tool', text: 'result', tools: [], time: 'tool-time' },
    { id: 'z', role: 'assistant', text: 'answer', tools: [], time: 'final' },
    { id: 'u2', role: 'user', text: 'next', tools: [] },
    { id: 'a2', role: 'assistant', text: 'next answer', tools: [] },
  ]
  const turns = devinTurns(messages)
  expect(turns.map((t) => t.map((m) => m.id))).toEqual([['u'], ['a', 't', 'z'], ['u2'], ['a2']])
  const row = devinTurn(turns[1]!)
  expect(row.type).toBe('assistant')
  expect(row.type === 'assistant' && row.ts).toBe('final')
  expect(row.type === 'assistant' && row.parts.map((p) => p.type)).toEqual(['text', 'tools', 'tools', 'text'])
  expect(devinTurn(turns[0]!)).toEqual(devinRow(messages[0]!))
})

test('malformed or incomplete history never becomes a valid update', () => {
  const valid: DevinUpdate = { version: 2, reset: true, loading: false, session: null, state: null, order: [], messages: [] }
  expect(parseDevinUpdate(valid)).toBe(valid)
  for (const value of [{}, null, { ...valid, messages: undefined }, { ...valid, order: null },
    { ...valid, messages: [{ id: 'a', role: 'assistant', text: 'answer', tools: null }] },
    { ...valid, state: { status: 'idle' } }]) {
    expect(() => parseDevinUpdate(value)).toThrow('Retrying')
  }
})
