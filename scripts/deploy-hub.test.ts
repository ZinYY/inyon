import { describe, expect, test } from 'bun:test'
import { statSync, readFileSync, existsSync } from 'node:fs'
import path from 'node:path'
import { RELEASE_KEY, VERSION } from '@inyon/protocol'

// The hub's container (deploy/hub) and the Render Blueprint that builds it. The image downloads
// the release its Dockerfile names, so that has to move with each release; the contract test
// (deploy/hub/test/contract.sh, in deploy-hub.yml) checks the running image itself.
const ROOT = path.resolve(import.meta.dir, '..')
const read = (f: string) => readFileSync(path.join(ROOT, f), 'utf8')
const dockerfile = read('deploy/hub/Dockerfile')
const arg = (name: string) => dockerfile.match(new RegExp(`^ARG ${name}=(.*)$`, 'm'))?.[1]

describe('the hub image', () => {
  test('downloads the release this checkout is, as a release bumps package.json and Cargo.toml', () => {
    const pkg = JSON.parse(read('package.json'))
    const cargo = read('Cargo.toml').match(/^\[workspace\.package\][^[]*?^version = "(.*)"$/m)?.[1]
    expect(pkg.version).toBe(VERSION)
    expect(cargo).toBe(VERSION)
    expect(arg('INYON_VERSION')).toBe(VERSION)
  })

  test('trusts the release only under the release key', () => {
    expect(arg('RELEASE_PUBLIC_KEY')).toBe(RELEASE_KEY)
    expect(dockerfile).toContain('minisign -V -P "$RELEASE_PUBLIC_KEY" -m SHA256SUMS -x SHA256SUMS.minisig')
    expect(dockerfile).toContain('sha256sum -c want')
    // no checksum pinned by hand, which a release would have to update too
    expect(dockerfile).not.toMatch(/\b[0-9a-f]{64}\b/)
  })

  test('has a release file for both systems the hosts run', () => {
    expect(dockerfile).toContain('amd64) file=inyon-linux-x64;;')
    expect(dockerfile).toContain('arm64) file=inyon-linux-arm64;;')
  })

  test('starts as root only to hand over the volume, and its scripts are executable', () => {
    expect(dockerfile).not.toMatch(/^USER /m)
    expect(read('deploy/hub/boot.sh')).toContain('exec setpriv --reuid=1000 --regid=1000')
    for (const f of ['deploy/hub/boot.sh', 'deploy/hub/test/contract.sh']) expect(statSync(path.join(ROOT, f)).mode & 0o111).toBe(0o111)
  })
})

describe('the Render Blueprint', () => {
  const blueprint = Bun.YAML.parse(read('render.yaml')) as { services: Record<string, any>[] }
  const [hub] = blueprint.services

  test('builds deploy/hub as a Docker web service', () => {
    expect(blueprint.services).toHaveLength(1)
    expect(hub).toMatchObject({ type: 'web', runtime: 'docker', dockerfilePath: './deploy/hub/Dockerfile', dockerContext: './deploy/hub' })
    expect(existsSync(path.join(ROOT, hub!.dockerfilePath))).toBe(true)
  })

  test('keeps the home folder on a disk, which needs a paid instance', () => {
    expect(hub!.disk.mountPath).toBe('/data')
    expect(dockerfile).toMatch(/^ENV HOME=\/data\/home /m)
    expect(hub!.plan).not.toBe('free')
  })

  test('checks the health path Caddy answers, and makes the password', () => {
    expect(hub!.healthCheckPath).toBe('/healthz')
    expect(read('deploy/hub/Caddyfile')).toContain('path /healthz')
    expect(hub!.envVars).toContainEqual({ key: 'INYON_PASSWORD', generateValue: true })
  })

  test("doesn't redeploy every hub made from it on each push here", () => {
    // a bare off is YAML for false, which Render's schema refuses
    expect(hub!.autoDeployTrigger).toBe('off')
  })
})
