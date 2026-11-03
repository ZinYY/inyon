import { afterEach, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { authorizeCommand } from './phone.ts'

const homes: string[] = []
afterEach(() => homes.splice(0).forEach((h) => rmSync(h, { recursive: true, force: true })))

const PHONE = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPhone inyon-phone'
function home(keys?: string) {
  const h = mkdtempSync(join(tmpdir(), 'inyon-authorize-'))
  homes.push(h)
  if (keys !== undefined) {
    mkdirSync(join(h, '.ssh'), { mode: 0o700 })
    writeFileSync(join(h, '.ssh', 'authorized_keys'), keys, { mode: 0o600 })
  }
  return h
}
const run = (h: string) => Bun.spawnSync(['sh', '-c', authorizeCommand(PHONE)], { env: { ...process.env, HOME: h } })
const keysOf = (h: string) => readFileSync(join(h, '.ssh', 'authorized_keys'), 'utf8')

test('the command makes ~/.ssh and authorized_keys with the permissions sshd wants, and adds the key once', () => {
  const h = home()
  expect(run(h).exitCode).toBe(0)
  expect(keysOf(h)).toBe(`${PHONE}\n`)
  expect(statSync(join(h, '.ssh')).mode & 0o777).toBe(0o700)
  expect(statSync(join(h, '.ssh', 'authorized_keys')).mode & 0o777).toBe(0o600)
  expect(run(h).exitCode).toBe(0)
  expect(keysOf(h)).toBe(`${PHONE}\n`)
})

test('a last line with no newline after it gets one before the key, so the two never join', () => {
  const h = home('ssh-ed25519 AAAAmine me@laptop')
  expect(run(h).exitCode).toBe(0)
  expect(keysOf(h)).toBe(`ssh-ed25519 AAAAmine me@laptop\n${PHONE}\n`)
  // a file that ends properly, or is empty, gets no blank line
  const h2 = home('ssh-ed25519 AAAAmine me@laptop\n')
  run(h2)
  expect(keysOf(h2)).toBe(`ssh-ed25519 AAAAmine me@laptop\n${PHONE}\n`)
  const h3 = home('')
  run(h3)
  expect(keysOf(h3)).toBe(`${PHONE}\n`)
})
