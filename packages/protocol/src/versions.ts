// The product's name and addresses, and release numbers for "Update available" checks.

/** What the product is called, everywhere a person reads it. One place, so a rename changes it once. */
export const PRODUCT = 'Inyon'
/** The lowercase form, for folders, packages, services and storage keys */
export const SLUG = 'inyon'
/** The name it had before, which migration code still reads (folders, env vars, storage keys) */
export const OLD_SLUG = 'tether'
export const SOURCE_REPO = 'Abhash-Chakraborty/inyon'
/**
 * The download page (static, on GitHub Pages at its own domain). A computer's QR code opens it
 * when scanned with a phone's camera, and it offers to copy the pairing link for the app.
 */
export const WEB_URL = 'https://inyon.tech/'
/** The short address the one-line installers are typed with: …/get.sh and …/get.ps1 */
export const INSTALL_URL = 'https://inyon.tech'
/** The Android application id */
export const APP_ID = 'tech.abhashchakraborty.inyon'

/** The repository every release is published on. */
export const RELEASES_REPO = SOURCE_REPO
/** The minisign key every release's SHA256SUMS is signed with (key id 0F27F489FF0BBD53). */
export const RELEASE_KEY = 'RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6'
/**
 * The website's copy of the newest release, where the app and the inyon program are downloaded
 * from. Older releases stay on GitHub only.
 */
export const DOWNLOAD_URL = `${INSTALL_URL}/download`
/** What that copy is: {"version":"0.14.0","tag":"v0.14.0","date":…,"files":[{name,size,sha256}…]} */
export const LATEST_URL = `${DOWNLOAD_URL}/latest.json`

/**
 * The files a release attaches for one system ("linux-x64", "windows-x64"). Release 0.13 depends
 * on these names: from then on `cli` is a small command-line program and today's program ships as
 * `engine`. Every updater from 0.11 on looks for the engine's name in SHA256SUMS and installs both
 * files when it's there, so a machine crosses over by itself. Change the names only with that in
 * mind. Each file is also attached gzipped, as its name plus .gz (see compressed); the plain file
 * stays for installs from before 0.11.
 */
export function releaseFiles(system: string) {
  const exe = system.startsWith('windows') ? '.exe' : ''
  return { cli: `inyon-${system}${exe}`, engine: `inyon-engine-${system}${exe}` }
}
/** The gzipped copy of a release file, under half the size; taken first when SHA256SUMS lists it. */
export const compressed = (file: string) => `${file}.gz`
/** The engine's name once installed, next to `inyon` in ~/.inyon/bin (plus .exe on Windows). */
export const ENGINE = 'inyon-engine'

/**
 * A release number as [major, minor, patch]: "0.6.10" and "v0.6.10" both work. A prerelease
 * ("0.7.0-rc.1") or anything else gives undefined, so it never counts as an update.
 */
export function parseRelease(v: string): [number, number, number] | undefined {
  const m = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(v.trim())
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : undefined
}

/**
 * Whether release `next` is newer than the running version `current`. A build between releases
 * ("0.6.3-build.41", from main after 0.6.3) counts as the release it's built on; a test build of
 * a release ("0.19.0-rc.1") comes before it, so 0.19.0 is an update for it.
 */
export function newerRelease(next: string, current: string): boolean {
  const a = parseRelease(next)
  const b = parseRelease(current.trim().replace(/[-+].*$/, ''))
  if (!a || !b) return false
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i]
  return TEST_BUILD.test(current.trim())
}

/** A signed test build of a release, published as a GitHub pre-release: "0.19.0-rc.1". */
export const TEST_BUILD = /^v?\d+\.\d+\.\d+-rc\.\d+$/

/** One file of the newest release, as latest.json lists it. */
export interface ReleaseFile {
  name: string
  size?: number
  sha256?: string
}

/**
 * The newest release, from latest.json on the website: its version ("0.14.0") and its files,
 * each downloaded from `${DOWNLOAD_URL}/<name>`. Throws when the site can't be reached or doesn't
 * name a plain release number, so a prerelease or a broken file never counts as an update.
 * latest.json isn't signed: it only says what is newest. What gets installed is checked on its
 * own, by `inyon update` against the release's signed SHA256SUMS and by Android against the
 * app's signing key.
 */
export async function fetchLatestRelease(get: typeof fetch = fetch): Promise<{ version: string; files: ReleaseFile[] }> {
  const res = await get(LATEST_URL, { headers: { accept: 'application/json' } })
  const host = new URL(LATEST_URL).host
  if (!res.ok) throw new Error(`${host} answered ${res.status}`)
  const body = (await res.json().catch(() => undefined)) as { version?: unknown; files?: unknown } | undefined
  const version = typeof body?.version === 'string' ? body.version.trim().replace(/^v/, '') : ''
  if (!parseRelease(version)) throw new Error(`${host} didn't name a release`)
  const files: ReleaseFile[] = []
  for (const f of Array.isArray(body?.files) ? body.files : []) {
    if (typeof f?.name !== 'string' || !f.name) continue
    files.push({ name: f.name, ...(typeof f.size === 'number' ? { size: f.size } : {}), ...(typeof f.sha256 === 'string' ? { sha256: f.sha256 } : {}) })
  }
  return { version, files }
}

/** The newest release among `versions` that is newer than `current`, if any. */
export function newestRelease(versions: string[], current: string): string | undefined {
  let best: string | undefined
  for (const v of versions) if (newerRelease(v, best ?? current)) best = v
  return best?.replace(/^v/, '')
}
