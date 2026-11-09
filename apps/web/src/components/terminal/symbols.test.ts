import { expect, mock, test } from 'bun:test'
import { existsSync, readFileSync, statSync } from 'node:fs'
import path from 'node:path'
import { terminalFontsReady } from './theme.ts'

const css = readFileSync(path.join(import.meta.dir, 'symbols.css'), 'utf8')
const src = readFileSync(path.join(import.meta.dir, 'TerminalView.tsx'), 'utf8')
const theme = readFileSync(path.join(import.meta.dir, 'theme.ts'), 'utf8')
// the new app's terminal (apps/app/term) draws with the same font and contrast
const shim = readFileSync(path.join(import.meta.dir, '../../../../app/term/term.ts'), 'utf8')

// "⏵⏵ accept edits on" came out as two boxes on an Android phone: its fonts have no U+23F5
test("the terminal's last font draws the symbols agents use that phones lack", () => {
  const url = /url\(\.\/([^)]+)\)/.exec(css)?.[1]
  expect(url).toBe('symbols.woff2')
  const file = path.join(import.meta.dir, url!)
  expect(existsSync(file)).toBe(true)
  // a cut of a few glyphs, not a whole symbol font in the APK
  expect(statSync(file).size).toBeLessThan(4096)
  expect(css).toContain("font-family: 'Inyon Symbols'")
  // ⏵ ⏸ ⏹ ⏺ and the rest of the block
  expect(css).toContain('unicode-range: U+23E9-23FA')
  expect(src).toContain("import './symbols.css'")
  expect(/TERMINAL_FONT = "([^"]+)"/.exec(theme)?.[1]?.trim().endsWith("'Inyon Symbols'")).toBe(true)
  for (const s of [src, shim]) expect(s).toMatch(/fontFamily: TERMINAL_FONT/)
  expect(shim).toContain('symbols.woff2')
  expect(existsSync(path.join(import.meta.dir, 'symbols-OFL.txt'))).toBe(true)
})

// parts of Claude's diffs came out dark on dark over psmux: xterm lifts text that is too faint
test('text in the terminal keeps a readable contrast', () => {
  for (const s of [src, shim]) expect(s).toMatch(/minimumContrastRatio: MIN_CONTRAST/)
  expect(Number(/MIN_CONTRAST = ([\d.]+)/.exec(theme)?.[1])).toBeGreaterThanOrEqual(4.5)
})

test('private-use prompt icons have a terminal-only, bundled fallback', () => {
  const file = path.join(import.meta.dir, 'nerd-symbols.woff2')
  expect(readFileSync(file).subarray(0, 4).toString()).toBe('wOF2')
  expect(statSync(file).size).toBeLessThan(1000000)
  expect(css).toContain('unicode-range: U+E000-F8FF,U+F0000-FFFFD,U+100000-10FFFD')
  expect(theme).toContain("TERMINAL_FONT = \"'Inyon Nerd Symbols', 'JetBrains Mono Variable'")
  expect(readFileSync(path.join(import.meta.dir, '../../fonts.css'), 'utf8')).not.toContain('Nerd')
  expect(existsSync(path.join(import.meta.dir, 'nerd-symbols-LICENSE.txt'))).toBe(true)
  for (const s of [src, shim]) expect(s).toContain('terminalFontsReady().then')
})

test('font readiness waits for late icon loading and tolerates a failed fallback', async () => {
  let finish!: (faces: FontFace[]) => void
  const load = mock((font: string) => font.includes('Nerd') ? new Promise<FontFace[]>((r) => { finish = r }) : Promise.reject(new Error('offline')))
  let ready = false
  const promise = terminalFontsReady({ load }).then((loaded) => { ready = loaded })
  await Promise.resolve()
  expect(ready).toBe(false)
  finish([{} as FontFace])
  await promise
  expect(ready).toBe(true)
  expect(load.mock.calls.map(([font]) => font)).toHaveLength(2)
  expect(await terminalFontsReady({ load: async () => [] })).toBe(false)
  expect(await terminalFontsReady({ load: async () => { throw new Error('offline') } })).toBe(false)
})
