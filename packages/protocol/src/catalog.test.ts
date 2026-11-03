import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { AGENTS, agyModelId } from './agents.ts'
import { CATALOG_JSON, plainCatalog } from '../scripts/catalog.ts'

test("the Rust crate's copy of the catalog matches AGENTS", () => {
  const copy = JSON.parse(readFileSync(CATALOG_JSON, 'utf8'))
  // when this fails after a change to agents.ts: bun packages/protocol/scripts/catalog.ts
  expect(copy).toEqual(plainCatalog())
})

test('every command in `set` is its template filled in, the way the Rust side does it', () => {
  const ids = ['opus', 'gpt-6-luna', 'gemini-3.8-flash', 'gemini-3.1-pro', 'claude-sonnet-4-6']
  const efforts = [undefined, 'low', 'medium', 'high', 'max']
  for (const def of Object.values(AGENTS)) {
    const { model, effort, goal, ultracode } = def.set
    // model_command in crates/inyon-protocol/src/agents.rs
    for (const id of ids) {
      for (const e of efforts) {
        const filled = def.kind === 'antigravity' ? agyModelId(id, e) : id
        if (model) expect(model(id, e)).toBe(model('{id}').replace('{id}', filled))
      }
    }
    if (effort) for (const l of ['low', 'xhigh', 'ultracode']) expect(effort(l)).toBe(effort('{level}').replace('{level}', l))
    if (goal) expect(goal('ship it, then {tidy}')).toBe(goal('{text}').replace('{text}', 'ship it, then {tidy}'))
    if (ultracode) expect([ultracode(true), ultracode(false)]).toEqual(['/effort ultracode on', '/effort ultracode off'])
  }
})

test('gen-check.ts compares every exported type', () => {
  const check = readFileSync(new URL('./gen-check.ts', import.meta.url), 'utf8')
  const missing: string[] = []
  for (const file of ['index.ts', 'agents.ts', 'versions.ts']) {
    const source = readFileSync(new URL(`./${file}`, import.meta.url), 'utf8')
    for (const [, name] of source.matchAll(/^export (?:interface|type) (\w+)/gm)) if (!new RegExp(`\\bH\\.${name}\\b`).test(check)) missing.push(`${file}: ${name}`)
  }
  expect(missing).toEqual([])
})
