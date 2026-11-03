import { afterAll, afterEach, beforeEach, describe, expect, test } from 'bun:test'
import crypto from 'node:crypto'
import os from 'node:os'
import path from 'node:path'
import zlib from 'node:zlib'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { releaseFiles } from '@inyon/protocol'
import { type Installer, stampInstaller } from './installers.ts'

// get.sh (and get.ps1, where PowerShell is installed) run for real, against a local server standing
// in for the website's copy of the release, with HOME in a throwaway folder. The "program" it
// installs is a script that records how it was started. By default the script is the website's
// copy, which carries the sha256 of the list it may trust; the plain copy checks the list's
// signature with minisign (the test key, apps/cli/tests/keys) and refuses without it unless told.
const sha = (b: string | Uint8Array) => crypto.createHash('sha256').update(b).digest('hex')
const GET = path.join(import.meta.dir, 'get.sh')
const GET_PS1 = path.join(import.meta.dir, 'get.ps1')
/** this machine as the releases name it */
const SYSTEM = `${process.platform === 'win32' ? 'windows' : process.platform}-${os.arch()}`
const NAME = releaseFiles(SYSTEM).cli
const WINDOWS = 'inyon-windows-x64.exe'
const PROGRAM = '#!/bin/sh\necho "started with $*" > "$HOME/started"\n'
const ENGINE_NAME = releaseFiles(SYSTEM).engine
const KEYS = path.resolve(import.meta.dir, '../apps/cli/tests/keys')
const TEST_KEY = readFileSync(path.join(KEYS, 'test.pub'), 'utf8').split('\n')[1]!
const SIGNED = !!Bun.which('minisign')
const PWSH = Bun.which('pwsh')
/** `split`: a release from 0.13 on, which lists the engine as a file of its own; `apk`: the
 * version whose inyon-<version>.apk the list names, as a real release's does; `latest`: the
 * version latest.json names ('' for none) */
let release = { gz: true, sums: '', split: false, apk: '0.20.0', latest: '0.20.0' }
/** the signature served for the list: a good one, one made over other text, or none at all */
let signature: 'good' | 'bad' | 'none' = 'good'
let requests: string[] = []

async function sign(text: string) {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'inyon-sign-'))
  try {
    const f = path.join(dir, 'SHA256SUMS')
    writeFileSync(f, text)
    const p = Bun.spawn(['minisign', '-S', '-s', path.join(KEYS, 'test.key'), '-m', f, '-x', `${f}.minisig`], { stdin: 'ignore', stdout: 'ignore', stderr: 'pipe' })
    if ((await p.exited) !== 0) throw new Error(`minisign: ${await new Response(p.stderr).text()}`)
    return readFileSync(`${f}.minisig`, 'utf8')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

/** the release's SHA256SUMS as the server hands it out */
function sums() {
  const engine = release.split ? `${sha('engine')}  ${ENGINE_NAME}\n${sha(zlib.gzipSync('engine'))}  ${ENGINE_NAME}.gz\n${sha('musl')}  ${ENGINE_NAME}-musl\n` : ''
  const apk = release.apk ? `${sha('the app')}  inyon-${release.apk}.apk\n` : ''
  const windows = `${sha(PROGRAM)}  ${WINDOWS}\n${sha(zlib.gzipSync(PROGRAM))}  ${WINDOWS}.gz\n`
  return release.sums || `${sha(PROGRAM)}  ${NAME}\n${release.gz ? `${sha(zlib.gzipSync(PROGRAM))}  ${NAME}.gz\n` : ''}${engine}${windows}${apk}`
}

const server = Bun.serve({
  port: 0,
  hostname: '127.0.0.1',
  async fetch(req) {
    const { pathname, host } = new URL(req.url)
    // GitHub's newest release redirects to its tag
    if (pathname.endsWith('/releases/latest')) {
      requests.push('releases/latest')
      return Response.redirect(`http://${host}/releases/tag/v${release.latest}`, 302)
    }
    // the file's name, wherever the folder is (the website's /download, or a GitHub tag)
    const name = pathname.slice(pathname.lastIndexOf('/') + 1)
    requests.push(name)
    if (name === 'SHA256SUMS') return new Response(sums())
    if (name === 'SHA256SUMS.minisig' && signature !== 'none') return new Response(await sign(signature === 'bad' ? `${sums()}tampered\n` : sums()))
    if (name === 'latest.json' && release.latest) return Response.json({ version: release.latest, tag: `v${release.latest}` })
    if (name === NAME || name === WINDOWS) return new Response(PROGRAM)
    if ((name === `${NAME}.gz` && release.gz) || name === `${WINDOWS}.gz`) return new Response(zlib.gzipSync(PROGRAM))
    return new Response('not found', { status: 404 })
  },
})
afterAll(() => server.stop(true))

let home: string
beforeEach(() => {
  home = mkdtempSync(path.join(os.tmpdir(), 'inyon-get-'))
  release = { gz: true, sums: '', split: false, apk: '0.20.0', latest: '0.20.0' }
  signature = 'good'
  requests = []
})
afterEach(() => rmSync(home, { recursive: true, force: true }))

/**
 * The installer to run: the website's copy, with the sha256 of the list it may trust filled in
 * (by default the list the server hands out right now), or the plain one from the repository.
 */
function installer(name: Installer, script: 'site' | 'plain', listSha256 = sha(sums())) {
  const src = readFileSync(name === 'get.sh' ? GET : GET_PS1, 'utf8')
  if (script === 'plain') return name === 'get.sh' ? GET : GET_PS1
  const f = path.join(home, `site-${name}`)
  writeFileSync(f, stampInstaller(name, src, listSha256))
  return f
}

// not spawnSync: the server answering the script runs on this same thread
async function run(env: Record<string, string> = {}, args: string[] = [], script: 'site' | 'plain' | string = 'site') {
  const file = script === 'site' || script === 'plain' ? installer('get.sh', script) : script
  const p = Bun.spawn(['/bin/sh', file, ...args], {
    env: { ...process.env, HOME: home, INYON_DOWNLOAD: `http://127.0.0.1:${server.port}`, INYON_SIGNING_KEY: TEST_KEY, ...env },
    stdout: 'pipe',
    stderr: 'pipe',
  })
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited])
  return { code, out: out + err }
}
const installed = () => path.join(home, '.inyon', 'bin', 'inyon')
const started = () => readFileSync(path.join(home, 'started'), 'utf8')
/** a PATH with only these tools on it (get.sh needs uname, curl, awk, cat, grep, a sha256 and the file tools) */
function tools(names: string[]) {
  const bin = path.join(home, 'tools')
  mkdirSync(bin, { recursive: true })
  for (const tool of names) {
    const found = Bun.which(tool)
    if (found && !existsSync(path.join(bin, tool))) symlinkSync(found, path.join(bin, tool))
  }
  return bin
}
const TOOLS = ['uname', 'mkdir', 'curl', 'awk', 'cat', 'grep', 'sha256sum', 'shasum', 'cut', 'rm', 'chmod', 'mv']
const REFUSED_UNSIGNED = "isn't signed by Inyon's release key, so nothing was installed"
const NO_MINISIGN = "because minisign isn't installed, so nothing was installed"

test('a release from 0.13 on: the command goes in, its setup gets the engine, and --hub is a setup role', async () => {
  release.split = true
  const r = await run()
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(started()).toBe('started with setup\n')
  // only the command is downloaded here; the engine is the command's to fetch
  expect(requests).toEqual(['SHA256SUMS', `${NAME}.gz`])
  expect(existsSync(path.join(home, '.inyon', 'bin', 'inyon-engine'))).toBe(false)

  expect((await run({}, ['--hub'])).code).toBe(0)
  expect(started()).toBe('started with setup --role hub\n')
})

test('a release from before 0.13 is one program, set up as it always was', async () => {
  expect((await run({}, ['--hub'])).code).toBe(0)
  expect(started()).toBe('started with hub install\n')
  expect((await run({}, ['--nope'])).code).toBe(2)
})

test('gets the gzipped file when the release lists one, checks it, and starts setup with the unpacked program', async () => {
  const r = await run()
  expect(r.out).toContain(`Downloading ${NAME}.gz`)
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(readFileSync(path.join(home, 'started'), 'utf8')).toBe('started with setup\n')
  expect(requests).toEqual(['SHA256SUMS', `${NAME}.gz`])
  expect(existsSync(`${installed()}.download.gz`)).toBe(false)
})

test('a release without a .gz (before 0.11) is downloaded plain', async () => {
  release.gz = false
  const r = await run()
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(requests).toEqual(['SHA256SUMS', NAME])
})

test('without gzip on the computer the plain file is taken, so nothing else is needed', async () => {
  // a PATH with every tool get.sh uses but gzip (and minisign: the notice is tested below)
  const r = await run({ PATH: tools(TOOLS) })
  expect(r.out).toContain(`Downloading ${NAME}…`)
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(requests).toEqual(['SHA256SUMS', NAME])
})

test("the website's copy trusts the list it was published with, without minisign", async () => {
  const r = await run({ PATH: tools([...TOOLS, 'gzip']) })
  expect(r.code).toBe(0)
  expect(r.out).not.toContain('signature')
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(requests).toEqual(['SHA256SUMS', `${NAME}.gz`])
})

test('without minisign the plain copy installs nothing, unless told to go on without the signature', async () => {
  const PATH = tools([...TOOLS, 'gzip'])
  const r = await run({ PATH }, [], 'plain')
  expect(r.code).toBe(1)
  expect(r.out).toContain(NO_MINISIGN)
  expect(r.out).toContain('--no-signature-check')
  expect(existsSync(installed())).toBe(false)
  expect(existsSync(path.join(home, '.inyon', 'bin', 'SHA256SUMS.download'))).toBe(false)
  expect(requests).toEqual(['SHA256SUMS'])

  for (const [env, args] of [[{}, ['--no-signature-check']], [{ INYON_NO_SIGNATURE_CHECK: '1' }, []]] as const) {
    rmSync(path.join(home, '.inyon'), { recursive: true, force: true })
    const told = await run({ PATH, ...env }, [...args], 'plain')
    expect(told.code).toBe(0)
    expect(told.out).toContain("(the checksum list's signature wasn't checked, as asked)")
    expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
    expect(started()).toBe('started with setup\n')
  }
  // the options mix
  expect((await run({ PATH }, ['--no-signature-check', '--hub'], 'plain')).code).toBe(0)
  expect(started()).toBe('started with hub install\n')
})

test.skipIf(!SIGNED)('with minisign the list must carry a good signature, from the key downloads are pointed at', async () => {
  const refused = async (env: Record<string, string> = {}, args: string[] = []) => {
    const r = await run(env, args, 'plain')
    expect(r.code).toBe(1)
    expect(r.out).toContain(REFUSED_UNSIGNED)
    expect(existsSync(installed())).toBe(false)
    expect(requests).toEqual(['SHA256SUMS', 'SHA256SUMS.minisig'])
    expect(existsSync(path.join(home, '.inyon', 'bin', 'SHA256SUMS.download'))).toBe(false)
    requests = []
  }
  signature = 'bad'
  await refused()
  // going on without the signature is for when nothing can check it, never past a bad one
  await refused({}, ['--no-signature-check'])
  signature = 'none'
  await refused()
  // the test key only stands in because downloads are pointed away from the website; the release
  // key is the default, and this list isn't signed with it
  signature = 'good'
  await refused({ INYON_SIGNING_KEY: '' })
  expect((await run({}, [], 'plain')).code).toBe(0)
  expect(requests).toEqual(['SHA256SUMS', 'SHA256SUMS.minisig', 'latest.json', `${NAME}.gz`])
  // the website's copy pointed at another list checks that one's signature instead
  requests = []
  expect((await run({}, [], installer('get.sh', 'site', sha('another list')))).code).toBe(0)
  expect(requests).toEqual(['SHA256SUMS', 'SHA256SUMS.minisig', 'latest.json', `${NAME}.gz`])
})

test.skipIf(!SIGNED)('a signed list has to be the one for the version asked for', async () => {
  const refused = async (base: string, says: string) => {
    const r = await run({ INYON_DOWNLOAD: `http://127.0.0.1:${server.port}${base}` }, [], 'plain')
    expect(r.code).toBe(1)
    expect(r.out).toContain(says)
    expect(existsSync(installed())).toBe(false)
    expect(existsSync(path.join(home, '.inyon', 'bin', 'SHA256SUMS.download'))).toBe(false)
  }
  const installs = async (base: string) => {
    const r = await run({ INYON_DOWNLOAD: `http://127.0.0.1:${server.port}${base}` }, [], 'plain')
    expect(r.code).toBe(0)
    expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
    rmSync(installed())
  }
  // an older release's list, signed as well, where latest.json names a newer one
  release.apk = '0.19.0'
  await refused('/download', "isn't the one for Inyon 0.20.0 (it doesn't list inyon-0.20.0.apk), so nothing was installed")
  // a build's list (node-latest) names no APK at all
  release.apk = ''
  await refused('/download', "isn't the one for Inyon 0.20.0")
  // a mirror that doesn't say which version it serves
  release.apk = '0.20.0'
  release.latest = ''
  await refused('/download', "Couldn't tell which version")
  // a GitHub tag asks for its own version, and GitHub's newest for the tag it redirects to
  release.latest = '0.20.0'
  await installs('/releases/download/v0.20.0')
  await refused('/releases/download/v0.21.0', "isn't the one for Inyon 0.21.0")
  await installs('/releases/latest/download')
  release.latest = '0.21.0'
  await refused('/releases/latest/download', "isn't the one for Inyon 0.21.0")
  // a tag that isn't a version asks for that build
  release.apk = ''
  await installs('/releases/download/node-latest')
})

const SITE = 'https://inyon.tech/download/'
/** the website's earlier address, which now redirects to the same path on inyon.tech */
const OLD_SITE = 'https://inyon.abhashchakraborty.tech/download/'
const GITHUB = 'https://github.com/Abhash-Chakraborty/inyon/releases/'
/** addresses that only look like the website's */
const LOOKALIKES = ['https://inyon.abhashchakraborty.tech.example.com/download/', 'https://xinyon.tech/download/', 'https://inyon.tech.example.com/download/']

/**
 * A curl in front of the real one (put first on `PATH`, or into `bin`): it notes each address on
 * the website (at either address), GitHub or one of the lookalikes and fetches that file from the
 * local server instead, so nothing leaves this machine. Returns the addresses noted so far.
 */
function realAddresses(bin = tools([])) {
  const asked = path.join(home, 'asked')
  rmSync(path.join(bin, 'curl'), { force: true })
  writeFileSync(
    path.join(bin, 'curl'),
    `#!/bin/sh
n=$#
for a in "$@"; do
  case "$a" in ${[SITE, OLD_SITE, SITE.replace('https:', 'http:'), OLD_SITE.replace('https:', 'http:'), GITHUB, ...LOOKALIKES].map((p) => `${p}*`).join(' | ')}) echo "$a" >> "${asked}"; a="http://127.0.0.1:${server.port}/\${a##*/}" ;; esac
  set -- "$@" "$a"
done
shift $n
exec ${Bun.which('curl')} "$@"
`,
    { mode: 0o755 },
  )
  return { bin, asked: () => (existsSync(asked) ? readFileSync(asked, 'utf8').trim().split('\n') : []) }
}

test('by default the release comes from the website', async () => {
  const curl = realAddresses()
  const r = await run({ PATH: `${curl.bin}:${process.env.PATH}`, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: '', INYON_SIGNING_KEY: '' })
  expect(r.code).toBe(0)
  expect(curl.asked()).toEqual([`${SITE}SHA256SUMS`, `${SITE}${NAME}.gz`])
})

test("the website's copy refuses a list other than its own when nothing can check the signature", async () => {
  const curl = realAddresses(tools([...TOOLS, 'gzip']))
  const r = await run({ PATH: curl.bin, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: '' }, [], installer('get.sh', 'site', sha('the next release')))
  expect(r.code).toBe(1)
  expect(r.out).toContain("isn't the one this installer was published with")
  expect(existsSync(installed())).toBe(false)
  expect(curl.asked()).toEqual([`${SITE}SHA256SUMS`])
  // pointed somewhere else it says what it needs to check that list
  const elsewhere = await run({ PATH: tools([...TOOLS, 'gzip']) }, [], installer('get.sh', 'site', sha('the next release')))
  expect(elsewhere.code).toBe(1)
  expect(elsewhere.out).toContain(NO_MINISIGN)
})

test.skipIf(!SIGNED)('a signing key from the environment is ignored for the website and GitHub, named or not', async () => {
  const curl = realAddresses()
  const PATH = `${curl.bin}:${process.env.PATH}`
  // INYON_SIGNING_KEY names the test key, which signed this list, every time
  for (const INYON_DOWNLOAD of ['', SITE, SITE.slice(0, -1), `${GITHUB}latest/download`, `${GITHUB}download/v0.14.0`]) {
    const r = await run({ PATH, INYON_DOWNLOAD, TETHER_DOWNLOAD: '' }, [], 'plain')
    expect(r.code).toBe(1)
    expect(r.out).toContain(REFUSED_UNSIGNED)
    expect(existsSync(installed())).toBe(false)
  }
  // TETHER_DOWNLOAD, the name from before the rename, is no different
  expect((await run({ PATH, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: SITE }, [], 'plain')).code).toBe(1)
  // somewhere else, it counts (run() points INYON_DOWNLOAD at the local server)
  expect((await run({}, [], 'plain')).code).toBe(0)
})

test.skipIf(!SIGNED)("the website's earlier address is still the website, and lookalikes of either address aren't", async () => {
  const curl = realAddresses()
  const PATH = `${curl.bin}:${process.env.PATH}`
  // INYON_SIGNING_KEY names the test key, which signed this list: ignored for the website at
  // either address, over https or http
  for (const INYON_DOWNLOAD of [OLD_SITE, OLD_SITE.slice(0, -1), OLD_SITE.replace('https:', 'http:'), SITE.replace('https:', 'http:')]) {
    const r = await run({ PATH, INYON_DOWNLOAD, TETHER_DOWNLOAD: '' }, [], 'plain')
    expect(r.code).toBe(1)
    expect(r.out).toContain(REFUSED_UNSIGNED)
    expect(existsSync(installed())).toBe(false)
  }
  // an address that only looks like the website's is somewhere else, where the key counts
  for (const INYON_DOWNLOAD of LOOKALIKES.map((l) => l.slice(0, -1))) {
    const r = await run({ PATH, INYON_DOWNLOAD, TETHER_DOWNLOAD: '' }, [], 'plain')
    expect(r.code).toBe(0)
    rmSync(installed())
  }
  // and each was asked for the list
  for (const base of [OLD_SITE, ...LOOKALIKES]) expect(curl.asked()).toContain(`${base}SHA256SUMS`)
})

test("without gzip, the website's earlier address also takes the plain file from GitHub", async () => {
  release.apk = '0.14.0'
  const curl = realAddresses(tools(TOOLS))
  const r = await run({ PATH: curl.bin, INYON_DOWNLOAD: OLD_SITE.slice(0, -1), TETHER_DOWNLOAD: '' })
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(curl.asked()).toEqual([`${OLD_SITE}SHA256SUMS`, `${GITHUB}download/v0.14.0/${NAME}`])
})

test('get.ps1 takes the website at either address as official, and nothing that only looks like it', () => {
  // the same check get.ps1 makes; PowerShell's -match ignores case, as the i flag does here
  const line = readFileSync(path.join(import.meta.dir, 'get.ps1'), 'utf8').match(/^\$official = \$base -match '([^']+)'$/m)
  expect(line).not.toBeNull()
  const official = new RegExp(line![1]!, 'i')
  for (const base of [SITE, SITE.slice(0, -1), 'https://inyon.tech', 'http://inyon.tech/download', 'HTTPS://Inyon.Tech/download', OLD_SITE, 'http://inyon.abhashchakraborty.tech/download', 'https://inyon.abhashchakraborty.tech', `${GITHUB}latest/download`]) {
    expect(official.test(base)).toBe(true)
  }
  for (const base of [...LOOKALIKES, 'http://127.0.0.1:8000/download', 'https://inyon.abhashchakraborty.technology/download', 'https://github.com/someone-else/inyon/releases/latest/download']) {
    expect(official.test(base)).toBe(false)
  }
})

test("without gzip, the website's release takes the plain file from GitHub, checked against the same list", async () => {
  release.apk = '0.14.0'
  const curl = realAddresses(tools(TOOLS))
  const r = await run({ PATH: curl.bin, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: '' })
  expect(r.code).toBe(0)
  expect(readFileSync(installed(), 'utf8')).toBe(PROGRAM)
  expect(curl.asked()).toEqual([`${SITE}SHA256SUMS`, `${GITHUB}download/v0.14.0/${NAME}`])
  // a list without the APK doesn't say which tag: the plain file is asked of the website
  release.apk = ''
  rmSync(path.join(home, 'asked'))
  expect((await run({ PATH: curl.bin, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: '' })).code).toBe(0)
  expect(curl.asked()).toEqual([`${SITE}SHA256SUMS`, `${SITE}${NAME}`])
  // the plain file from GitHub must match the list like any other
  release.apk = '0.14.0'
  release.sums = `${sha('tampered')}  ${NAME}\n${sha('the app')}  inyon-0.14.0.apk\n`
  rmSync(installed())
  const bad = await run({ PATH: curl.bin, INYON_DOWNLOAD: '', TETHER_DOWNLOAD: '' })
  expect(bad.code).toBe(1)
  expect(bad.out).toContain("didn't match the release's checksum")
  expect(existsSync(installed())).toBe(false)
})

test('on musl (Alpine) nothing asks for libstdc++, not even the retired Bun hub switch', async () => {
  release.split = true
  // an ldd that answers as musl's does, in front of everything else
  const bin = tools([])
  writeFileSync(path.join(bin, 'ldd'), '#!/bin/sh\necho "musl libc (aarch64)"\n', { mode: 0o755 })
  const r = await run({ PATH: `${bin}:${process.env.PATH}` })
  expect(r.code).toBe(0)
  expect(started()).toBe('started with setup\n')
  // from 0.19 a computer that isn't a hub runs on the static inyon alone
  expect(r.out).not.toContain('libstdc++')
  // from 0.21 the hub runs on the inyon command too
  const hub = await run({ PATH: `${bin}:${process.env.PATH}` }, ['--hub'])
  expect(hub.code).toBe(0)
  expect(started()).toBe('started with setup --role hub\n')
  expect(hub.out).not.toContain('libstdc++')
  const bunHub = await run({ PATH: `${bin}:${process.env.PATH}`, INYON_HUB_IMPL: 'bun' }, ['--hub'])
  expect(bunHub.code).toBe(0)
  // from 0.23 the Bun hub is gone, and the static inyon needs neither library
  expect(bunHub.out).not.toContain('libstdc++')
  // a release without a musl engine is taken for one from before the split
  release.sums = `${sha(PROGRAM)}  ${NAME}\n${sha('engine')}  ${ENGINE_NAME}\n`
  expect((await run({ PATH: `${bin}:${process.env.PATH}` }, ['--hub'])).code).toBe(0)
  expect(started()).toBe('started with hub install\n')
})

test('a .gz that is not the one listed is refused before it is unpacked', async () => {
  release.sums = `${sha(PROGRAM)}  ${NAME}\n${sha('tampered')}  ${NAME}.gz\n`
  const r = await run()
  expect(r.code).toBe(1)
  expect(r.out).toContain("didn't match the release's checksum")
  expect(existsSync(installed())).toBe(false)
  expect(existsSync(`${installed()}.download.gz`)).toBe(false)
  expect(requests).toEqual(['SHA256SUMS', `${NAME}.gz`])
})

test('an unpacked file that does not match the plain checksum is refused too', async () => {
  release.sums = `${sha('tampered')}  ${NAME}\n${sha(zlib.gzipSync(PROGRAM))}  ${NAME}.gz\n`
  const r = await run()
  expect(r.code).toBe(1)
  expect(r.out).toContain("didn't match the release's checksum")
  expect(existsSync(installed())).toBe(false)
  expect(existsSync(`${installed()}.download`)).toBe(false)
})

// get.ps1 under PowerShell 7 (GitHub's Linux runners have it): everything up to starting setup,
// which Linux can't do with an inyon.exe, so a good run is one that leaves the program in place
// PowerShell takes about a second to start, so these get a minute each
describe.skipIf(!PWSH)('get.ps1', () => {
  const exe = () => path.join(home, '.inyon', 'bin', 'inyon.exe')
  async function ps1(env: Record<string, string> = {}, script: 'site' | 'plain' | string = 'site') {
    const file = script === 'site' || script === 'plain' ? installer('get.ps1', script) : script
    const p = Bun.spawn([PWSH!, '-NoProfile', '-NonInteractive', '-File', file], {
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        INYON_DOWNLOAD: `http://127.0.0.1:${server.port}`,
        INYON_SIGNING_KEY: TEST_KEY,
        INYON_NO_MODIFY_PATH: '1',
        ...env,
      },
      stdout: 'pipe',
      stderr: 'pipe',
    })
    const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited])
    // PowerShell colours an error and wraps it in a box; read it as one plain line
    const plain = (out + err).replace(/\x1b\[[0-9;]*m/g, '').replace(/\s*\|\s+/g, ' ')
    return { code, out: plain.replace(/\s+/g, ' ') }
  }
  const got = () => existsSync(exe()) && readFileSync(exe(), 'utf8') === PROGRAM
  const leftovers = () => existsSync(`${exe()}.sums`) || existsSync(`${exe()}.sums.minisig`)
  /** a PATH without minisign on it (PowerShell itself needs nothing from it) */
  const bare = () => tools([])

  test("the website's copy trusts the list it was published with, without minisign", async () => {
    const r = await ps1({ PATH: bare() })
    expect(got()).toBe(true)
    expect(r.out).not.toContain('nothing was installed')
    expect(requests).toEqual(['SHA256SUMS', `${WINDOWS}.gz`])
  }, 60_000)

  test('without minisign the plain copy installs nothing, unless INYON_NO_SIGNATURE_CHECK says to', async () => {
    const r = await ps1({ PATH: bare() }, 'plain')
    expect(r.code).not.toBe(0)
    expect(r.out).toContain(NO_MINISIGN)
    expect(r.out).toContain("$env:INYON_NO_SIGNATURE_CHECK = '1'")
    expect(existsSync(exe())).toBe(false)
    expect(leftovers()).toBe(false)
    expect(requests).toEqual(['SHA256SUMS'])
    // the website's copy pointed at another release is no different
    expect((await ps1({ PATH: bare() }, installer('get.ps1', 'site', sha('another list')))).out).toContain(NO_MINISIGN)

    const told = await ps1({ PATH: bare(), INYON_NO_SIGNATURE_CHECK: '1' }, 'plain')
    expect(told.out).toContain("(the checksum list's signature wasn't checked, as INYON_NO_SIGNATURE_CHECK asked)")
    expect(got()).toBe(true)
  }, 60_000)

  test.skipIf(!SIGNED)('with minisign the list must carry a good signature and be the one for the version asked for', async () => {
    const refused = async (says: string, env: Record<string, string> = {}) => {
      requests = []
      const r = await ps1(env, 'plain')
      expect(r.code).not.toBe(0)
      expect(r.out).toContain(says)
      expect(existsSync(exe())).toBe(false)
      expect(leftovers()).toBe(false)
    }
    const installs = async (env: Record<string, string> = {}) => {
      requests = []
      await ps1(env, 'plain')
      expect(got()).toBe(true)
      rmSync(exe())
    }
    const at = (base: string) => ({ INYON_DOWNLOAD: `http://127.0.0.1:${server.port}${base}` })

    await installs()
    expect(requests).toEqual(['SHA256SUMS', 'SHA256SUMS.minisig', 'latest.json', `${WINDOWS}.gz`])
    signature = 'bad'
    await refused(REFUSED_UNSIGNED)
    await refused(REFUSED_UNSIGNED, { INYON_NO_SIGNATURE_CHECK: '1' })
    signature = 'good'
    await refused(REFUSED_UNSIGNED, { INYON_SIGNING_KEY: '' })

    release.apk = '0.19.0'
    await refused("isn't the one for Inyon 0.20.0 (it doesn't list inyon-0.20.0.apk), so nothing was installed")
    release.apk = '0.20.0'
    release.latest = ''
    await refused("Couldn't tell which version")
    release.latest = '0.20.0'
    await installs(at('/releases/download/v0.20.0'))
    await refused("isn't the one for Inyon 0.21.0", at('/releases/download/v0.21.0'))
    await installs(at('/releases/latest/download'))
    release.latest = '0.21.0'
    await refused("isn't the one for Inyon 0.21.0", at('/releases/latest/download'))
    release.apk = ''
    await installs(at('/releases/download/node-latest'))
  }, 60_000)
})

describe('inyon on the PATH', () => {
  const MARK = '# added by Inyon'
  const read = (rel: string) => (existsSync(path.join(home, rel)) ? readFileSync(path.join(home, rel), 'utf8') : '')
  const lines = (rel: string) => read(rel).split('\n').filter((l) => l.includes(MARK))

  test('zsh gets one marked line in ~/.zshrc, once, after what was there', async () => {
    writeFileSync(path.join(home, '.zshrc'), 'alias ll="ls -l"') // no newline at the end
    const r = await run({ SHELL: '/bin/zsh' })
    expect(r.code).toBe(0)
    expect(read('.zshrc')).toBe(`alias ll="ls -l"\nexport PATH="$HOME/.inyon/bin:$PATH" ${MARK}\n`)
    expect(r.out).toContain('Added inyon to your PATH in ~/.zshrc, so new terminals find it by name.')
    expect(r.out).toContain('export PATH="$HOME/.inyon/bin:$PATH"')
    // running it again changes nothing
    const again = await run({ SHELL: '/bin/zsh' })
    expect(lines('.zshrc')).toHaveLength(1)
    expect(again.out).not.toContain('Added inyon to your PATH')
  })

  test('zsh with ZDOTDIR uses the .zshrc there', async () => {
    const r = await run({ SHELL: '/usr/bin/zsh', ZDOTDIR: path.join(home, '.config', 'zsh') })
    expect(r.code).toBe(0)
    expect(lines('.config/zsh/.zshrc')).toHaveLength(1)
    expect(read('.zshrc')).toBe('')
  })

  test('bash gets ~/.bashrc (and ~/.bash_profile on a Mac), fish a file of its own, anything else ~/.profile', async () => {
    expect((await run({ SHELL: '/bin/bash' })).code).toBe(0)
    expect(lines('.bashrc')).toEqual([`export PATH="$HOME/.inyon/bin:$PATH" ${MARK}`])
    expect(lines('.bash_profile')).toHaveLength(process.platform === 'darwin' ? 1 : 0)

    expect((await run({ SHELL: '/usr/local/bin/fish' })).code).toBe(0)
    expect(read('.config/fish/conf.d/inyon.fish')).toBe(`contains $HOME/.inyon/bin $PATH; or set -gx PATH $HOME/.inyon/bin $PATH ${MARK}\n`)

    expect((await run({ SHELL: '/bin/dash' })).code).toBe(0)
    expect(lines('.profile')).toHaveLength(1)
  })

  test('nothing changes when it is already on the PATH, or with INYON_NO_MODIFY_PATH=1', async () => {
    const onPath = await run({ SHELL: '/bin/zsh', PATH: `${path.join(home, '.inyon', 'bin')}:${process.env.PATH}` })
    expect(onPath.code).toBe(0)
    expect(existsSync(path.join(home, '.zshrc'))).toBe(false)

    const optOut = await run({ SHELL: '/bin/zsh', INYON_NO_MODIFY_PATH: '1' })
    expect(optOut.code).toBe(0)
    expect(existsSync(path.join(home, '.zshrc'))).toBe(false)
    expect(optOut.out).not.toContain('PATH')
  })

  test('the line it writes works: a new shell finds inyon by name', async () => {
    await run({ SHELL: '/bin/dash' })
    const p = Bun.spawn(['/bin/sh', '-c', '. "$HOME/.profile"; command -v inyon'], { env: { HOME: home, PATH: '/usr/bin:/bin' }, stdout: 'pipe' })
    expect((await new Response(p.stdout).text()).trim()).toBe(path.join(home, '.inyon', 'bin', 'inyon'))
  })
})
