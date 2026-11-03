import { describe, expect, test } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import path from 'node:path'

// The design rules keep sparkle marks out of the app (they read as "AI magic" rather than saying
// what a control does). Ultracode uses a gauge and skills a bookmark instead.
const BANNED = ['Sparkle', 'Sparkles', 'WandSparkles', 'Wand2', 'Wand']

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = path.join(dir, f)
    return statSync(p).isDirectory() ? files(p) : /\.tsx?$/.test(f) ? [p] : []
  })
}

describe('icons', () => {
  test('no sparkle or wand icons are imported from lucide', () => {
    const src = path.join(import.meta.dir, '..')
    const hits = files(src).flatMap((f) => {
      const imports = [...readFileSync(f, 'utf8').matchAll(/import\s*\{([^}]*)\}\s*from\s*'lucide-react'/g)].flatMap((m) => m[1].split(',').map((s) => s.trim().split(/\s+as\s+/)[0]))
      return imports.filter((i) => BANNED.includes(i)).map((i) => `${path.relative(src, f)}: ${i}`)
    })
    expect(hits).toEqual([])
  })
})
