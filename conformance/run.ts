#!/usr/bin/env bun
/**
 * Hub conformance: runs every scenario against a hub and compares what it answered with the
 * golden transcripts in conformance/golden.
 *
 *   bun conformance/run.ts                    the hub cargo built in this checkout
 *   bun conformance/run.ts --hub '<command>'  any hub started with the same environment
 *   bun conformance/run.ts --update           rewrite the goldens from this run
 *   bun conformance/run.ts --only claude-send,phones   some scenarios
 *   bun conformance/run.ts --keep             keep each scenario's temp folder
 *   bun conformance/run.ts --verbose          print the hub's output as it goes
 */
import fs from 'node:fs'
import path from 'node:path'
import { parseArgs } from 'node:util'
import { report } from './lib/diff.ts'
import { runScenario, type Scenario } from './lib/scenario.ts'

const { values } = parseArgs({
  options: {
    hub: { type: 'string' },
    update: { type: 'boolean', default: false },
    only: { type: 'string' },
    keep: { type: 'boolean', default: false },
    verbose: { type: 'boolean', default: false },
    out: { type: 'string' },
  },
})

const DIR = import.meta.dir
const GOLDEN = path.join(DIR, 'golden')

export async function loadScenarios(): Promise<Scenario[]> {
  const files = fs.readdirSync(path.join(DIR, 'scenarios')).filter((f) => f.endsWith('.ts')).sort()
  return Promise.all(files.map(async (f) => (await import(path.join(DIR, 'scenarios', f))).default as Scenario))
}

const only = values.only?.split(',').map((s) => s.trim())
const scenarios = (await loadScenarios()).filter((s) => !only || only.includes(s.name))
if (!scenarios.length) {
  console.error(`no scenario named ${values.only}`)
  process.exit(2)
}

const started = Date.now()
let failed = 0
fs.mkdirSync(GOLDEN, { recursive: true })
if (values.out) fs.mkdirSync(values.out, { recursive: true })
for (const s of scenarios) {
  const t0 = Date.now()
  let lines: string[]
  try {
    lines = await runScenario(s, { command: values.hub, keep: values.keep, log: values.verbose ? (l) => process.stderr.write(l) : undefined })
  } catch (e) {
    failed++
    console.log(`✗ ${s.name}  ${(e as Error).message}`)
    continue
  }
  const secs = ((Date.now() - t0) / 1000).toFixed(1)
  const file = path.join(GOLDEN, `${s.name}.jsonl`)
  const text = lines.map((l) => `${l}\n`).join('')
  if (values.out) fs.writeFileSync(path.join(values.out, `${s.name}.jsonl`), text)
  if (values.update) {
    fs.writeFileSync(file, text)
    console.log(`✎ ${s.name}  ${lines.length} lines, ${secs} s`)
    continue
  }
  const golden = fs.existsSync(file) ? fs.readFileSync(file, 'utf8').split('\n').filter(Boolean) : []
  const diff = report(golden, lines)
  if (!diff) console.log(`✓ ${s.name}  ${secs} s`)
  else {
    failed++
    console.log(`✗ ${s.name}  differs from golden/${s.name}.jsonl (${secs} s)\n${diff}\n`)
  }
}

const total = ((Date.now() - started) / 1000).toFixed(1)
if (values.update) console.log(`\nwrote ${scenarios.length} golden files in ${total} s`)
else console.log(`\n${scenarios.length - failed} of ${scenarios.length} scenarios match (${total} s)${failed ? '. Run with --update if the change is intended.' : ''}`)
process.exit(failed ? 1 : 0)
