import { afterEach, beforeEach, expect, test } from 'bun:test'
import { gunzipSync } from 'node:zlib'
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

// scripts/engine-copies.sh, run for real on made-up programs: the release's inyon-engine-* files
// for inyon 0.12 to 0.22, made from the inyon-* ones.
const SCRIPT = path.resolve(import.meta.dir, 'engine-copies.sh')
const MARK = 'inyon: the built-in engine, windowless copy'
// where the PE header starts, and where its subsystem field is from there (signature, file header)
const PE = 0x80
const SUBSYSTEM = PE + 4 + 20 + 68

let dist: string
beforeEach(() => {
  dist = mkdtempSync(path.join(os.tmpdir(), 'engine-copies-'))
})
afterEach(() => rmSync(dist, { recursive: true, force: true }))

/** A Windows console program: the DOS header pointing at a PE header, then the mark. */
function exe(): Buffer {
  const b = Buffer.alloc(0x200)
  b.write('MZ', 0)
  b.writeUInt32LE(PE, 0x3c)
  b.write('PE\0\0', PE, 'latin1')
  b.writeUInt16LE(3, SUBSYSTEM)
  b.write(MARK, 0x100)
  return b
}

function release(skip = '') {
  for (const name of ['inyon-linux-x64', 'inyon-linux-arm64', 'inyon-darwin-x64', 'inyon-darwin-arm64']) {
    if (name !== skip) writeFileSync(path.join(dist, name), `${name} ${MARK}\n`, { mode: 0o755 })
  }
  writeFileSync(path.join(dist, 'inyon-windows-x64.exe'), exe())
  writeFileSync(path.join(dist, 'inyon-windows-arm64.exe'), exe())
}

const run = (...systems: string[]) => Bun.spawnSync(['sh', SCRIPT, dist, ...systems], { stderr: 'pipe' })
const read = (name: string) => readFileSync(path.join(dist, name))

test('each engine name a release from 0.13 to 0.22 carried is made, gzipped too', () => {
  release()
  expect(run().exitCode).toBe(0)
  const engines = readdirSync(dist).filter((f) => f.startsWith('inyon-engine-')).sort()
  const names = [
    'inyon-engine-darwin-arm64',
    'inyon-engine-darwin-x64',
    'inyon-engine-linux-arm64',
    'inyon-engine-linux-arm64-musl',
    'inyon-engine-linux-x64',
    'inyon-engine-linux-x64-musl',
    'inyon-engine-windows-x64.exe',
  ]
  expect(engines).toEqual(names.flatMap((n) => [n, `${n}.gz`]).sort())
  for (const n of names) expect(gunzipSync(read(`${n}.gz`))).toEqual(read(n))
})

test('linux and macOS copies are the same bytes, the static linux build standing in for musl', () => {
  release()
  run()
  for (const arch of ['x64', 'arm64']) {
    expect(read(`inyon-engine-linux-${arch}`)).toEqual(read(`inyon-linux-${arch}`))
    expect(read(`inyon-engine-linux-${arch}-musl`)).toEqual(read(`inyon-linux-${arch}`))
    expect(read(`inyon-engine-darwin-${arch}`)).toEqual(read(`inyon-darwin-${arch}`))
    expect(statSync(path.join(dist, `inyon-engine-linux-${arch}`)).mode & 0o111).toBeTruthy()
  }
})

test('the windows copy opens no console: only the subsystem field changes, and it keeps the mark', () => {
  release()
  run()
  const from = read('inyon-windows-x64.exe')
  const copy = read('inyon-engine-windows-x64.exe')
  expect(copy.length).toBe(from.length)
  expect(copy.readUInt16LE(SUBSYSTEM)).toBe(2)
  const changed = [...copy].flatMap((b, i) => (b === from[i] ? [] : [i]))
  expect(changed).toEqual([SUBSYSTEM])
  expect(copy.includes(MARK)).toBe(true)
  // the inyon.exe itself is left as it was
  expect(from.readUInt16LE(SUBSYSTEM)).toBe(3)
})

test('the same files make the same gzipped copies', () => {
  release()
  run()
  const first = read('inyon-engine-linux-x64.gz')
  rmSync(path.join(dist, 'inyon-engine-linux-x64.gz'))
  run()
  expect(read('inyon-engine-linux-x64.gz')).toEqual(first)
})

test('a missing program or a windows file that is not one stops it', () => {
  release('inyon-darwin-x64')
  const missing = run()
  expect(missing.exitCode).not.toBe(0)
  expect(missing.stderr.toString()).toContain('inyon-darwin-x64 is missing')

  release()
  writeFileSync(path.join(dist, 'inyon-windows-x64.exe'), `not a program ${MARK}`.padEnd(0x200, ' '))
  const bad = run()
  expect(bad.exitCode).not.toBe(0)
  expect(bad.stderr.toString()).toContain("isn't a Windows program")
})

test('named systems get only their copies, and a system with no engine is refused', () => {
  rmSync(dist, { recursive: true, force: true })
  dist = mkdtempSync(path.join(os.tmpdir(), 'engine-copies-'))
  // what the container tests have: one linux build
  writeFileSync(path.join(dist, 'inyon-linux-arm64'), `inyon ${MARK}\n`, { mode: 0o755 })
  expect(run('linux-arm64').exitCode).toBe(0)
  expect(readdirSync(dist).filter((f) => f.startsWith('inyon-engine-')).sort()).toEqual([
    'inyon-engine-linux-arm64',
    'inyon-engine-linux-arm64-musl',
    'inyon-engine-linux-arm64-musl.gz',
    'inyon-engine-linux-arm64.gz',
  ])
  const arm = run('windows-arm64')
  expect(arm.exitCode).not.toBe(0)
  expect(arm.stderr.toString()).toContain('no engine for windows-arm64')
})
