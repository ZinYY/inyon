import { expect, test } from 'bun:test'
import { devinRow, mergeDevin, type DevinUpdate, type DevinMessage } from './devin.ts'

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
