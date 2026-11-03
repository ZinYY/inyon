/**
 * The one-line installers (get.sh, get.ps1) as the website serves them at its root. Each carries
 * an empty line for the sha256 of the release's checksum list; the site fills it in once it has
 * checked that list's signature, so the copy people pipe into a shell can trust the list without
 * minisign. scripts/container-tests.sh does the same for its mock release.
 */
export const INSTALLERS = ['get.sh', 'get.ps1'] as const
export type Installer = (typeof INSTALLERS)[number]

export const isInstaller = (name: string): name is Installer => (INSTALLERS as readonly string[]).includes(name)

/** The empty line each installer carries, and the name it gives the list's sha256. */
const SLOT: Record<Installer, { empty: RegExp; name: string; line: (sha: string) => string }> = {
  'get.sh': { empty: /^LIST_SHA256=$/gm, name: 'LIST_SHA256', line: (sha) => `LIST_SHA256=${sha}` },
  'get.ps1': { empty: /^\$listSha256 = ''$/gm, name: '$listSha256', line: (sha) => `$listSha256 = '${sha}'` },
}

/**
 * `script` with the sha256 of the checksum list it may trust filled in. An installer from before
 * 0.20 has no place for it and comes back as it is (it trusts the list as it always did); one that
 * names the slot but hasn't exactly one empty line for it is refused rather than half filled.
 */
export function stampInstaller(name: Installer, script: string, listSha256: string): string {
  if (!/^[0-9a-f]{64}$/.test(listSha256)) throw new Error(`${name}: ${listSha256} isn't a sha256`)
  const slot = SLOT[name]
  const found = script.match(slot.empty)?.length ?? 0
  if (found === 1) return script.replace(slot.empty, slot.line(listSha256))
  if (found === 0 && !script.includes(slot.name)) return script
  throw new Error(`${name}: expected one empty ${slot.name} line, found ${found}`)
}
