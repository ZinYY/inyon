import { expect, test } from 'bun:test'
import { modifiedInput } from './modifiers.ts'

test('Shift stays held across multiple keys, including Tab, arrows and letters', () => {
  let mods = { ctrl: false, alt: false, shift: true }
  for (const [input, output] of [['\t', '\x1b[Z'], ['\x1b[A', '\x1b[1;2A'], ['a', 'A'], ['1', '!'], ['\r', '\x1b[13;2u']]) {
    const result = modifiedInput(input, mods)
    expect(result.data).toBe(output)
    expect(result.mods.shift).toBe(true)
    mods = result.mods
  }
  mods.shift = false
  expect(modifiedInput('\t', mods).data).toBe('\t')
})

test('Shift combines with one-shot Ctrl/Alt and preserves paste and already modified keys', () => {
  const result = modifiedInput('\x1b[D', { ctrl: true, alt: true, shift: true })
  expect(result.data).toBe('\x1b[1;8D')
  expect(result.mods).toEqual({ ctrl: false, alt: false, shift: true })
  expect(modifiedInput('\x1b[1;5A', result.mods).data).toBe('\x1b[1;6A')
  expect(modifiedInput('c', { ctrl: true, alt: false, shift: false }).data).toBe('\x03')
  const paste = '\x1b[200~Keep This 123\x1b[201~'
  expect(modifiedInput(paste, result.mods).data).toBe(paste)
  expect(modifiedInput('中文', result.mods).data).toBe('中文')
})
