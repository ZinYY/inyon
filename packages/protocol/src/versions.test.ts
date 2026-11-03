import { expect, test } from 'bun:test'
import { DOWNLOAD_URL, fetchLatestRelease, INSTALL_URL, LATEST_URL } from './versions.ts'

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
  expect(DOWNLOAD_URL).toBe(`${INSTALL_URL}/download`)
  expect(LATEST_URL).toBe('https://inyon.tech/download/latest.json')
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
    await expect(fetchLatestRelease(answering({ version, files: [] }).get)).rejects.toThrow("inyon.tech didn't name a release")
  }
  await expect(fetchLatestRelease(answering('<html>not json</html>').get)).rejects.toThrow("didn't name a release")
  await expect(fetchLatestRelease(answering('null').get)).rejects.toThrow("didn't name a release")
})

test('a site that answers with an error, or not at all, throws', async () => {
  await expect(fetchLatestRelease(answering('not found', 404).get)).rejects.toThrow('inyon.tech answered 404')
  const offline = (async () => {
    throw new TypeError('fetch failed')
  }) as unknown as typeof fetch
  await expect(fetchLatestRelease(offline)).rejects.toThrow('fetch failed')
})
