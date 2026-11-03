import { describe, expect, test } from 'bun:test'
import { buildRows, type Row } from './chat.ts'
import { item } from '../test/fixtures.ts'

const types = (rows: Row[]) => rows.map((r) => r.type)
const turn = (r: Row | undefined) => {
  if (r?.type !== 'assistant') throw new Error(`expected an assistant row, got ${r?.type}`)
  return r
}

describe('buildRows', () => {
  test('empty transcript, no rows', () => {
    expect(buildRows([])).toEqual([])
  })

  test('a user message and the reply become two rows', () => {
    const u = item.user('hi')
    const rows = buildRows([u, item.thinking('hmm'), item.text('hello'), item.end(4200)])
    expect(types(rows)).toEqual(['user', 'assistant'])
    expect(rows[0].key).toBe(`u${u.seq}`)
    const a = turn(rows[1])
    expect(a.parts.map((p) => p.type)).toEqual(['thinking', 'text'])
    expect(a.durationMs).toBe(4200)
    expect(a.open).toBe(false)
  })

  test('the last turn stays open until it ends', () => {
    const rows = buildRows([item.user('hi'), item.text('working on it')])
    expect(turn(rows[1]).open).toBe(true)
  })

  test('consecutive tool calls share a group, with their results matched by id', () => {
    const rows = buildRows([item.tool('t1'), item.tool('t2', 'Read'), item.result('t2', 'file'), item.result('t1', 'out'), item.text('done'), item.tool('t3')])
    const parts = turn(rows[0]).parts
    expect(parts.map((p) => p.type)).toEqual(['tools', 'text', 'tools'])
    const group = parts[0].type === 'tools' ? parts[0].calls : []
    expect(group.map((c) => [c.id, c.name, c.result?.text])).toEqual([['t1', 'Bash', 'out'], ['t2', 'Read', 'file']])
    // t3 hasn't answered yet
    expect(parts[2].type === 'tools' && parts[2].calls[0].result).toBeUndefined()
  })

  test('questions, plans and todos stand on their own and split tool groups', () => {
    const rows = buildRows([item.tool('t1'), item.tool('q', 'AskUserQuestion', { questions: [] }), item.tool('t2'), item.tool('p', 'ExitPlanMode'), item.tool('d', 'TodoWrite')])
    const parts = turn(rows[0]).parts
    expect(parts.map((p) => p.type)).toEqual(['tools', 'special', 'tools', 'special', 'special'])
    expect(parts[1].type === 'special' && parts[1].call.name).toBe('AskUserQuestion')
  })

  test('a new user message ends the turn above it', () => {
    const rows = buildRows([item.user('a'), item.text('one'), item.user('b'), item.text('two')])
    expect(types(rows)).toEqual(['user', 'assistant', 'user', 'assistant'])
    expect(turn(rows[1]).open).toBe(false)
    expect(turn(rows[3]).open).toBe(true)
  })

  test('a queued message is a user row in transcript order', () => {
    const rows = buildRows([item.text('busy'), item.user('later please', { queued: true }), item.text('ok')])
    expect(types(rows)).toEqual(['assistant', 'user', 'assistant'])
    expect(rows[1].type === 'user' && rows[1].item.queued).toBe(true)
  })

  describe('commands', () => {
    test('a nameless command right after one fills in its output', () => {
      const rows = buildRows([item.command('compact'), item.command('', '', 'Compacted')])
      expect(rows).toHaveLength(1)
      expect(rows[0]).toMatchObject({ type: 'command', name: 'compact', output: 'Compacted' })
    })

    test("output never overwrites a command that already has some", () => {
      const rows = buildRows([item.command('cost', '', '$1'), item.command('', '', 'other')])
      expect(rows.map((r) => r.type === 'command' && r.output)).toEqual(['$1', 'other'])
    })

    test('a command ends the turn above it', () => {
      const rows = buildRows([item.text('hi'), item.command('cost', ''), item.text('next')])
      expect(types(rows)).toEqual(['assistant', 'command', 'assistant'])
      expect(rows[1]).toMatchObject({ name: 'cost' })
    })

    test("a setting changed with the app's controls isn't shown as a message, nor its output", () => {
      const rows = buildRows([
        item.text('hi'),
        item.command('model', 'opus'),
        item.command('', '', 'Set model to Opus'),
        item.command('/effort', 'high'),
        item.command('compact'),
        item.command('', '', 'Compacted'),
      ])
      expect(types(rows)).toEqual(['assistant', 'command'])
      expect(rows[1]).toMatchObject({ name: 'compact', output: 'Compacted' })
    })
  })

  describe('goals', () => {
    test('set, met and cleared goals keep their flags, in order', () => {
      const rows = buildRows([item.goal('tests pass'), item.goal('tests pass', { met: true, reason: 'green' }), item.goal('tests pass', { cleared: true })])
      expect(types(rows)).toEqual(['goal', 'goal', 'goal'])
      const goals = rows.map((r) => (r.type === 'goal' ? r.item : null))
      expect(goals.map((g) => [g?.met, g?.cleared ?? false, g?.reason])).toEqual([[false, false, undefined], [true, false, 'green'], [false, true, undefined]])
    })

    test('replies after a goal start a new turn instead of joining the one above', () => {
      const rows = buildRows([item.user('go'), item.text('first'), item.goal('ship it'), item.text('second')])
      expect(types(rows)).toEqual(['user', 'assistant', 'goal', 'assistant'])
      expect(turn(rows[1]).parts).toHaveLength(1)
      expect(turn(rows[3]).parts[0]).toMatchObject({ type: 'text', text: 'second' })
    })
  })

  describe('notices', () => {
    test('an interruption joins the turn and ends it', () => {
      const rows = buildRows([item.text('doing'), item.notice('interrupted'), item.text('again')])
      expect(types(rows)).toEqual(['assistant', 'assistant'])
      expect(turn(rows[0]).parts.map((p) => p.type)).toEqual(['text', 'notice'])
      expect(turn(rows[0]).open).toBe(false)
    })

    test('a background job finishing mid-turn stays inside the turn', () => {
      const rows = buildRows([item.text('doing'), item.notice('task', 'job done'), item.text('more')])
      expect(rows).toHaveLength(1)
      expect(turn(rows[0]).parts.map((p) => p.type)).toEqual(['text', 'notice', 'text'])
      expect(turn(rows[0]).open).toBe(true)
    })

    test('other notices, and any notice between turns, are rows of their own', () => {
      const rows = buildRows([item.text('a'), item.notice('compact'), item.notice('interrupted'), item.notice('task')])
      expect(types(rows)).toEqual(['assistant', 'notice', 'notice', 'notice'])
    })
  })

  test('keys are stable across rebuilds and unique', () => {
    const items = [item.user('a'), item.text('b'), item.command('c'), item.goal('d'), item.notice('compact'), item.text('e')]
    const keys = buildRows(items).map((r) => r.key)
    expect(new Set(keys).size).toBe(keys.length)
    expect(buildRows(items).map((r) => r.key)).toEqual(keys)
  })

  test('a long conversation builds without dropping anything', () => {
    const items = []
    for (let i = 0; i < 500; i++) items.push(item.user(`q${i}`), item.tool(`t${i}`), item.result(`t${i}`), item.text(`a${i}`), item.end())
    const rows = buildRows(items)
    expect(rows).toHaveLength(1000)
    expect(rows.every((r) => r.type !== 'assistant' || (!r.open && r.parts.length === 2))).toBe(true)
  })
})
