import { expect, test } from 'bun:test'
import { mergeDevin, type DevinUpdate, type DevinMessage } from './devin.ts'

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
