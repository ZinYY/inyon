import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import crypto from 'node:crypto'
import { copyFileSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

// The release pipeline: what each workflow's token may do, and the Android job's re-signing of the
// release's checksum list (.github/scripts/list-apk.sh), run for real against the test key.
const ROOT = path.resolve(import.meta.dir, '..')
const WORKFLOWS = path.join(ROOT, '.github/workflows')

type Perms = Record<string, string> | string | undefined
interface Step {
  run?: string
  uses?: string
  with?: Record<string, unknown>
  env?: Record<string, string>
}
interface Job {
  permissions?: Perms
  uses?: string
  steps?: Step[]
  env?: Record<string, string>
}
interface Workflow {
  permissions?: Perms
  jobs: Record<string, Job>
}

const workflows: Record<string, Workflow> = Object.fromEntries(
  readdirSync(WORKFLOWS)
    .filter((f) => f.endsWith('.yml'))
    .map((f) => [f, Bun.YAML.parse(readFileSync(path.join(WORKFLOWS, f), 'utf8')) as Workflow]),
)

/** the scopes a permissions block grants write on ('write-all' grants everything) */
function writes(p: Perms): string[] {
  if (p === undefined) return ['default']
  if (typeof p === 'string') return p === 'write-all' ? ['all'] : []
  return Object.entries(p)
    .filter(([, v]) => v === 'write')
    .map(([k]) => k)
}

/** The only jobs that may write, and what: each one publishes, and none installs or runs dependencies
 * (the signing jobs may install Ubuntu's minisign package, and nothing else). */
const PUBLISHERS: Record<string, string[]> = {
  'node.yml/publish': ['contents'],
  'android.yml/publish': ['contents'],
  'pages.yml/deploy': ['pages', 'id-token'],
  'npm.yml/publish': ['id-token'],
}
/** steps that run code from outside this repository: dependency installs, builds, tests and the tools they start */
const THIRD_PARTY =
  /\b(bun|npm|pnpm|yarn)\s+(install|i|ci|add)\b|\bbunx?\s+x\b|\bbunx\b|\bbun\s+(run\s+)?(build|test)\b|gradlew|\bcargo\b|\bcap\s+sync\b|\bapt(-get)?\s+install\b|\bpip3?\s+install\b/
/** the one package install a publishing job may run */
const MINISIGN = 'sudo apt-get update -q > /dev/null && sudo apt-get install -y -q minisign > /dev/null'


describe('workflow permissions', () => {
  test('every workflow is read-only unless a job says otherwise', () => {
    for (const [file, wf] of Object.entries(workflows)) {
      expect({ file, writes: writes(wf.permissions) }).toEqual({ file, writes: [] })
    }
  })

  test('only the publishing jobs can write, and they run no third-party code', () => {
    for (const [file, wf] of Object.entries(workflows)) {
      for (const [id, job] of Object.entries(wf.jobs)) {
        const key = `${file}/${id}`
        const granted = writes(job.permissions ?? wf.permissions).sort()
        expect({ key, granted }).toEqual({ key, granted: (PUBLISHERS[key] ?? []).slice().sort() })
        if (!granted.length) continue
        // a job that calls another workflow hands it its token
        expect({ key, uses: job.uses }).toEqual({ key, uses: undefined })
        for (const step of job.steps ?? []) {
          const run = (step.run ?? '').split(MINISIGN).join('')
          expect({ key, run }).not.toEqual({ key, run: expect.stringMatching(THIRD_PARTY) })
        }
      }
    }
  })

  test('every job that calls another workflow passes it a read-only token', () => {
    for (const [file, wf] of Object.entries(workflows)) {
      for (const [id, job] of Object.entries(wf.jobs)) {
        if (job.uses) expect({ key: `${file}/${id}`, writes: writes(job.permissions) }).toEqual({ key: `${file}/${id}`, writes: [] })
      }
    }
  })

  test('the release key is only handed to the jobs that sign', () => {
    const signers: string[] = []
    for (const [file, wf] of Object.entries(workflows)) {
      for (const [id, job] of Object.entries(wf.jobs)) {
        if (JSON.stringify(job).includes('secrets.MINISIGN_SECRET_KEY')) signers.push(`${file}/${id}`)
      }
    }
    expect(signers.sort()).toEqual(['android.yml/publish', 'node.yml/publish'])
  })

  test('checkouts in the publishing jobs keep no credentials on disk', () => {
    for (const key of ['node.yml/publish', 'android.yml/publish']) {
      const [file, id] = key.split('/') as [string, string]
      for (const step of workflows[file]!.jobs[id]!.steps ?? []) {
        if (step.uses?.startsWith('actions/checkout@')) expect({ key, with: step.with }).toEqual({ key, with: { 'persist-credentials': false } })
      }
    }
  })
})

describe('the release list', () => {
  const run = (file: string, job: string) => (workflows[file]!.jobs[job]!.steps ?? []).map((s) => s.run ?? '').join('\n')

  test('node.yml lists the installers with everything else, and requires them', () => {
    const script = run('node.yml', 'publish')
    expect(script).toContain("ls | grep -v -e '^SHA256SUMS' | xargs sha256sum > SHA256SUMS")
    expect(script).not.toContain("'^get\\.'")
    expect(script).toMatch(/want="get\.sh get\.ps1 /)
  })

  test('node.yml makes the inyon-engine copies for older versions before it lists the release', () => {
    const script = run('node.yml', 'publish')
    const copies = script.indexOf('scripts/engine-copies.sh')
    expect(copies).toBeGreaterThan(-1)
    expect(copies).toBeLessThan(script.indexOf('xargs sha256sum > SHA256SUMS'))
    // every name inyon 0.12 to 0.22 asks for is required, with its .gz
    expect(script).toContain('for e in linux-x64 linux-x64-musl linux-arm64 linux-arm64-musl darwin-x64 darwin-arm64 windows-x64.exe; do')
    expect(script).toContain('want="$want inyon-engine-$e inyon-engine-$e.gz"')
    // no Bun build is left to make them
    expect(Object.keys(workflows['node.yml']!.jobs)).not.toContain('engine')
  })

  test("the Android job re-signs only through list-apk.sh, after the APK it uploaded", () => {
    const script = run('android.yml', 'publish')
    const check = script.indexOf('.github/scripts/list-apk.sh')
    expect(check).toBeGreaterThan(script.indexOf('gh release upload "$GITHUB_REF_NAME" "$apk"'))
    expect(script.indexOf('gh release upload "$GITHUB_REF_NAME" "$sums/SHA256SUMS"')).toBeGreaterThan(check)
    // no signing of its own
    expect(script).not.toContain('minisign -S')
  })

  test('node.yml run again on a tag keeps the APK line through keep-apk.sh, before it signs', () => {
    const script = run('node.yml', 'publish')
    const keep = script.indexOf('.github/scripts/keep-apk.sh" "$was" SHA256SUMS')
    expect(keep).toBeGreaterThan(script.indexOf('xargs sha256sum > SHA256SUMS'))
    expect(script.indexOf('minisign -S')).toBeGreaterThan(keep)
  })
})

// ---------- list-apk.sh against the test key ----------

const SCRIPT = path.join(ROOT, '.github/scripts/list-apk.sh')
const KEYS = path.join(ROOT, 'apps/cli/tests/keys')
const PUBKEY = readFileSync(path.join(KEYS, 'test.pub'), 'utf8').split('\n')[1]!
// the test key's file as a secret holds it: the comment line is optional
const KEY = readFileSync(path.join(KEYS, 'test.key'), 'utf8')
const sha = (b: string | Buffer) => crypto.createHash('sha256').update(b).digest('hex')
const SIGNED = !!Bun.which('minisign')

describe.skipIf(!SIGNED)('list-apk.sh', () => {
  let dir: string
  const LIST = `${sha('cli')}  inyon-linux-x64\n${sha('get')}  get.sh\n`
  const APK = 'the app'
  beforeEach(() => {
    dir = mkdtempSync(path.join(os.tmpdir(), 'inyon-list-apk-'))
    writeFileSync(path.join(dir, 'inyon-0.20.0.apk'), APK)
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  async function minisign(...args: string[]) {
    const p = Bun.spawn(['minisign', ...args], { stdin: 'ignore', stdout: 'pipe', stderr: 'pipe' })
    return (await p.exited) === 0
  }
  /** the release as node.yml left it: the list, signed with the test key */
  async function release(list = LIST, signWith = path.join(KEYS, 'test.key')) {
    const sums = path.join(dir, 'sums')
    rmSync(sums, { recursive: true, force: true })
    Bun.spawnSync(['mkdir', '-p', sums])
    writeFileSync(path.join(sums, 'SHA256SUMS'), list)
    expect(await minisign('-S', '-s', signWith, '-m', path.join(sums, 'SHA256SUMS'), '-x', path.join(sums, 'SHA256SUMS.minisig'))).toBe(true)
    // the APK as the release serves it
    copyFileSync(path.join(dir, 'inyon-0.20.0.apk'), path.join(sums, 'inyon-0.20.0.apk'))
    return sums
  }
  async function listApk(sums: string, env: Record<string, string> = {}) {
    const p = Bun.spawn(['/bin/sh', SCRIPT, sums, path.join(dir, 'inyon-0.20.0.apk'), path.join(sums, 'inyon-0.20.0.apk')], {
      env: { PATH: process.env.PATH!, KEY, PUBKEY, ...env },
      stdout: 'pipe',
      stderr: 'pipe',
    })
    const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited])
    return { code, out: out + err }
  }
  const read = (sums: string) => readFileSync(path.join(sums, 'SHA256SUMS'), 'utf8')
  const verifies = (sums: string) => minisign('-V', '-q', '-m', path.join(sums, 'SHA256SUMS'), '-x', path.join(sums, 'SHA256SUMS.minisig'), '-P', PUBKEY)

  test('adds the APK to a signed list and signs it again', async () => {
    const sums = await release()
    const r = await listApk(sums)
    expect(r.out).toContain('Listed inyon-0.20.0.apk')
    expect(r.code).toBe(0)
    expect(read(sums)).toBe(`${LIST}${sha(APK)}  inyon-0.20.0.apk\n`)
    expect(await verifies(sums)).toBe(true)
    // run again (the job re-run), the APK's line is replaced rather than repeated
    expect((await listApk(sums)).code).toBe(0)
    expect(read(sums)).toBe(`${LIST}${sha(APK)}  inyon-0.20.0.apk\n`)
  })

  test('a list someone changed after it was signed is never signed over', async () => {
    const sums = await release()
    // a different program put on the release, its line swapped into the list
    writeFileSync(path.join(sums, 'SHA256SUMS'), LIST.replace(sha('cli'), sha('evil')))
    const before = readFileSync(path.join(sums, 'SHA256SUMS.minisig'), 'utf8')
    const r = await listApk(sums)
    expect(r.code).toBe(1)
    expect(r.out).toContain("isn't signed by the release key, so it isn't signed again")
    expect(read(sums)).toBe(LIST.replace(sha('cli'), sha('evil')))
    expect(readFileSync(path.join(sums, 'SHA256SUMS.minisig'), 'utf8')).toBe(before)
  })

  test('a list signed with another key, or not at all, is refused', async () => {
    // a second key, made here, stands in for anyone else's
    const other = path.join(dir, 'other')
    expect(await minisign('-G', '-W', '-p', `${other}.pub`, '-s', `${other}.key`)).toBe(true)
    const sums = await release(LIST, `${other}.key`)
    expect((await listApk(sums)).code).toBe(1)
    expect(read(sums)).toBe(LIST)
    rmSync(path.join(sums, 'SHA256SUMS.minisig'))
    const r = await listApk(sums)
    expect(r.code).toBe(1)
    expect(r.out).toContain('has no signature')
  })

  test('an APK on the release that is not the one built here is not listed', async () => {
    const sums = await release()
    writeFileSync(path.join(sums, 'inyon-0.20.0.apk'), 'another app')
    const r = await listApk(sums)
    expect(r.code).toBe(1)
    expect(r.out).toContain("isn't the one this job built")
    expect(read(sums)).toBe(LIST)
  })

  test('nothing is signed without both keys', async () => {
    const sums = await release()
    expect((await listApk(sums, { KEY: '' })).code).toBe(1)
    expect((await listApk(sums, { PUBKEY: '' })).code).toBe(1)
    expect(read(sums)).toBe(LIST)
  })
})

// ---------- keep-apk.sh against the test key ----------

describe.skipIf(!SIGNED)('keep-apk.sh', () => {
  let dir: string
  const OLD = `${sha('cli')}  inyon-linux-x64\n${sha('the app')}  inyon-0.20.0.apk\n`
  const NEW = `${sha('cli 2')}  inyon-linux-x64\n`
  beforeEach(() => {
    dir = mkdtempSync(path.join(os.tmpdir(), 'inyon-keep-apk-'))
    writeFileSync(path.join(dir, 'inyon-0.20.0.apk'), 'the app')
    writeFileSync(path.join(dir, 'SHA256SUMS'), OLD)
    Bun.spawnSync(['minisign', '-S', '-s', path.join(KEYS, 'test.key'), '-m', path.join(dir, 'SHA256SUMS'), '-x', path.join(dir, 'SHA256SUMS.minisig')], { stdin: 'ignore' })
    writeFileSync(path.join(dir, 'new'), NEW)
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  async function keep(env: Record<string, string> = {}) {
    const p = Bun.spawn(['/bin/sh', path.join(ROOT, '.github/scripts/keep-apk.sh'), dir, path.join(dir, 'new')], {
      env: { PATH: process.env.PATH!, PUBKEY, ...env },
      stdout: 'pipe',
      stderr: 'pipe',
    })
    const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited])
    return { code, out: out + err, list: readFileSync(path.join(dir, 'new'), 'utf8') }
  }

  test('keeps the APK line from the signed list, once', async () => {
    expect(await keep()).toMatchObject({ code: 0, list: `${NEW}${sha('the app')}  inyon-0.20.0.apk\n` })
    expect((await keep()).list).toBe(`${NEW}${sha('the app')}  inyon-0.20.0.apk\n`)
  })

  test('nothing is kept from a list changed after it was signed, or from an APK swapped since', async () => {
    writeFileSync(path.join(dir, 'SHA256SUMS'), OLD.replace(sha('the app'), sha('evil')))
    writeFileSync(path.join(dir, 'inyon-0.20.0.apk'), 'evil')
    expect(await keep()).toMatchObject({ code: 0, list: NEW })
    writeFileSync(path.join(dir, 'SHA256SUMS'), OLD)
    const r = await keep()
    expect(r).toMatchObject({ code: 0, list: NEW })
    expect(r.out).toContain("doesn't match its signed line")
  })

  test('a first run, with nothing on the release yet, keeps nothing and goes on', async () => {
    rmSync(path.join(dir, 'SHA256SUMS'))
    expect(await keep()).toMatchObject({ code: 0, list: NEW })
    expect((await keep({ PUBKEY: '' })).code).toBe(1)
  })
})
