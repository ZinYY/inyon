// Writes the AGENTS catalog as plain JSON for the Rust side (crates/inyon-protocol/src/agents.json).
// Run it after changing agents.ts: `bun packages/protocol/scripts/catalog.ts`. The protocol tests
// fail until the two match.
import { AGENTS, type AgentDef } from '../src/agents.ts'

export const CATALOG_JSON = new URL('../../../crates/inyon-protocol/src/agents.json', import.meta.url)

// The functions in `set` become templates, filled in again on the Rust side
const template = (set: AgentDef['set']) => ({
  model: set.model?.('{id}'),
  effort: set.effort?.('{level}'),
  goal: set.goal?.('{text}'),
  clearGoal: set.clearGoal,
  compact: set.compact,
  clear: set.clear,
  rc: set.rc,
  ultracode: set.ultracode && { on: set.ultracode(true), off: set.ultracode(false) },
})

/** The catalog without functions or regular expressions: what agents.json should hold. */
export function plainCatalog(): unknown {
  const out: Record<string, unknown> = {}
  for (const [kind, def] of Object.entries(AGENTS)) {
    const { args, set, ...rest } = def
    out[kind] = {
      ...rest,
      args: args.map((r) => {
        if (r.flags) throw new Error(`${kind}: the Rust copy reads patterns without flags (${r})`)
        return r.source
      }),
      set: template(set),
    }
  }
  // drops the keys left undefined, as the hub's JSON does
  return JSON.parse(JSON.stringify(out))
}

if (import.meta.main) {
  await Bun.write(CATALOG_JSON, `${JSON.stringify(plainCatalog(), null, 2)}\n`)
  console.log(`wrote ${CATALOG_JSON.pathname}`)
}
