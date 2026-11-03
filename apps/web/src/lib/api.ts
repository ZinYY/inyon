import { AGENT_CATALOG, type AgentCatalog, type AgentKind, type ChatItem, type CompanionPairCode, type HeldPrompt, type PairedCompanion, type PageEntry, type PageList, type PageOpen, type PageRegister, type PastConversation, type ProvidersReport, type SessionMark, type SleepRule, type DelayedSend, type SlashCommand, type ToolResultMeta } from '@inyon/protocol'
import { hubOrigin } from './hub.ts'
import { isPhone, phone } from './phone.ts'
import { checkPlain, hubToken, isHubHost, splitHostId, type PairedHub } from './hubs.ts'
import { NoChunkedUploads, retryable, uploadChunked, UploadError, type Uploaded } from './upload.ts'

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message)
  }
}

/** The paired hub a request about one of its machines (or their pages) belongs to, and its address there. */
function hubTarget(url: URL): { hub: PairedHub; url: string } | undefined {
  const m = url.pathname.match(/^\/api\/(hosts|pages)\/([^/]+)(.*)$/)
  if (!m) return
  const { hub, id } = splitHostId(decodeURIComponent(m[2]!))
  if (!hub) return
  return { hub, url: `${hub.url}/api/${m[1]}/${encodeURIComponent(id)}${m[3]}${url.search}` }
}

/**
 * A request about one of a paired hub's machines goes to that hub, with the phone's token. The
 * body goes as bytes read in advance: handed on as a stream, Chromium (and so the Android app)
 * sends it only over HTTP/2 and fails it on a hub's HTTP/1.1 before it leaves the phone.
 */
async function toHub(req: Request, { hub, url }: { hub: PairedHub; url: string }): Promise<Response> {
  // the token goes only where plain http is safe (hubs.ts), and only if this device still has it
  try {
    await checkPlain(hub.url)
  } catch (e) {
    throw new ApiError(0, (e as Error).message)
  }
  const token = await hubToken(hub.id)
  if (!token) throw new ApiError(401, `${hub.name} no longer knows this phone. Pair it again.`)
  const headers = new Headers(req.headers)
  headers.set('authorization', `Bearer ${token}`)
  const body = req.body ? await req.arrayBuffer() : undefined
  try {
    return await fetch(url, { method: req.method, headers, body, credentials: 'omit', signal: req.signal })
  } catch {
    // nothing came back: name the hub rather than say the browser's "Failed to fetch"
    if (req.signal.aborted) throw new ApiError(0, `${hub.name} took too long to answer`)
    throw new ApiError(0, `Couldn't reach ${hub.name}. Check this phone's connection to it.`)
  }
}

/** The hub over HTTP in a browser; the core running inside the phone app; a paired hub for its machines. */
async function send(req: Request): Promise<Response> {
  const hub = hubTarget(new URL(req.url))
  if (hub) return toHub(req, hub)
  if (!isPhone()) return fetch(req)
  const rt = await phone()
  // an in-process call can't be cut off, but the caller's timeout still applies
  return new Promise((resolve, reject) => {
    req.signal.addEventListener('abort', () => reject(new ApiError(0, 'The server took too long to answer')), { once: true })
    rt.fetch(req).then(resolve, reject)
  })
}

/** A request to a paired hub itself (not about one of its machines), with the phone's token. */
async function hubRequest<T>(hub: PairedHub, method: string, path: string, timeoutMs = 15_000): Promise<T> {
  const url = `${hub.url}${path}`
  const res = await toHub(new Request(url, { method, signal: AbortSignal.timeout(timeoutMs) }), { hub, url })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new ApiError(res.status, data.error ?? `Request failed (${res.status})`)
  return data as T
}

async function request<T>(method: string, url: string, body?: unknown, timeoutMs?: number): Promise<T> {
  const res = await send(new Request(new URL(url, location.href), {
    method,
    credentials: 'same-origin',
    headers: body !== undefined ? { 'content-type': 'application/json' } : {},
    body: body !== undefined ? JSON.stringify(body) : undefined,
    signal: timeoutMs ? AbortSignal.timeout(timeoutMs) : undefined,
  }))
  const data = await res.json().catch(() => ({}))
  if (!res.ok) {
    // a paired hub refusing the phone's token doesn't sign this app out
    if (res.status === 401 && !url.endsWith('/login') && !hubTarget(new URL(url, location.href))) window.dispatchEvent(new Event('inyon:signed-out'))
    throw new ApiError(res.status, data.error ?? `Request failed (${res.status})`)
  }
  return data as T
}

/** Pane ids travel without the leading "%". */
export const paneSlug = (pane: string) => pane.replace(/^%/, '')
export const paneFromSlug = (slug: string) => (slug.startsWith('%') ? slug : `%${slug}`)

const h = (id: string) => `/api/hosts/${encodeURIComponent(id)}`
const pg = (host: string, slug?: string) => `/api/pages/${encodeURIComponent(host)}${slug ? `/${encodeURIComponent(slug)}` : ''}`
const panePath = (host: string, pane: string) => `${h(host)}/panes/${paneSlug(pane)}`
const qs = (o: Record<string, string>) => `?${new URLSearchParams(o)}`

export type { Uploaded } from './upload.ts'

/** What PATCH …/organize changes on a session; a field left out stays as it is. */
export interface SessionPatch {
  pinned?: boolean
  parked?: boolean | null
  snoozedUntil?: number | null
  snoozeNotice?: boolean | null
  tags?: string[] | null
  note?: string | null
  closeWhenDone?: boolean
}

export interface UploadUsage {
  bytes: number
  files: number
  expired: number
  dir: string
  /** the retention setting `expired` counts with (from 0.20.2; older hubs leave it out) */
  days?: number
}

/** One folder Inyon looks in for temp files (`inyon temp`): Claude Code's temp folder, its paste cache, … */
export interface TempRoot {
  id: string
  agent: string
  label: string
  path: string
  size: number
  items: number
  exists: boolean
}

/** One thing in a temp folder that can go as a whole: a Claude session's folder, an upload day, a file. */
export interface TempItem {
  id: string
  root: string
  path: string
  name: string
  kind: 'dir' | 'file'
  size: number
  files: number
  /** ms, the newest change inside */
  modified: number
  /** changed in the last 30 minutes: an agent may still be using it */
  recent: boolean
  /** up to four images inside, newest first */
  images: string[]
}

export interface TempListing {
  v: number
  host: string
  roots: TempRoot[]
  items: TempItem[]
}

export interface TempFile {
  path: string
  rel: string
  size: number
  modified: number
  kind: 'image' | 'text' | 'other'
}

export interface TempPreview {
  path: string
  mime: string
  size: number
  text?: string
  base64?: string
  truncated: boolean
}

export interface TempRemoved {
  removed: string[]
  freed: number
  failed: [path: string, why: string][]
}

export interface ConfigFile {
  agent: AgentKind
  label: string
  path: string
  format: 'json' | 'toml' | 'md'
  exists: boolean
  size: number
  mtime?: number
}

export interface SetupCheck {
  id: string
  label: string
  ok: boolean
  detail?: string
  fix?: { label: string; action: string }
  manual?: string
}

export interface SignInScreen {
  lines: string[]
  links: string[]
  code?: string
  input?: { label: string; secret?: boolean }
  exit?: number
  /** what the command said went wrong, when it did */
  error?: string
  menu?: { question?: string; options: { n: number; label: string; detail?: string; selected: boolean }[] }
}

export interface SetupAgent {
  kind: AgentKind
  installed: boolean
  version?: string
  signedIn: boolean
  /** the newest published version, when the hub could ask the registry */
  latest?: string
  outdated?: boolean
  /** how it was installed, which decides how it's updated */
  via?: 'npm' | 'installer'
  /** an npm install under its own folder, updated in place */
  prefix?: string
  /** OpenCode: the providers connected on the machine */
  providers?: string[]
}

export interface SetupReport {
  os: string
  pkg?: string
  sudo: boolean
  checks: SetupCheck[]
  agents: SetupAgent[]
  keepSession: boolean
}

/** What a setup step came back with; `manual` and `hint` only when it failed. */
export interface SetupResult {
  ok: boolean
  output?: string
  pane?: string
  version?: string
  latest?: string
  manual?: string
  hint?: string
}

export interface ServerForm {
  name: string
  target: string
  port?: number
  /** set when the phone connects itself; a paired computer checks the key on its own */
  hostKey?: string
  auth?: 'phone-key' | 'password' | 'key' | 'none'
  /** the paired computer that opens the connection ('' for the phone itself) */
  via?: string
  password?: string
  privateKey?: string
  passphrase?: string
}

export interface HubSettings {
  uploads: { retentionDays: number; maxMB: number; maxTotalMB: number; longPasteLines: number }
  autoResume?: boolean
  /** joined machines may hold messages for agents with `inyon send` */
  machinesSend?: boolean
  /** your own agents; missing until the first is saved, and the Bun hub never keeps them */
  customAgents?: CustomAgent[]
}

/** Any command, run and read as the built-in agent it works like (Settings › Agents). */
export interface CustomAgent {
  /** from the name, set by the hub */
  id: string
  name: string
  command: string
  like: AgentKind
}

/** The hub's own update (hub mode only; see crates/inyon-hub/src/hub_update.rs). */
export interface HubUpdate {
  current: string
  latest?: string
  behind?: number
  canUpdate: boolean
  reason?: string
  checkedAt: number
  running?: boolean
  error?: string
  log?: string
  restartNeeded?: boolean
}

export interface DirEntry {
  name: string
  dir: boolean
  size: number
  /** seconds since 1970 */
  mtime: number
}

export type FileView =
  | { kind: 'text'; path: string; text: string; size: number; truncated: boolean }
  | { kind: 'image'; path: string; type: string; data: string; size: number }
  | { kind: 'binary'; path: string; size: number }
  | { kind: 'large'; path: string; size: number }

export interface GitFile {
  path: string
  from?: string
  status: 'M' | 'A' | 'D' | 'R' | '?'
  add: number
  del: number
}

export interface GitState {
  repo: boolean
  root?: string
  branch?: string
  remote?: boolean
  upstream?: string
  ahead?: number
  behind?: number
  files: GitFile[]
}

type Hunk = NonNullable<ToolResultMeta['patch']>[number]

/**
 * How long the app waits for a machine's temp files. The hub gives up on the machine after 60 s
 * (100 s for a delete) and says why, so these wait a little longer to get that answer.
 */
export const TEMP_WAIT = 70_000
export const TEMP_DELETE_WAIT = 110_000

export type SendFailure = { at: number; host: string; pane: string; agent?: string | null; error: string; chars: number; screen: string[] }

/** The companion agent (`GET /api/companion`): its agent while it's on, its pane while it runs. Rust hub only. */
export interface CompanionStatus {
  agent: 'claude' | 'codex' | null
  host: string
  pane: string | null
}

export const api = {
  /** `version` and `commit` say what the hub runs, once signed in */
  session: () => request<{ authed: boolean; setup?: boolean; setupCode?: boolean; version?: string; commit?: string }>('GET', '/api/session'),
  // a new hub: the first visit picks the password
  createPassword: (password: string, code?: string) => request<{ ok: boolean }>('POST', '/api/setup', { password, code }),
  changePassword: (current: string, next: string) => request<{ ok: boolean }>('POST', '/api/password', { current, next }),
  login: (password: string) => request<{ ok: boolean }>('POST', '/api/login', { password }),
  logout: () => request('POST', '/api/logout').catch(() => {}),
  addHost: (b: { name: string; target: string; port?: number; identityFile?: string }) => request<{ id: string }>('POST', '/api/hosts', b),
  // the phone app: servers it SSHes into itself
  addServer: (b: ServerForm) => request<{ id: string }>('POST', '/api/hosts', b),
  editServer: (id: string, b: Partial<ServerForm>) => request('PATCH', h(id), b),
  probe: (target: string, port?: number) => request<{ fingerprint: string; keyType: string }>('POST', '/api/phone/probe', { target, port }, 20_000),
  phoneKey: () => request<{ publicKey: string }>('GET', '/api/phone/key'),
  /** `machineId` from 0.20.3: the computer's own id, as `inyon pair` shows its start */
  pairLaptop: (link: string) => request<{ id: string; machineId?: string }>('POST', '/api/phone/pair', { link }, 60_000),
  updateHost: (id: string, b: Record<string, unknown>) => request('PATCH', h(id), b),
  repair: (id: string) => request<{ id: string; token: string; command: string; commands?: { unix: string; windows: string } }>('POST', `${h(id)}/pair`, { origin: hubOrigin() }),
  pair: (name: string) => request<{ id: string; token: string; command: string; commands?: { unix: string; windows: string } }>('POST', '/api/hosts/pair', { name, origin: hubOrigin() }),
  /** `untold`, on the phone: the computer was out of reach, so it trusts the phone until it hears */
  removeHost: (id: string) => request<{ ok?: boolean; untold?: string }>('DELETE', h(id)),
  integration: (id: string, enable: boolean) => request('POST', `${h(id)}/integration`, { enable }),
  /** brings the machine's inyon up to date (it restarts and reconnects by itself) */
  nodeUpdate: (id: string) => request('POST', `${h(id)}/node/update`),
  /** `signIn`: a terminal-first agent's sign-in command instead of the agent, in a terminal to sign in from */
  launch: (id: string, b: { session?: string; name?: string; cwd?: string; agent: AgentKind | 'shell'; custom?: string; model?: string; mode?: string; prompt?: string; signIn?: boolean }) =>
    request<{ pane: string }>('POST', `${h(id)}/launch`, b),
  /** `steer`: while Codex works, hand it the message now (Enter) instead of queueing it for after the turn (Tab) */
  send: (host: string, pane: string, text: string, requestId?: string, steer?: boolean) => request('POST', `${panePath(host, pane)}/send`, { text, requestId, ...(steer && { steer }) }, 310_000),
  /** a model, effort or goal change from the app's controls: sent like a message, never shown as one */
  setting: (host: string, pane: string, text: string) => request('POST', `${panePath(host, pane)}/send`, { text, setting: true }, 310_000),
  /** `question` is the promptKey of the question shown, so an answer that arrives late isn't taken for the next one */
  answer: (host: string, pane: string, option: number, text?: string, question?: string) =>
    request('POST', `${panePath(host, pane)}/answer`, { option, text, question }),
  interrupt: (host: string, pane: string) => request('POST', `${panePath(host, pane)}/interrupt`),
  // messages the hub holds until the agent is free (from 0.22)
  hold: (host: string, pane: string, text: string) => request<HeldPrompt>('POST', `${panePath(host, pane)}/held`, { text }),
  editHeld: (host: string, pane: string, id: string, b: { text?: string; to?: number }) => request('PATCH', `${panePath(host, pane)}/held/${encodeURIComponent(id)}`, b),
  dropHeld: (host: string, pane: string, id: string) => request('DELETE', `${panePath(host, pane)}/held/${encodeURIComponent(id)}`),
  dropChosen: (host: string, pane: string) => request('DELETE', `${panePath(host, pane)}/chosen`),
  sendHeld: (host: string, pane: string, id: string) => request('POST', `${panePath(host, pane)}/held/${encodeURIComponent(id)}/send`, undefined, 310_000),
  rename: (host: string, pane: string, name: string) => request('PUT', `${panePath(host, pane)}/name`, { name }),
  /** pins, parks, snoozes, tags or notes a session (from 0.24); null clears a field, and the answer is the mark after it */
  organize: (host: string, pane: string, patch: SessionPatch) => request<{ session: SessionMark | null }>('PATCH', `${panePath(host, pane)}/organize`, patch),
  /** a message the hub sends at `at` (unix ms), once the agent is free then */
  /** `conv`: the conversation it's for, when it's put back (Undo); else the pane's own. */
  sendLater: (host: string, pane: string, text: string, at: number, conv?: string) => request<DelayedSend>('POST', `${panePath(host, pane)}/sends`, { text, at, ...(conv && { conv }) }),
  /** A waiting message goes now: off the schedule and into the held queue in one step. */
  sendNow: (host: string, id: string) => request<{ pane: string }>('POST', `${h(host)}/sends/${encodeURIComponent(id)}/now`),
  cancelSend: (host: string, id: string) => request('DELETE', `${h(host)}/sends/${encodeURIComponent(id)}`),
  /** puts this hub's idle agents to sleep after `minutes`; null turns it off */
  setSleep: (minutes: number | null, scope = 'all') => request<SleepRule[]>('PUT', '/api/organize/sleep', { scope, minutes }),
  /** renames the tmux session itself (`session` is tmux's id, `$52`); answers the name tmux took (from 0.22) */
  renameTmuxSession: (host: string, session: string, name: string) =>
    request<{ name: string }>('PUT', `${h(host)}/sessions/${encodeURIComponent(session.replace(/^\$/, ''))}/name`, { name }),
  reconnect: (host: string, pane: string) => request('POST', `${panePath(host, pane)}/reconnect`),
  restart: (host: string, pane: string) => request('POST', `${panePath(host, pane)}/restart`),
  exit: (host: string, pane: string) => request('POST', `${panePath(host, pane)}/exit`),
  /** the agent exits, then its tmux pane closes (and its window or session, if that was the last) */
  end: (host: string, pane: string) => request<{ closed: 'pane' | 'window' | 'session' }>('POST', `${panePath(host, pane)}/end`),
  keys: (host: string, pane: string, keys: string[]) => request('POST', `${panePath(host, pane)}/keys`, { keys }),
  rewind: (host: string, pane: string, text?: string) => request('POST', `${panePath(host, pane)}/rewind`, { text }, 60_000),
  mode: (host: string, pane: string, mode: string) => request('POST', `${panePath(host, pane)}/mode`, { mode }),
  rc: (host: string, pane: string, on: boolean) => request('POST', `${panePath(host, pane)}/rc`, { on }),
  /** Claude Code's ultracode switch, for the session */
  ultracode: (host: string, pane: string, on: boolean) => request('POST', `${panePath(host, pane)}/ultracode`, { on }),
  input: (host: string, pane: string, text: string) => request('POST', `${panePath(host, pane)}/input`, { text }),
  stopTask: (host: string, task: { id: string; path?: string }) => request<{ stopped: boolean; detail: string }>('POST', `${h(host)}/tasks/stop`, { id: task.id, path: task.path }),
  start: (host: string, pane: string, agent: AgentKind, resume?: string) => request('POST', `${panePath(host, pane)}/start`, { agent, resume }),
  /** the agent's past conversations in the pane's folder; `agent` says whose when none is running */
  conversations: (host: string, pane: string, agent?: AgentKind) =>
    request<{ agent: AgentKind; cwd: string; conversations: PastConversation[] }>('GET', `${panePath(host, pane)}/conversations${agent ? qs({ agent }) : ''}`, undefined, 45_000),
  /** the agent exits (the pane stays) and starts again on conversation `id` */
  switchConversation: (host: string, pane: string, agent: AgentKind, id: string) => request<{ switched: boolean }>('POST', `${panePath(host, pane)}/switch`, { agent, id }, 120_000),
  refresh: () => request<{ ok: true }>('POST', '/api/refresh', undefined, 15_000),
  commands: (host: string, pane: string) => request<SlashCommand[]>('GET', `${panePath(host, pane)}/commands`),
  models: (host: string, pane: string) => request<AgentCatalog>('GET', `${panePath(host, pane)}/models`, undefined, 25_000),
  files: (host: string, pane: string) => request<{ cwd: string; files: string[] }>('GET', `${panePath(host, pane)}/files`),
  /** the session's folder, read only: one folder's entries, or one file to look at */
  dir: (host: string, pane: string, path: string) => request<{ cwd: string; path: string; entries: DirEntry[] }>('GET', `${panePath(host, pane)}/dir${qs({ path })}`, undefined, 20_000),
  file: (host: string, pane: string, path: string) => request<FileView>('GET', `${panePath(host, pane)}/file${qs({ path })}`, undefined, 40_000),
  /** its git working tree against the last commit */
  git: (host: string, pane: string) => request<GitState>('GET', `${panePath(host, pane)}/git`, undefined, 25_000),
  gitDiff: (host: string, pane: string, path: string) => request<{ path: string; binary: boolean; hunks: Hunk[] }>('GET', `${panePath(host, pane)}/git/diff${qs({ path })}`, undefined, 25_000),
  gitCommit: (host: string, pane: string, message: string) => request<{ commit: string }>('POST', `${panePath(host, pane)}/git/commit`, { message }, 70_000),
  gitPush: (host: string, pane: string) => request('POST', `${panePath(host, pane)}/git/push`, undefined, 100_000),
  /** a copy of the conversation in a new window next to this one */
  fork: (host: string, pane: string) => request<{ pane: string }>('POST', `${panePath(host, pane)}/fork`, undefined, 30_000),
  signIn: (id: string, agent: AgentKind) => request<{ pane: string }>('POST', `${h(id)}/signin`, { agent }),
  signInScreen: (id: string, pane: string) => request<SignInScreen>('GET', `${h(id)}/signin/${paneSlug(pane)}`),
  signInSend: (id: string, pane: string, b: { text?: string; keys?: string[]; option?: number }) => request('POST', `${h(id)}/signin/${paneSlug(pane)}`, b),
  signInStop: (id: string, pane: string) => request('DELETE', `${h(id)}/signin/${paneSlug(pane)}`),
  /** OpenCode's model providers on a machine; `pane` names a session whose own server can answer */
  providers: (id: string, pane?: string) => request<ProvidersReport>('GET', `${h(id)}/providers${pane ? qs({ pane: paneSlug(pane) }) : ''}`, undefined, 120_000),
  /** runs `opencode auth login` for that provider on the machine; the sign-in sheet drives the pane it returns */
  connectProvider: (id: string, provider: string) => request<{ pane: string }>('POST', `${h(id)}/providers/${encodeURIComponent(provider)}`),
  disconnectProvider: (id: string, provider: string) => request('DELETE', `${h(id)}/providers/${encodeURIComponent(provider)}`, undefined, 60_000),
  // the agent catalog this app knows: a query value, since a header would need every paired hub to allow it
  setup: (id: string) => request<SetupReport>('GET', `${h(id)}/setup?catalog=${AGENT_CATALOG}`),
  runSetup: (id: string, action: string) => request<SetupResult>('POST', `${h(id)}/setup`, { action }),
  keepSession: (id: string, keepSession: boolean) => request('POST', `${h(id)}/setup`, { keepSession }),
  hubUpdate: (refresh = false) => request<HubUpdate>('GET', `/api/hub/update${refresh ? '?refresh=1' : ''}`, undefined, 60_000),
  startHubUpdate: () => request<{ started: boolean }>('POST', '/api/hub/update'),
  settings: () => request<HubSettings>('GET', '/api/settings'),
  /** sends that failed, newest first, each with the bottom of the screen it failed on (from 0.22) */
  sendFailures: () => request<SendFailure[]>('GET', '/api/send-failures'),
  push: (b: { subscription: PushSubscriptionJSON; vapid: { privateJwk: JsonWebKey; subject: string }; done: boolean; pages?: boolean }) => request('POST', '/api/push', b),
  unpush: (endpoint: string) => request('DELETE', '/api/push', { endpoint }),
  pushTest: (endpoint: string) => request<{ sent: number }>('POST', '/api/push/test', { endpoint }, 30_000),
  saveSettings: (s: Partial<HubSettings>) => request<HubSettings>('PUT', '/api/settings', s),
  companion: () => request<CompanionStatus>('GET', '/api/companion'),
  /** starting types the agent into a new tmux session, which takes a few seconds */
  startCompanion: (agent: 'claude' | 'codex') => request<CompanionStatus>('POST', '/api/companion', { agent }, 60_000),
  stopCompanion: () => request('DELETE', '/api/companion'),
  uploadUsage: (id: string) => request<UploadUsage>('GET', `${h(id)}/uploads`),
  cleanUploads: (id: string, olderThanDays: number) => request<UploadUsage>('POST', `${h(id)}/uploads/clean`, { olderThanDays }),
  configs: (id: string) => request<ConfigFile[]>('GET', `${h(id)}/configs`),
  readConfig: (id: string, path: string) => request<{ path: string; content: string }>('GET', `${h(id)}/config${qs({ path })}`),
  writeConfig: (id: string, path: string, content: string) => request('PUT', `${h(id)}/config`, { path, content }),
  // ---- temp files agents leave on a machine, through its `inyon temp` (walking gigabytes takes a while) ----
  tempList: (id: string) => request<TempListing>('GET', `${h(id)}/temp`, undefined, TEMP_WAIT),
  tempFiles: (id: string, path: string) => request<{ path: string; files: TempFile[]; truncated: boolean }>('GET', `${h(id)}/temp/files${qs({ path })}`, undefined, TEMP_WAIT),
  /** `max`: the largest image sent whole; a bigger one comes back without its bytes */
  tempPreview: (id: string, path: string, max?: number) => request<TempPreview>('GET', `${h(id)}/temp/preview${qs(max ? { path, max: String(max) } : { path })}`, undefined, TEMP_WAIT),
  tempDelete: (id: string, paths: string[]) => request<TempRemoved>('POST', `${h(id)}/temp/delete`, { paths }, TEMP_DELETE_WAIT),
  subagents: (id: string, transcript: string) =>
    request<{ agentId: string; toolUseId?: string; type?: string; description?: string; path: string }[]>('GET', `${h(id)}/subagents${qs({ transcript })}`),
  transcript: (id: string, path: string) => request<ChatItem[]>('GET', `${h(id)}/transcript${qs({ path })}`),
  taskOutput: (id: string, task: { id: string; path?: string }) => request<{ path: string; output: string }>('GET', `${h(id)}/task-output${qs({ path: task.path ?? '', id: task.id })}`),

  // ---- pages: sites an agent made, served by the hub under a token link ----
  /** the pages of the hub this app came from */
  pages: () => request<PageList>('GET', '/api/pages', undefined, 20_000),
  /** a paired hub's pages (the phone app), with the phone's token */
  hubPages: (hub: PairedHub) => hubRequest<PageList>(hub, 'GET', '/api/pages', 20_000),
  /** a link to the page, good for 12 hours; `url` is a path on the page's hub */
  openPage: (host: string, slug: string) => request<PageOpen>('POST', `${pg(host, slug)}/open`, undefined, 20_000),
  /** every link to the page stops working */
  revokePage: (host: string, slug: string) => request<{ ok: true }>('POST', `${pg(host, slug)}/revoke`),
  /** adds a folder with an index.html as a page */
  addPage: (host: string, b: PageRegister) => request<PageEntry>('POST', pg(host), b, 20_000),
  /** takes a page off the list; its folder stays */
  removePage: (host: string, slug: string) => request<{ ok: true }>('DELETE', pg(host, slug)),
  /** a file of a page on a computer paired with the phone: the core answers `/v/<token>/…` itself (lib/pageBridge.ts) */
  pageFile: (path: string, method: 'GET' | 'HEAD') => send(new Request(new URL(path, location.href), { method, credentials: 'omit' })),

  // ---- phones paired with this hub (the hub's web app shows the code; the app claims it) ----
  /** A one-time code for a phone to pair with this hub; needs a recent sign-in. */
  phonePair: (origin: string) => request<{ id: string; code: string; link: string; expiresAt: number }>('POST', '/api/phones/pair', { origin }),
  phones: () => request<{ id: string; name: string; pairedAt: number; lastSeen?: number; thisPhone?: boolean }[]>('GET', '/api/phones'),
  forgetPhone: (id: string) => request<unknown>('DELETE', `/api/phones/${encodeURIComponent(id)}`),
  // ---- companion apps (Legato and the like) pair with a short code the hub shows ----
  companionPair: () => request<CompanionPairCode>('POST', '/api/companions/pair', {}),
  companions: () => request<PairedCompanion[]>('GET', '/api/companions'),
  forgetCompanion: (id: string) => request<unknown>('DELETE', `/api/companions/${encodeURIComponent(id)}`),

  /**
   * Sends a file to a machine a chunk at a time, resuming after a dropped connection (hubs from
   * 0.24), or the old way to an older paired hub. `onProgress` hears the share the machine has, and
   * `waiting` while the connection is down. Aborting `signal` stops it and deletes what arrived.
   */
  upload(host: string, pane: string | undefined, file: File, onProgress?: (fraction: number, waiting?: boolean) => void, signal?: AbortSignal): Promise<Uploaded> {
    const base = new URL(`${h(host)}/uploads`, location.href).toString()
    return uploadChunked(send, base, pane ? paneSlug(pane) : undefined, file, { onProgress, signal }).catch((err) => {
      if (err instanceof NoChunkedUploads) return legacyUpload(host, pane, file, onProgress)
      if (err instanceof UploadError) throw new ApiError(err.status, err.message)
      if (err instanceof ApiError || signal?.aborted) throw err
      // fetch's own words ("Failed to fetch", "Load failed") differ by browser and say little
      throw new ApiError(0, retryable(err) ? 'The connection dropped' : (err as Error).message)
    })
  },
}

/** Uploads to a hub from before 0.24: whole in one request with progress, or in pieces. */
function legacyUpload(host: string, pane: string | undefined, file: File, onProgress?: (fraction: number) => void): Promise<Uploaded> {
  const form = new FormData()
  form.append('files', file, file.name)
  if (pane) form.append('pane', paneSlug(pane))
  // in the phone app the file goes over SSH from here, a piece at a time: a whole video at once
  // runs the app out of memory. A big file goes to the hub in pieces too, so it never holds it whole,
  // and so does any file for a paired hub's machine, since only a request through send() finds that hub.
  if (isPhone() || file.size > UPLOAD_PIECE || isHubHost(host)) return uploadInPieces(host, pane, file, onProgress)
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', `${h(host)}/uploads`)
    xhr.upload.onprogress = (e) => e.lengthComputable && onProgress?.(e.loaded / e.total)
    xhr.onload = () => {
      let data: any = {}
      try {
        data = JSON.parse(xhr.responseText)
      } catch {}
      if (xhr.status >= 200 && xhr.status < 300 && data.files?.[0]) resolve(data.files[0])
      else reject(new ApiError(xhr.status, data.error ?? `Upload failed (${xhr.status})`))
    }
    xhr.onerror = () => reject(new ApiError(0, 'Network error during upload'))
    xhr.send(form)
  })
}

export const UPLOAD_PIECE = 2 * 1024 * 1024

/**
 * Runs `fn` again when the connection hiccups (a link dropping, a server busy): up to three more
 * tries, waiting longer each time. Refusals (bad input, too large, not allowed) fail at once.
 */
/** A dropped connection or a busy server, as opposed to a refusal that a retry won't change. */
export function isTransient(err: unknown) {
  // fetch throws a TypeError when the network is gone
  if (err instanceof TypeError) return true
  if (!(err instanceof ApiError)) return false
  return err.status === 0 || err.status === 408 || err.status === 429 || err.status >= 500
}

export async function withRetries<T>(fn: () => Promise<T>, waits = [700, 2000, 5000]): Promise<T> {
  for (let i = 0; ; i++) {
    try {
      return await fn()
    } catch (err) {
      if (!isTransient(err) || i >= waits.length) throw err
      await new Promise((r) => setTimeout(r, waits[i]))
    }
  }
}

async function uploadInPieces(host: string, pane: string | undefined, file: File, onProgress?: (fraction: number) => void): Promise<Uploaded> {
  let saved: Uploaded | undefined
  for (let at = 0; at < file.size || at === 0; at += UPLOAD_PIECE) {
    const form = new FormData()
    form.append('files', new File([file.slice(at, at + UPLOAD_PIECE)], file.name, { type: file.type }), file.name)
    if (pane) form.append('pane', paneSlug(pane))
    form.append('total', String(file.size))
    form.append('offset', String(at))
    if (saved) form.append('path', saved.path)
    saved = await withRetries(async () => {
      const res = await send(new Request(new URL(`${h(host)}/uploads`, location.href), { method: 'POST', body: form }))
      const data = await res.json().catch(() => ({}))
      if (!res.ok || !data.files?.[0]) throw new ApiError(res.status, data.error ?? `Upload failed (${res.status})`)
      return data.files[0] as Uploaded
    })
    onProgress?.(file.size ? Math.min(1, (at + UPLOAD_PIECE) / file.size) : 1)
  }
  return saved!
}
