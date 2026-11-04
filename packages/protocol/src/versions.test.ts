import { expect, test } from 'bun:test'
import { DOWNLOAD_URL, fetchLatestRelease, INSTALL_URL, LATEST_URL } from './versions.ts'
import { VERSION } from './index.ts'
import { version as packageVersion } from '../../../package.json'

test('web version follows the root package without a separate hardcoded number', () => {
  expect(VERSION).toBe(packageVersion)
})

test('the fork GitHub release response supplies its version and assets', async () => {
  const response = answering({ tag_name: 'v0.24.49', assets: [{ name: 'inyon-linux-x64', size: 123 }] })
  expect(await fetchLatestRelease(response.get)).toEqual({ version: '0.24.49', files: [{ name: 'inyon-linux-x64', size: 123 }] })
  expect(response.urls).toEqual(['https://api.github.com/repos/ZinYY/inyon/releases/latest'])
  for (const state of [{ prerelease: true }, { draft: true }]) {
    await expect(fetchLatestRelease(answering({ tag_name: 'v0.25.0', ...state }).get)).rejects.toThrow("didn't name a release")
  }
})

/** A fetch that answers every request with `body` (JSON unless it's text) and notes the address. */
function answering(body: unknown, status = 200) {
  const urls: string[] = []
  const get = (async (url: string) => {
    urls.push(String(url))
    return new Response(typeof body === 'string' ? body : JSON.stringify(body), { status })
  }) as unknown as typeof fetch
  return { get, urls }
}

test('the newest release is read from latest.json on the website', async () => {
  expect(DOWNLOAD_URL).toBe('https://github.com/ZinYY/inyon/releases/latest/download')
  expect(LATEST_URL).toBe('https://api.github.com/repos/ZinYY/inyon/releases/latest')
  const listing = {
    version: '0.14.0',
    tag: 'v0.14.0',
    date: '2026-10-13',
    files: [
      { name: 'inyon-0.14.0.apk', size: 2861545, sha256: 'ab'.repeat(32) },
      { name: 'SHA256SUMS', size: 12 },
      // anything that isn't a file entry is left out
      { size: 3 },
      'inyon-linux-x64',
      { name: '' },
    ],
  }
  const { get, urls } = answering(listing)
  expect(await fetchLatestRelease(get)).toEqual({
    version: '0.14.0',
    files: [
      { name: 'inyon-0.14.0.apk', size: 2861545, sha256: 'ab'.repeat(32) },
      { name: 'SHA256SUMS', size: 12 },
    ],
  })
  expect(urls).toEqual([LATEST_URL])
  // a "v" in front is fine, and files may be missing altogether
  expect(await fetchLatestRelease(answering({ version: 'v0.14.1' }).get)).toEqual({ version: '0.14.1', files: [] })
})

test('anything but a plain release number throws, so it never counts as an update', async () => {
  for (const version of ['0.15.0-rc.1', '0.15', 'latest', '', 15, undefined]) {
    await expect(fetchLatestRelease(answering({ version, files: [] }).get)).rejects.toThrow("api.github.com didn't name a release")
  }
  await expect(fetchLatestRelease(answering('<html>not json</html>').get)).rejects.toThrow("didn't name a release")
  await expect(fetchLatestRelease(answering('null').get)).rejects.toThrow("didn't name a release")
})

test('a site that answers with an error, or not at all, throws', async () => {
  await expect(fetchLatestRelease(answering('not found', 404).get)).rejects.toThrow('api.github.com answered 404')
  const offline = (async () => {
    throw new TypeError('fetch failed')
  }) as unknown as typeof fetch
  await expect(fetchLatestRelease(offline)).rejects.toThrow('fetch failed')
})
