import { expect, test } from 'bun:test'
import { reuse } from './reuse.ts'
import { agentPane, host } from '../test/fixtures.ts'

test('an equal overview is the previous one', () => {
  const prev = [host('a', { panes: [agentPane('%1', 'claude')] })]
  const next = [host('a', { panes: [agentPane('%1', 'claude')] })]
  expect(reuse(prev, next)).toBe(prev)
})

test('one changed pane keeps every other object', () => {
  const prev = [host('a', { panes: [agentPane('%1', 'claude'), agentPane('%2', 'codex')] }), host('b')]
  const next = [host('a', { panes: [agentPane('%1', 'claude'), agentPane('%2', 'codex', { title: 'new' })] }), host('b')]
  const out = reuse(prev, next)
  expect(out).not.toBe(prev)
  expect(out[1]).toBe(prev[1])
  expect(out[0]!.config).toBe(prev[0]!.config)
  const [w1, w2] = out[0]!.sessions[0]!.windows
  expect(w1).toBe(prev[0]!.sessions[0]!.windows[0])
  expect(w2!.panes[0]).not.toBe(prev[0]!.sessions[0]!.windows[1]!.panes[0])
  expect(w2!.panes[0]!.title).toBe('new')
})

test('a removed or added machine changes the list only', () => {
  const a = host('a')
  const b = host('b')
  const shorter = reuse([a, b], [host('a')])
  expect(shorter).toHaveLength(1)
  expect(shorter[0]).toBe(a)
  const longer = reuse([a], [host('a'), host('b')])
  expect(longer[0]).toBe(a)
  expect(longer[1]).toEqual(b)
})

test('a dropped field counts as a change', () => {
  const prev = { x: 1, y: 2 }
  expect(reuse(prev, { x: 1 } as any)).toEqual({ x: 1 })
  expect(reuse({ x: 1 } as any, prev)).toEqual(prev)
})
