import { AGENT_CATALOG, type PhoneClaimResult } from '@inyon/protocol'
import { isNative, plugin } from './native.ts'
import { secretStore } from './secrets.ts'

/**
 * Hubs the phone app has paired with. The phone keeps reaching its own computers itself; a hub
 * brings its machines along, and they show in the same lists. A hub's machines carry its id in
 * front of theirs (`hub:<hubId>/<id>`), so every request and message finds its way back.
 */
export interface PairedHub {
  id: string
  name: string
  /** the hub's origin, https://hub.example or http://box:4777 */
  url: string
  version?: string
  pairedAt: number
}

/**
 * A hub as this device keeps it. The token lives in the secret store (secrets.ts) under
 * `hub-token:<id>`; only a browser page without one (plain http has no WebCrypto) keeps it here.
 */
type StoredHub = PairedHub & { token?: string }

const KEY = 'inyon.hubs.v1'
const PREFIX = 'hub:'

function stored(): StoredHub[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '[]')
    return Array.isArray(v) ? v.filter((h) => h && typeof h.id === 'string' && typeof h.url === 'string') : []
  } catch {
    return []
  }
}

export function listHubs(): PairedHub[] {
  return stored().map(({ token: _, ...h }) => h)
}

const listeners = new Set<(hubs: PairedHub[]) => void>()
function save(hubs: StoredHub[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(hubs))
  } catch {}
  const list = listHubs()
  for (const fn of listeners) fn(list)
}
/** Called whenever the list changes (the sockets follow it). */
export function onHubs(fn: (hubs: PairedHub[]) => void) {
  listeners.add(fn)
  return () => void listeners.delete(fn)
}

export const hubById = (id: string) => listHubs().find((h) => h.id === id)

export function forgetHub(id: string) {
  save(stored().filter((h) => h.id !== id))
  tokens.delete(id)
  void secretStore()?.delete(tokenKey(id)).catch(() => {})
}

// ---------- the token each hub gave this device ----------

const tokenKey = (id: string) => `hub-token:${id}`
const tokens = new Map<string, string>()

/** Keeps a hub's token in the secret store, or in the hub's record where there is none. */
async function keepToken(hub: PairedHub, token: string) {
  const store = secretStore()
  const rest = stored().filter((h) => h.id !== hub.id)
  if (store) {
    await store.set(tokenKey(hub.id), token)
    save([...rest, hub])
  } else {
    save([...rest, { ...hub, token }])
  }
  tokens.set(hub.id, token)
}

/**
 * The token a hub gave this device. One a version before 0.20.2 kept in the hub's record moves to
 * the secret store the first time it's asked for.
 */
export async function hubToken(id: string): Promise<string | undefined> {
  const known = tokens.get(id)
  if (known) return known
  const store = secretStore()
  const inline = stored().find((h) => h.id === id)?.token
  if (inline) {
    if (store) {
      await store.set(tokenKey(id), inline)
      save(
        stored().map((h) => {
          if (h.id !== id) return h
          const { token: _, ...rest } = h
          return rest
        }),
      )
    }
    tokens.set(id, inline)
    return inline
  }
  const kept = store ? await store.get(tokenKey(id)) : null
  if (kept) tokens.set(id, kept)
  return kept ?? undefined
}

/** A hub's machine as this app names it, and back. */
export const hubHostId = (hubId: string, id: string) => `${PREFIX}${hubId}/${id}`
export function splitHostId(id: string): { hub?: PairedHub; id: string } {
  if (!id.startsWith(PREFIX)) return { id }
  const slash = id.indexOf('/', PREFIX.length)
  if (slash < 0) return { id }
  const hub = hubById(id.slice(PREFIX.length, slash))
  return hub ? { hub, id: id.slice(slash + 1) } : { id }
}
export const isHubHost = (id: string) => id.startsWith(PREFIX)

/**
 * Two machines with the same name (two hubs on one box, or two servers both called "ubuntu") look
 * the same in every list. A hub's machine named like one you've had longer gets its hub's address after it.
 */
export function tellApart<H extends { config: { id: string; name: string } }>(hosts: H[]): H[] {
  // the machine had longest keeps its name: this hub's own, else the first paired hub's (on a
  // phone every machine comes through a hub, and they answer in any order)
  const since = (h: H) => splitHostId(h.config.id).hub?.pairedAt ?? 0
  const keeps = new Map<string, H>()
  for (const h of hosts) {
    const k = keeps.get(h.config.name)
    if (!k || since(h) < since(k)) keeps.set(h.config.name, h)
  }
  return hosts.map((h) => {
    if (keeps.get(h.config.name) === h) return h
    const hub = splitHostId(h.config.id).hub
    if (!hub) return h
    const where = hub.url.replace(/^https?:\/\//, '')
    return { ...h, config: { ...h.config, name: `${h.config.name} · ${where}` } }
  })
}

/**
 * The link a hub shows in its QR code: inyon://hub?u=<origin>&c=<id:code>&n=<name>, or the download
 * page with it after the #. From 0.20.2 it also carries the hub's id (`h`) and, when the hub serves
 * HTTPS itself, its certificate's SHA-256 (`f`), which the app shows next to the hub's own copy.
 */
export interface HubLink {
  url: string
  code: string
  /** what the hub calls itself; anyone can write any name into a link */
  name?: string
  hubId?: string
  tlsFingerprint?: string
}

export function parseHubLink(text: string): HubLink | undefined {
  const t = text.trim()
  const m = t.match(/^(?:inyon:\/\/hub\?|https?:\/\/[^#\s]+#hub\?)(.+)$/)
  if (!m) return
  const q = new URLSearchParams(m[1])
  const url = q.get('u') ?? ''
  const code = q.get('c') ?? ''
  if (!/^https?:\/\/[^/\s]+$/.test(url) || !/^[0-9A-Za-z]{8}:\d{6}$/.test(code)) return
  const hubId = q.get('h')?.toLowerCase()
  const fp = q.get('f')?.toLowerCase()
  return {
    url,
    code,
    name: q.get('n') || undefined,
    ...(hubId && /^[0-9a-f]{16}$/.test(hubId) ? { hubId } : {}),
    ...(fp && /^[0-9a-f]{64}$/.test(fp) ? { tlsFingerprint: fp } : {}),
  }
}
export const looksLikeHubLink = (text: string) => !!parseHubLink(text)

/** A hub id or fingerprint in groups of four, to read off one screen and compare with another: "3f9c 1a7e 2b6d 4c80". */
export const groupsOf = (hex: string, groups = 4) => (hex.match(/.{1,4}/g) ?? []).slice(0, groups).join(' ')

/** "K7Q4-MX2T 493812", "K7Q4MX2T:493812" and the like, as the code the hub expects. */
export function normalizeHubCode(typed: string): string | undefined {
  const m = typed
    .toUpperCase()
    .replace(/[\s-]/g, '')
    .match(/^([0-9A-Z]{8}):?(\d{6})$/)
  return m ? `${m[1]}:${m[2]}` : undefined
}

/** A hub's address as typed: a host, host:port or full origin becomes an origin. */
export function hubOriginOf(typed: string): string | undefined {
  let t = typed.trim()
  if (!t) return
  // a bare name or an IP is a hub on a private network, reached over http; a domain name is https
  if (!/^https?:\/\//.test(t)) t = `${/^(localhost|\d{1,3}(\.\d{1,3}){3}|\[[0-9a-f:]+\]|[^.:/]+)(:\d+)?$/i.test(t) ? 'http' : 'https'}://${t}`
  try {
    const u = new URL(t)
    if (u.pathname !== '/' && u.pathname !== '') return
    return u.origin
  } catch {
    return
  }
}

// ---------- plain http: only to this phone or a VPN that encrypts on its own ----------

/** The parts of an IPv4 address, or undefined. */
function v4(s: string): number[] | undefined {
  const m = s.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/)
  const parts = m?.slice(1).map(Number)
  return parts && parts.every((n) => n <= 255) ? parts : undefined
}

/** The eight 16-bit groups of an IPv6 address, or undefined. */
function v6(s: string): number[] | undefined {
  const halves = s.split('::')
  if (halves.length > 2) return
  const groups = (part: string) => {
    if (!part) return []
    const out: number[] = []
    const items = part.split(':')
    for (const [i, x] of items.entries()) {
      const four = i === items.length - 1 ? v4(x) : undefined
      if (four) out.push((four[0]! << 8) | four[1]!, (four[2]! << 8) | four[3]!)
      else if (/^[0-9a-f]{1,4}$/i.test(x)) out.push(parseInt(x, 16))
      else return
    }
    return out
  }
  const head = groups(halves[0]!)
  const tail = halves.length === 2 ? groups(halves[1]!) : []
  if (!head || !tail) return
  const missing = 8 - head.length - tail.length
  if (halves.length === 1 ? missing !== 0 : missing < 1) return
  return [...head, ...Array(halves.length === 2 ? missing : 0).fill(0), ...tail]
}

/**
 * An address plain http may go to: this device (127.0.0.0/8, ::1), or a VPN that encrypts on its
 * own (Tailscale and NetBird's 100.64.0.0/10, Tailscale's fd7a:115c:a1e0::/48). A LAN address isn't:
 * anyone on the Wi-Fi could read the token there, or answer in the hub's place. The same rule as the
 * inyon command's (crates/inyon-engine/src/hub.rs).
 */
export function plainHttpIp(address: string): boolean {
  const s = address.replace(/^\[|\]$/g, '').replace(/%.*$/, '')
  const four = v4(s)
  if (four) return four[0] === 127 || (four[0] === 100 && four[1]! >= 64 && four[1]! <= 127)
  const g = v6(s)
  if (!g) return false
  const zeros = (n: number) => g.slice(0, n).every((x) => x === 0)
  if (zeros(5) && g[5] === 0xffff) return plainHttpIp(`${g[6]! >> 8}.${g[6]! & 255}.${g[7]! >> 8}.${g[7]! & 255}`)
  if (zeros(7) && g[7] === 1) return true
  return g[0] === 0xfd7a && g[1] === 0x115c && g[2] === 0xa1e0
}

/**
 * The first of a name's addresses that plain http mustn't go to, since the WebView looks the name
 * up again and may use any of them. Every address must be allowed, with one exception: NetBird
 * gives each peer an IPv6 address too, in a unique-local /64 picked at random per network
 * (fd91:8fa2:…), so no fixed range names it. A unique-local IPv6 (fc00::/7) passes when the name
 * also has an allowed IPv4 (100.64/10 or loopback): that is the VPN's own overlay next to its
 * 100.x address. A LAN or public address anywhere still refuses, and so does a unique-local
 * address alone. Same rule as `judge` in crates/inyon-engine/src/hub.rs.
 */
export function notPlain(addresses: string[]): string | undefined {
  const bare = (a: string) => a.replace(/^\[|\]$/g, '').replace(/%.*$/, '')
  const vpnV4 = addresses.some((a) => !!v4(bare(a)) && plainHttpIp(a))
  const overlay = (a: string) => {
    const g = v6(bare(a))
    return !!g && (g[0]! & 0xfe00) === 0xfc00
  }
  return addresses.find((a) => !plainHttpIp(a) && !(vpnV4 && overlay(a)))
}

/** What a hub's host is to plain http: fine as it is, a name to look up first, or refused. */
export function plainHost(hostname: string): 'ok' | 'look-up' | 'refused' {
  const h = hostname.toLowerCase()
  if (h === 'localhost') return 'ok'
  if (h.startsWith('[') || v4(h)) return plainHttpIp(h) ? 'ok' : 'refused'
  return 'look-up'
}

/** A name Tailscale or NetBird gives out, for a browser, which can't look a name up. */
const vpnName = (host: string) => !host.includes('.') || /\.(ts\.net|netbird\.cloud|netbird\.selfhosted)$/i.test(host)

const PLAIN_WHY = "anyone on that network could read what's sent, or answer in the hub's place. Use the hub's https address, or its Tailscale or NetBird address (100.x.y.z)."

/** Why the app won't use plain http with a hub, and what to do instead. */
export const plainRefusal = (host: string, points?: string) => `Inyon won't use plain http with ${host}${points ? ` (it points to ${points})` : ''}: ${PLAIN_WHY}`

/** The app refused a plain-http hub; the message says why. */
export class PlainRefused extends Error {}

// names that pointed only at allowed addresses, until when; looked up again after a minute
const looked = new Map<string, number>()

/**
 * Throws unless `url` is https, or plain http to an address above. A name is looked up first (in
 * the Android app) and every address it has must be one of them, so a NetBird or MagicDNS name
 * like http://abhash-arm works and a LAN or public one doesn't. A browser can't look names up, so
 * there only the names those VPNs give out pass.
 */
export async function checkPlain(url: string, now = Date.now()): Promise<void> {
  let u: URL
  try {
    u = new URL(url)
  } catch {
    throw new PlainRefused(`${url} isn't a web address.`)
  }
  if (u.protocol === 'https:') return
  if (u.protocol !== 'http:') throw new PlainRefused(`${url} isn't a web address.`)
  const host = u.hostname
  const kind = plainHost(host)
  if (kind === 'ok') return
  if (kind === 'refused') throw new PlainRefused(plainRefusal(host))
  if ((looked.get(host) ?? 0) > now) return
  const lookUp = plugin('InyonDevice')?.lookUp
  if (typeof lookUp !== 'function') {
    if (vpnName(host)) return
    throw new PlainRefused(`A browser can't check where ${host} points, so Inyon won't use plain http with it: ${PLAIN_WHY}`)
  }
  let addresses: string[] = []
  try {
    addresses = ((await lookUp({ host })) as { addresses?: string[] } | undefined)?.addresses ?? []
  } catch {}
  // a name that doesn't resolve right now (no network, the VPN off) is worth trying again
  if (!addresses.length) throw new Error(`Couldn't look up ${host}. Is this phone on the hub's network or VPN?`)
  const bad = notPlain(addresses)
  if (bad) throw new PlainRefused(plainRefusal(host, bad))
  looked.set(host, now + 60_000)
}

/** What to say when nothing came back from `url`: what to check, depending on where the app runs. */
export function unreachable(url: string, err: Error): string {
  if (err.name === 'TimeoutError') return `${url} didn't answer in time. Is this phone on the hub's network?`
  const plain = url.startsWith('http://')
  const page = typeof location !== 'undefined' && location.protocol === 'https:'
  // a page served over https may not call a plain-http address; the app may
  if (plain && page && !isNative()) return `${url} is a plain address, which a browser page can't reach. Pair from the Inyon app, or open ${url} in the browser and sign in there.`
  return `Couldn't reach ${url}. Check that it opens in this phone's browser and that the address is spelt as the hub shows it.`
}

/**
 * Trades a code for a token at the hub and remembers the hub. With `expect.hubId` (from the link)
 * a hub answering with another id is refused, and nothing is kept.
 */
export async function claimHub(url: string, code: string, name: string, expect: { hubId?: string } = {}): Promise<PairedHub> {
  await checkPlain(url)
  const res = await fetch(`${url}/api/phones/claim`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code, name }),
    credentials: 'omit',
    signal: AbortSignal.timeout(15_000),
  }).catch((err: Error) => {
    // the request never got an answer: the browser refused it or the address doesn't reach
    throw new Error(unreachable(url, err))
  })
  const data = (await res.json().catch(() => ({}))) as Partial<PhoneClaimResult> & { error?: string }
  if (!res.ok || !data.token || !data.hub) throw new Error(data.error ?? `The hub answered ${res.status}`)
  if (expect.hubId && data.hub.id !== expect.hubId) throw new Error(`The hub at ${url.replace(/^https?:\/\//, '')} isn't the one in the code: its ID is ${groupsOf(data.hub.id)}, the code said ${groupsOf(expect.hubId)}. Nothing was saved.`)
  const hub: PairedHub = {
    id: data.hub.id,
    name: data.hub.name,
    url,
    version: data.hub.version,
    pairedAt: Date.now(),
  }
  await keepToken(hub, data.token)
  return hub
}

/**
 * Whether a 401 or 403 is the hub saying it no longer knows this phone: `reason: "revoked"` from
 * 0.20.2, or the exact answer hubs before it gave. A captive portal, a proxy's sign-in or anything
 * else in the way says something else, and the hub is kept.
 */
export function revokedAnswer(status: number, body: unknown): boolean {
  if (status !== 401 && status !== 403) return false
  const b = body && typeof body === 'object' ? (body as Record<string, unknown>) : undefined
  if (b?.reason === 'revoked') return true
  return status === 401 && b?.error === 'Not signed in' && Object.keys(b).length === 1
}

/** A short-lived ticket for the hub's WebSocket, which can't carry the token itself. */
export async function hubTicket(hub: PairedHub): Promise<string> {
  await checkPlain(hub.url)
  const token = await hubToken(hub.id)
  if (!token) throw new HubRevoked(hub)
  const res = await fetch(`${hub.url}/api/phones/ticket`, {
    method: 'POST',
    headers: { authorization: `Bearer ${token}` },
    credentials: 'omit',
    signal: AbortSignal.timeout(10_000),
  })
  if (res.status === 401 || res.status === 403) {
    if (revokedAnswer(res.status, await res.json().catch(() => undefined))) throw new HubRevoked(hub)
    throw new Error(`The hub answered ${res.status}`)
  }
  if (!res.ok) throw new Error(`The hub answered ${res.status}`)
  const { ticket, hub: about } = (await res.json()) as { ticket: string; hub?: PhoneClaimResult['hub'] }
  // the hub says what it is now: keep its name and version current (they were only read at pairing)
  if (about?.id === hub.id && (about.version !== hub.version || about.name !== hub.name)) {
    save(stored().map((h) => (h.id === hub.id ? { ...h, name: about.name || h.name, version: about.version } : h)))
  }
  return ticket
}

/** The hub no longer knows this phone (it was forgotten on the hub). */
export class HubRevoked extends Error {
  constructor(public hub: PairedHub) {
    super(`${hub.name} no longer knows this phone`)
  }
}

export const hubWsUrl = (hub: PairedHub, ticket: string) => `${hub.url.replace(/^http/, 'ws')}/ws?ticket=${encodeURIComponent(ticket)}&catalog=${AGENT_CATALOG}`
