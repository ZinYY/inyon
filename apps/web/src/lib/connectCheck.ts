import type { HostState } from '@inyon/protocol'
import { causeOf, type Power } from './connlog.ts'
import { revokedAnswer, type PairedHub } from './hubs.ts'

/**
 * "Can't connect?": the checks the app runs itself, in order, for a hub or a computer that isn't
 * answering. Each says in one sentence what it found and what to do. Everything it touches (the
 * network, sockets, the phone's own lookups) comes in through `CheckDeps`, so tests can fail each
 * step on purpose.
 */

export type StepState = 'pending' | 'running' | 'pass' | 'warn' | 'fail' | 'skip'

/** A button the sheet offers next to a step's advice. */
export type StepAction = 'sign-in' | 'pair-hub' | 'pair-computer' | 'battery' | 'battery-settings' | 'stay-connected'

export interface Step {
  id: string
  title: string
  /** while it runs: "Checking the hub answers…" */
  doing: string
  state: StepState
  /** what it found, in a sentence */
  say?: string
  /** what to do about it */
  fix?: string
  /** the raw answer, shown only under Details */
  detail?: string
  action?: StepAction
}

/** What's being checked. */
export type Target =
  /** the hub this page is served from */
  | { kind: 'self'; id: 'self'; name: string; origin: string }
  /** a hub the phone app paired with */
  | { kind: 'hub'; id: string; name: string; hub: PairedHub }
  /** a computer the phone paired with directly, over the encrypted link */
  | { kind: 'computer'; id: string; name: string; host: HostState }
  /** a machine a hub carries: the hub first, then whether the hub reaches it */
  | { kind: 'machine'; id: string; name: string; host: HostState; via: Extract<Target, { kind: 'self' | 'hub' }> }

export interface Sock {
  onopen: ((e: unknown) => void) | null
  onclose: ((e: { code: number; reason: string }) => void) | null
  onerror: ((e: unknown) => void) | null
  close(): void
}

export interface CheckDeps {
  fetch: (url: string, init?: RequestInit) => Promise<Response>
  socket: (url: string) => Sock
  online: () => boolean
  /** the phone's own name lookup (InyonDevice.lookUp); a browser has none */
  lookUp?: (host: string) => Promise<string[]>
  /** throws when the app won't use plain http with this address (hubs.ts) */
  checkPlain?: (url: string) => Promise<void>
  token?: (hubId: string) => Promise<string | undefined>
  /** this page's own hub WebSocket */
  wsUrl?: () => string
  hubWsUrl?: (hub: PairedHub, ticket: string) => string
  /** asks the phone's core to try a computer again now */
  reconnect?: (id: string) => void
  host: (id: string) => HostState | undefined
  /** Android only */
  power?: () => Promise<Power | undefined>
  keepAliveWanted?: () => boolean
  sleep?: (ms: number) => Promise<void>
  /** how long a request or socket gets (8 s), and the handshake (20 s) */
  timeoutMs?: number
  handshakeMs?: number
}

/** Where a computer's address is published (the same relays as crates/inyon-link). */
export const PKARR_RELAYS = ['https://relay.pkarr.org', 'https://dns.iroh.link/pkarr']

type Result = Pick<Step, 'state' | 'say' | 'fix' | 'detail' | 'action'>
interface Ctx {
  session?: Response
  sessionBody?: { authed?: boolean }
  token?: string
  ticket?: string
}
interface Plan {
  id: string
  title: string
  doing: string
  /** a failure here means the steps after it can't tell anything */
  blocks?: boolean
  run: (ctx: Ctx) => Promise<Result>
}

const hostOf = (url: string) => {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}
const nameOf = (url: string) => {
  try {
    return new URL(url).hostname
  } catch {
    return url
  }
}
const isIp = (h: string) => /^\d{1,3}(\.\d{1,3}){3}$/.test(h) || h.startsWith('[') || h === 'localhost'
/** A name only Tailscale or NetBird answers for. */
const vpnName = (h: string) => !h.includes('.') || /\.(ts\.net|netbird\.cloud|netbird\.selfhosted)$/i.test(h)
const errText = (e: unknown) => (e instanceof Error ? `${e.name}: ${e.message}` : String(e))
const timedOut = (e: unknown) => e instanceof Error && (e.name === 'TimeoutError' || e.name === 'AbortError')

export const SKIPPED = 'Skipped until the step above passes.'

/** The steps for a target, in the order they run. */
export function plan(t: Target, d: CheckDeps): Plan[] {
  const ms = d.timeoutMs ?? 8000
  const n = Math.round(ms / 1000)
  const secs = `${n} second${n === 1 ? '' : 's'}`
  const steps: Plan[] = [
    {
      id: 'network',
      title: 'Network',
      doing: 'Checking this device is online…',
      blocks: true,
      run: async () =>
        d.online()
          ? { state: 'pass', say: 'This device is online.' }
          : { state: 'fail', say: 'This device is offline.', fix: 'Turn on Wi-Fi or mobile data, then try again.' },
    },
  ]
  const hub = t.kind === 'machine' ? t.via : t.kind === 'computer' ? undefined : t
  if (hub) steps.push(...hubSteps(hub, d, ms, secs))
  if (t.kind === 'machine') steps.push(machineStep(t, d))
  if (t.kind === 'computer') steps.push(...computerSteps(t, d, ms))
  if (d.power) steps.push(batteryStep(d))
  return steps
}

function hubSteps(t: Extract<Target, { kind: 'self' | 'hub' }>, d: CheckDeps, ms: number, secs: string): Plan[] {
  const url = t.kind === 'self' ? t.origin : t.hub.url
  const host = hostOf(url)
  const name = nameOf(url)
  const signal = () => AbortSignal.timeout(ms)
  const unreached = (e: unknown): Result =>
    timedOut(e)
      ? { state: 'fail', say: `The hub didn't answer within ${secs}.`, fix: `Check this device is on the hub's network or VPN, and that the hub is running (inyon service status on its machine).`, detail: errText(e) }
      : {
          state: 'fail',
          say: `Couldn't connect to ${host}.`,
          fix: `Check the hub is running and the address is right.${url.startsWith('https:') ? " On https, this device must also trust the hub's certificate." : ''}${t.kind === 'self' ? ' What you see now may be a saved copy of the app.' : ''}`,
          detail: errText(e),
        }
  return [
    {
      id: 'address',
      title: 'Address',
      doing: `Looking up ${name}…`,
      blocks: true,
      run: async () => {
        const plain = async (): Promise<Result | undefined> => {
          try {
            await d.checkPlain?.(url)
          } catch (e) {
            return { state: 'fail', say: (e as Error).message, fix: "Use the hub's https address, or its Tailscale or NetBird address." }
          }
        }
        if (isIp(name)) return (await plain()) ?? { state: 'pass', say: `${name} is an IP address, so there's nothing to look up.` }
        if (!d.lookUp) return { state: 'skip', say: "A browser can't look names up. The next step covers it." }
        let found: string[] = []
        try {
          found = await d.lookUp(name)
        } catch {}
        if (!found.length)
          return {
            state: 'fail',
            say: `${name} doesn't point to any address from here.`,
            fix: vpnName(name) ? 'That name only works on Tailscale or NetBird: turn it on on this phone.' : 'Check the address is spelt as the hub shows it, and that this network can look it up.',
          }
        return (await plain()) ?? { state: 'pass', say: `${name} points to ${found.slice(0, 3).join(', ')}.` }
      },
    },
    {
      id: 'answers',
      title: 'Hub answers',
      doing: 'Checking the hub answers…',
      blocks: true,
      run: async (ctx) => {
        try {
          if (t.kind === 'self') {
            ctx.session = await d.fetch('/api/session', { credentials: 'same-origin', cache: 'no-store', signal: signal() })
            ctx.sessionBody = await ctx.session.clone().json().catch(() => undefined)
            return { state: 'pass', say: `The hub answered.`, detail: `GET /api/session answered ${ctx.session.status}` }
          }
          // another site's answer can't be read without its say-so, but that one came back at all
          await d.fetch(`${url}/api/session`, { mode: 'no-cors', credentials: 'omit', cache: 'no-store', signal: signal() })
          return { state: 'pass', say: `Something answers at ${host}.` }
        } catch (e) {
          return unreached(e)
        }
      },
    },
    {
      id: 'host',
      title: 'Right address',
      doing: 'Checking the hub answers to this address…',
      blocks: true,
      run: async (ctx) => {
        const wrong: Result = { state: 'fail', say: `The hub doesn't answer to ${name}.`, fix: `Add ${name} to INYON_ALLOWED_HOSTS on the hub, or use the address the hub shows.` }
        if (t.kind === 'self') return ctx.session?.status === 421 ? { ...wrong, detail: 'HTTP 421' } : { state: 'pass', say: `The hub answers to ${name}.` }
        // without a token the request needs no preflight, and the hub lets any origin read its 421
        const plain = await d.fetch(`${url}/api/session`, { credentials: 'omit', cache: 'no-store', signal: signal() }).catch(() => undefined)
        if (plain?.status === 421) return { ...wrong, detail: 'HTTP 421' }
        ctx.token = await d.token?.(t.hub.id)
        if (!ctx.token) return { state: 'skip', say: 'This phone has no key for the hub any more; the next step says what to do.' }
        try {
          const res = await d.fetch(`${url}/api/session`, { headers: { authorization: `Bearer ${ctx.token}` }, credentials: 'omit', cache: 'no-store', signal: signal() })
          if (res.status === 421) return { ...wrong, detail: 'HTTP 421' }
          return { state: 'pass', say: `The hub answers to ${name}.`, detail: `GET /api/session answered ${res.status}` }
        } catch (e) {
          if (timedOut(e)) return unreached(e)
          return {
            state: 'fail',
            say: `Something answered at ${host} but turned the app away.`,
            fix: `Most often the hub doesn't answer to this name: add ${name} to INYON_ALLOWED_HOSTS on the hub. A proxy or a Wi-Fi sign-in page in the way does the same.`,
            detail: errText(e),
          }
        }
      },
    },
    {
      id: 'signed-in',
      title: 'Signed in',
      doing: t.kind === 'self' ? 'Checking you are signed in…' : 'Checking the hub still knows this phone…',
      blocks: true,
      run: async (ctx) => {
        if (t.kind === 'self') {
          if (ctx.sessionBody?.authed) return { state: 'pass', say: "You're signed in." }
          if (ctx.sessionBody?.authed === false) return { state: 'fail', say: 'You are signed out of this hub.', fix: 'Sign in again.', action: 'sign-in' }
          return { state: 'fail', say: `The hub answered ${ctx.session?.status ?? 'nothing'} where it says who's signed in.`, fix: 'Something in front of the hub (a proxy) may be answering for it.', detail: `GET /api/session answered ${ctx.session?.status}` }
        }
        const forgotten: Result = { state: 'fail', say: 'The hub no longer knows this phone.', fix: 'Pair again: on the hub choose Open on your phone, then Connect to a hub here.', action: 'pair-hub' }
        if (!ctx.token) return forgotten
        let res: Response
        try {
          res = await d.fetch(`${url}/api/phones/ticket`, { method: 'POST', headers: { authorization: `Bearer ${ctx.token}` }, credentials: 'omit', signal: signal() })
        } catch (e) {
          return unreached(e)
        }
        if (res.status === 401 || res.status === 403) {
          if (revokedAnswer(res.status, await res.json().catch(() => undefined))) return { ...forgotten, detail: `POST /api/phones/ticket answered ${res.status}` }
          return { state: 'fail', say: `Something other than the hub answered ${res.status}.`, fix: 'A Wi-Fi sign-in page or a proxy may be in the way. Open any site in the browser to finish a sign-in, or turn the VPN on.', detail: `POST /api/phones/ticket answered ${res.status}` }
        }
        if (!res.ok) return { state: 'fail', say: `The hub answered ${res.status}.`, fix: "The hub's log says why (inyon logs on its machine).", detail: `POST /api/phones/ticket answered ${res.status}` }
        ctx.ticket = ((await res.json().catch(() => ({}))) as { ticket?: string }).ticket
        if (!ctx.ticket) return { state: 'fail', say: 'The hub answered without a ticket for the live connection.', fix: 'Something in front of the hub may be answering for it. Update the hub if it is older than this app.', detail: `POST /api/phones/ticket answered ${res.status} without a ticket` }
        return { state: 'pass', say: 'The hub knows this phone.' }
      },
    },
    {
      id: 'socket',
      title: 'Live connection',
      doing: 'Opening the live connection…',
      blocks: true,
      run: async (ctx) => {
        const target = t.kind === 'self' ? d.wsUrl?.() : ctx.ticket && d.hubWsUrl?.(t.hub, ctx.ticket)
        if (!target) return { state: 'skip', say: "There's no live connection to try from here." }
        const proxy = 'If a proxy sits in front of the hub, it has to pass WebSockets through (the Upgrade and Connection headers).'
        const got = await openSocket(d, target, ms)
        if (got === 'open') return { state: 'pass', say: 'The live connection opens.' }
        if (got === 'timeout') return { state: 'fail', say: `The live connection didn't open within ${secs}.`, fix: proxy }
        if (got.code === 4001) return { state: 'fail', say: 'The hub refused the live connection: this device is signed out.', fix: t.kind === 'self' ? 'Sign in again.' : 'Pair this phone with the hub again.', action: t.kind === 'self' ? 'sign-in' : 'pair-hub', detail: `close ${got.code}` }
        return { state: 'fail', say: `The live connection was refused (code ${got.code}).`, fix: proxy, detail: `close ${got.code}${got.reason ? ` ${got.reason}` : ''}` }
      },
    },
  ]
}

/** Opens a WebSocket and closes it again: 'open', 'timeout', or how it closed. */
function openSocket(d: CheckDeps, url: string, ms: number): Promise<'open' | 'timeout' | { code: number; reason: string }> {
  return new Promise((resolve) => {
    let ws: Sock
    try {
      ws = d.socket(url)
    } catch {
      return resolve({ code: 1006, reason: '' })
    }
    const timer = setTimeout(() => (finish(), resolve('timeout')), ms)
    const finish = () => {
      clearTimeout(timer)
      ws.onopen = ws.onclose = ws.onerror = null
      try {
        ws.close()
      } catch {}
    }
    ws.onopen = () => (finish(), resolve('open'))
    ws.onclose = (e) => (finish(), resolve({ code: e?.code ?? 1006, reason: e?.reason ?? '' }))
    ws.onerror = () => {}
  })
}

function machineStep(t: Extract<Target, { kind: 'machine' }>, d: CheckDeps): Plan {
  return {
    id: 'machine',
    title: 'Hub reaches it',
    doing: `Checking the hub reaches ${t.name}…`,
    run: async () => {
      const h = d.host(t.id) ?? t.host
      if (h.status === 'online') return { state: 'pass', say: `${t.name} is connected to the hub.` }
      const fix =
        h.config.kind === 'ssh'
          ? `Check the hub's machine can ssh to ${h.config.target ?? t.name} without a password prompt.`
          : h.config.kind === 'local'
            ? "Check tmux is installed on the hub's machine."
            : `On ${t.name}, run inyon service status, and inyon logs --role node to see why it can't connect.`
      return { state: 'fail', say: h.error ? `The hub can't reach ${t.name}: ${h.error}` : `The hub can't reach ${t.name}.`, fix, detail: h.error }
    },
  }
}

function computerSteps(t: Extract<Target, { kind: 'computer' }>, d: CheckDeps, ms: number): Plan[] {
  const id = t.host.config.machineId ?? ''
  const signal = () => AbortSignal.timeout(ms)
  return [
    {
      id: 'lookup',
      title: 'Its address',
      doing: 'Looking up where the computer is…',
      run: async () => {
        if (!id) return { state: 'skip', say: "This computer has no ID to look up; pair it again." }
        const answers = await Promise.all(
          PKARR_RELAYS.map((r) =>
            d.fetch(`${r}/${id}`, { credentials: 'omit', cache: 'no-store', signal: signal() }).then(
              (res) => res.status,
              (e) => errText(e),
            ),
          ),
        )
        const detail = PKARR_RELAYS.map((r, i) => `${hostOf(r)}: ${answers[i]}`).join('\n')
        if (answers.some((a) => a === 200)) return { state: 'pass', say: 'The computer has published where it is.', detail }
        if (answers.some((a) => typeof a === 'number'))
          return { state: 'warn', say: "The computer hasn't published where it is.", fix: 'Is inyon link running there? Run inyon service status on the computer; inyon pair starts it. Its saved addresses are still tried.', detail }
        return { state: 'warn', say: "Couldn't reach the lookup service (pkarr).", fix: "This network may block it. The computer's saved addresses are still tried.", detail }
      },
    },
    {
      id: 'relay',
      title: 'Relay',
      doing: 'Checking its relay answers…',
      run: async () => {
        const relays = t.host.config.relays ?? []
        if (!relays.length) return { state: 'skip', say: "This computer doesn't use a relay." }
        const ok: string[] = []
        const bad: string[] = []
        for (const r of relays) {
          const origin = r.replace(/^ws/, 'http')
          await d.fetch(origin, { mode: 'no-cors', credentials: 'omit', cache: 'no-store', signal: signal() }).then(
            () => ok.push(hostOf(origin)),
            (e) => bad.push(`${hostOf(origin)}: ${errText(e)}`),
          )
        }
        if (ok.length) return { state: 'pass', say: `The relay at ${ok[0]} answers.` }
        return { state: 'warn', say: `The relay at ${hostOf(relays[0]!)} doesn't answer.`, fix: "The computer's tunnel and saved addresses are still tried.", detail: bad.join('\n') }
      },
    },
    {
      id: 'handshake',
      title: 'Handshake',
      doing: 'Reaching the computer…',
      run: async () => {
        if (d.host(t.id)?.status === 'online') return { state: 'pass', say: 'The computer answered and the encrypted handshake worked.' }
        d.reconnect?.(t.id)
        const until = Date.now() + (d.handshakeMs ?? 20_000)
        const sleep = d.sleep ?? ((n: number) => new Promise((r) => setTimeout(r, n)))
        let h = d.host(t.id)
        while (h?.status !== 'online' && Date.now() < until) {
          await sleep(500)
          h = d.host(t.id)
        }
        if (h?.status === 'online') return { state: 'pass', say: 'The computer answered and the encrypted handshake worked.' }
        const error = h?.error ?? t.host.error
        // "Connecting to studio… (Couldn't find the computer online. …)": the part in brackets
        const said = error?.match(/^Connecting to .+?… \((.+)\)$/s)?.[1] ?? error
        const cause = causeOf(error)
        const fix =
          cause === 'signed-out'
            ? 'The computer no longer knows this phone. Pair it again: run inyon pair there and scan the code.'
            : cause === 'handshake'
              ? "Something other than your computer answered at one of its addresses (another device has that IP now). Run inyon pair on the computer to refresh them."
              : 'Check the computer is on and inyon link runs there (inyon service status). If it uses a VPN, turn the VPN on here too.'
        return { state: 'fail', say: said ?? "The computer didn't answer.", fix, detail: error, ...(cause === 'signed-out' && { action: 'pair-computer' as const }) }
      },
    },
  ]
}

function batteryStep(d: CheckDeps): Plan {
  return {
    id: 'battery',
    title: 'Battery limits',
    doing: "Checking Android's battery limits…",
    run: async () => {
      const p = await d.power?.().catch(() => undefined)
      if (!p) return { state: 'skip', say: "Couldn't read Android's battery settings." }
      const detail = Object.entries(p).map(([k, v]) => `${k}: ${v}`).join('\n')
      if (d.keepAliveWanted && !d.keepAliveWanted()) return { state: 'warn', say: 'Stay connected is off, so Android pauses Inyon soon after you leave it.', fix: 'Turn Stay connected on.', action: 'stay-connected', detail }
      if (p.backgroundRestricted) return { state: 'warn', say: "Android restricts Inyon's background use.", fix: "Set Inyon's battery use to Unrestricted in Android's settings.", action: 'battery-settings', detail }
      if (p.ignoringOptimizations === false) return { state: 'warn', say: 'Android limits Inyon in the background, so connections drop once the screen has been off a while.', fix: 'Allow Inyon to run in the background.', action: 'battery', detail }
      if (p.keepAlive === false) return { state: 'warn', say: "Stay connected is on, but Android stopped its background connection.", fix: 'Start it again; opening Inyon also does.', action: 'stay-connected', detail }
      if (p.powerSave) return { state: 'warn', say: 'Battery saver is on, which cuts network use in the background.', fix: 'Turn battery saver off while you need alerts.', detail }
      return { state: 'pass', say: "Stay connected is on and Android doesn't limit Inyon.", detail }
    },
  }
}

/**
 * Runs the steps one after another, telling `update` each change. A step that fails and blocks
 * skips the ones after it, except the battery check, which stands on its own. `stopped` ends it
 * early (the sheet closed).
 */
export async function runChecks(t: Target, d: CheckDeps, update: (steps: Step[]) => void, stopped: () => boolean = () => false): Promise<Step[]> {
  const plans = plan(t, d)
  let steps: Step[] = plans.map((p) => ({ id: p.id, title: p.title, doing: p.doing, state: 'pending' }))
  const set = (i: number, s: Partial<Step>) => {
    steps = steps.map((x, j) => (j === i ? { ...x, ...s } : x))
    update(steps)
  }
  update(steps)
  const ctx: Ctx = {}
  let blocked = false
  for (const [i, p] of plans.entries()) {
    if (stopped()) break
    if (blocked && p.id !== 'battery') {
      set(i, { state: 'skip', say: SKIPPED })
      continue
    }
    set(i, { state: 'running' })
    let r: Result
    try {
      r = await p.run(ctx)
    } catch (e) {
      r = { state: 'fail', say: (e as Error).message || 'This check failed.', detail: errText(e) }
    }
    if (stopped()) break
    set(i, r)
    if (r.state === 'fail' && p.blocks) blocked = true
  }
  return steps
}
