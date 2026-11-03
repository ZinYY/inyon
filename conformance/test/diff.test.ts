import { expect, test } from 'bun:test'
import fs from 'node:fs'
import path from 'node:path'
import { report } from '../lib/diff.ts'
import { runScenario, type Scenario } from '../lib/scenario.ts'

const DIR = path.join(import.meta.dir, '..')
const golden = (name: string) => fs.readFileSync(path.join(DIR, 'golden', `${name}.jsonl`), 'utf8').split('\n').filter(Boolean)

test('matching transcripts report nothing', () => {
  const lines = golden('push')
  expect(report(lines, [...lines])).toBe('')
})

test('a changed line is shown with the JSON path that changed', () => {
  const lines = golden('push')
  const i = lines.findIndex((l) => l.includes('Bad push registration: keys'))
  const changed = [...lines]
  changed[i] = changed[i]!.replace('"status":400', '"status":422')
  const out = report(lines, changed)
  expect(out).toContain('- ')
  expect(out).toContain('+ ')
  expect(out).toContain('~ status: 400 → 422')
})

test('a missing and an extra line are both reported', () => {
  const lines = golden('push')
  const out = report(lines, [...lines.slice(1), '{"extra":true}'])
  expect(out).toContain(`- ${lines[0]}`)
  expect(out).toContain('+ {"extra":true}')
})

test(
  'a hub that answers one request differently fails its scenario',
  async () => {
    const push = (await import('../scenarios/12-push.ts')).default as Scenario
    const altered = `'${process.execPath}' '${path.join(import.meta.dir, 'altered-hub.ts')}'`
    const out = report(golden('push'), await runScenario(push, { command: altered }))
    expect(out).toContain('~ body.error: "Which endpoint?" → "Which endpoint"')
    // only the two answers that carry that text differ
    expect(out.match(/^ +~ /gm)?.length).toBe(2)
  },
  60_000,
)
