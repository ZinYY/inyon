// Types shared by the hub (crates/inyon-protocol is their Rust copy) and the client (apps/web).
import type { AgentKind } from './agents.ts'

export * from './agents.ts'
export * from './versions.ts'
export * from './backoff.ts'

/** local: the hub itself · ssh: the hub connects out over SSH · node: the machine connects in to the hub (no open ports) */
export type HostKind = 'local' | 'ssh' | 'node' | 'link'

export interface HostConfig {
  id: string
  name: string
  kind: HostKind
  /** ssh only: `user@host` or an alias from ~/.ssh/config */
  target?: string
  port?: number
  identityFile?: string
  /** ssh from the phone app: the server's host key fingerprint, confirmed when it was added */
  hostKey?: string
  /**
   * ssh from the phone app: how it signs in (the secret itself stays in the phone's keystore).
   * "none" is for servers that let you in without a key or password (Tailscale SSH, an account
   * with an empty password).
   */
  auth?: 'phone-key' | 'password' | 'key' | 'none'
  /**
   * ssh from the phone app: the id of a paired computer that opens the connection instead of the
   * phone, with its own keys and ~/.ssh/config, for servers only it can reach. `target` may then
   * be an alias from that config.
   */
  via?: string
  /** node only: sha256 of the pairing token (the token itself is never stored) */
  tokenHash?: string
  /** node only: when the machine first connected with its token (unset while the command hasn't run) */
  pairedAt?: number
  /** node only: when its current token was made; one that's never used stops working after a day */
  tokenIssuedAt?: number
  /** link only (a laptop the phone app paired with directly): its id, from `inyon pair` */
  machineId?: string
  /** link only: where it was last reachable, and the relays it uses */
  urls?: string[]
  relays?: string[]
  /** put the Claude integration back when something replaces ~/.claude/settings.json */
  keepIntegration?: boolean
}

export type HostStatus = 'connecting' | 'online' | 'error'

export type AgentState = 'idle' | 'working' | 'waiting' | 'unknown'

/** Working, or idle with shells, monitors or agents still running in the background: something is happening. */
export const isBusy = (a: { state: AgentState; background?: string | null }) => a.state === 'working' || (a.state === 'idle' && !!a.background)

export interface RateWindow {
  pct: number
  /** unix seconds */
  resetsAt?: number
}

/** Live numbers the agent reports (Claude: its status line payload; Codex: token_count events). */
export interface AgentStatus {
  model?: string
  modelId?: string
  effort?: string
  fast?: boolean
  version?: string
  contextPct?: number
  contextUsed?: number
  contextSize?: number
  costUsd?: number
  durationMs?: number
  apiDurationMs?: number
  linesAdded?: number
  linesRemoved?: number
  fiveHour?: RateWindow
  sevenDay?: RateWindow
  /** prepaid credits (Codex): a balance, or unlimited */
  credits?: { balance?: string; unlimited?: boolean }
  /** plan name when the agent reports it */
  plan?: string
  outputStyle?: string
  /**
   * Why some numbers are missing: the account changed since the limits were reported, or the
   * conversation was compacted since the context was. Both refill on the agent's next reply.
   */
  stale?: { limits?: 'account'; context?: 'compacted' }
  /** unix ms of the last update */
  updatedAt: number
}

export interface AgentPaneInfo {
  kind: AgentKind
  /** a custom agent's name (Settings › Agents); `kind` is the agent it works like */
  custom?: string
  pid: number
  sessionId?: string
  transcriptPath?: string
  /** where the pane → session mapping came from */
  mappedBy?: 'hook' | 'cwd' | 'resume' | 'process'
  state: AgentState
  /** e.g. "manual", "plan", "accept edits" */
  mode?: string
  /** short text for the overview, e.g. the permission being asked */
  waitingFor?: string
  /** answers offered by the prompt on screen (single-choice prompts only), for quick replies */
  options?: { n: number; label: string }[]
  /** working spinner text */
  activity?: string
  /** background shells or tasks still running while the agent itself waits, e.g. "1 shell" */
  background?: string
  /** Claude Code: ultracode (a dynamic workflow for every task) is on for the session */
  ultracode?: boolean
  status?: AgentStatus
  /**
   * The integration is on but this session stopped sending live numbers (its settings changed
   * under it). Resuming the session fixes it; the hub does that itself once it's safe.
   */
  statusStopped?: boolean
  startedAt?: number
  /** the last turn ended on an error instead of finishing; gone with the next prompt */
  failure?: TurnFailure
}

/** Why a turn stopped (Claude Code's StopFailure hook). */
export interface TurnFailure {
  /**
   * "rate_limit", "billing_error", "authentication_failed", "overloaded", "server_error",
   * "invalid_request", "max_output_tokens" or "unknown"
   */
  error: string
  /** rate limits: when the full limit resets, unix seconds */
  resetsAt?: number
}

export interface PaneInfo {
  id: string // tmux pane id, e.g. "%12"
  index: number
  active: boolean
  command: string
  cwd: string
  title: string
  /** the name you gave this session in Inyon */
  label?: string
  width: number
  height: number
  dead: boolean
  agent?: AgentPaneInfo
  /** runs your companion agent (Settings › Companion); at most one pane across all machines */
  companion?: boolean
}

export interface WindowInfo {
  id: string // "@3"
  index: number
  name: string
  active: boolean
  activity: number
  panes: PaneInfo[]
}

export interface SessionInfo {
  id: string // "$1"
  name: string
  attached: number
  windows: WindowInfo[]
}

export type IntegrationState = 'unknown' | 'installed' | 'missing' | 'removed' | 'outdated'

/** Inyon's version, the same for the app, the hub and the inyon program (the root package.json's). */
export const VERSION = '0.24.0'

/**
 * A machine's inyon that can be updated from the app, or an update the app started. `latest` is the
 * hub's own version for a machine paired with the hub, the newest release for a phone-paired one.
 */
export interface NodeUpdate {
  latest: string
  /** false for an inyon (or tether-node) too old to update itself (its setup command has to be run again) */
  supported: boolean
  state?: 'updating' | 'updated' | 'failed'
  error?: string
}

export type MachineKind = 'server' | 'laptop' | 'desktop'

export interface HostState {
  config: HostConfig
  status: HostStatus
  error?: string
  /** hostname reported by the machine */
  hostname?: string
  /**
   * Claude Code integration: `removed` when its scripts are there but ~/.claude/settings.json no
   * longer points at them (the file was replaced), `outdated` when the scripts are an older version.
   */
  integration: IntegrationState
  /** Antigravity has its own settings and status wrapper. */
  agyIntegration?: IntegrationState
  tmux: 'unknown' | 'ok' | 'no-server' | 'missing'
  /** agent CLIs found on PATH */
  agents: AgentKind[]
  /** agents with a saved sign-in on the machine (unknown until the first probe) */
  signedIn?: AgentKind[]
  os?: string
  /** what the machine is, for its icon (detected on the machine) */
  machineKind?: MachineKind
  /** the inyon version a paired machine or laptop runs */
  nodeVersion?: string
  /**
   * what that machine's inyon warns about its connection to the hub (plain http, from a machine
   * added before 0.20), with how to fix it
   */
  nodeWarning?: string
  /** set when that inyon is behind, or while an update runs */
  nodeUpdate?: NodeUpdate
  latencyMs?: number
  sessions: SessionInfo[]
  updatedAt: number
  /** last successful check (ms); kept across hub restarts */
  lastSeen?: number
  /** when the machine stopped answering; its sessions are the last ones seen, saved on disk */
  offlineSince?: number
  /** plan limits of the accounts signed in there */
  limits?: AccountLimits[]
}

/** An agent's plan limits on a machine, read from the account it's signed in to, with or without a session running. */
export interface AccountLimits {
  kind: AgentKind
  fiveHour?: RateWindow
  sevenDay?: RateWindow
  plan?: string
  credits?: { balance?: string; unlimited?: boolean }
  /** limit resets the account has banked (Codex): counted, never used */
  resets?: number
  /** unix ms of the reading */
  updatedAt: number
}

// ---------- Chat (normalized Claude Code transcript) ----------

export type AssistantBlock =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: any }

export interface ToolResultMeta {
  /** Edit/Write/MultiEdit: unified hunks */
  patch?: { oldStart: number; oldLines: number; newStart: number; newLines: number; lines: string[] }[]
  filePath?: string
  /** Write: created a new file */
  created?: boolean
  stdout?: string
  stderr?: string
  interrupted?: boolean
  /** AskUserQuestion */
  answers?: Record<string, string>
  /** ExitPlanMode */
  plan?: string
  /** Read */
  numLines?: number
  /** Bash/Monitor run in the background: the task it started (see ChatInfo.tasks) */
  taskId?: string
}

export type ChatItem =
  | { kind: 'user'; seq: number; uuid: string; ts: string; text: string; images: number; queued?: boolean; shell?: boolean }
  | { kind: 'assistant'; seq: number; uuid: string; ts: string; msgId: string; model?: string; block: AssistantBlock }
  | { kind: 'tool_result'; seq: number; uuid: string; ts: string; toolUseId: string; isError: boolean; text: string; truncated?: boolean; meta?: ToolResultMeta }
  | { kind: 'command'; seq: number; uuid: string; ts: string; name: string; args: string; output?: string }
  | { kind: 'notice'; seq: number; uuid: string; ts: string; level: 'info' | 'warn' | 'error'; subtype: string; text: string }
  | { kind: 'turn_end'; seq: number; uuid: string; ts: string; durationMs: number }
  | { kind: 'goal'; seq: number; uuid: string; ts: string; condition: string; met: boolean; reason?: string; cleared?: boolean }

export interface BackgroundTask {
  id: string
  type: 'shell' | 'monitor' | 'workflow' | 'other'
  description: string
  command?: string
  status: 'running' | 'completed' | 'failed' | 'stopped'
  outputFile?: string
  summary?: string
  startedAt?: string
  endedAt?: string
}

export interface SubagentInfo {
  toolUseId: string
  type?: string
  description: string
  prompt?: string
  status: 'running' | 'done' | 'error'
  background?: boolean
  startedAt?: string
  transcriptPath?: string
}

export interface ArtifactInfo {
  url: string
  title?: string
  updatedAt?: string
}

export interface SessionStats {
  contextTokens?: number
  costUsd?: number
  durationMs?: number
  apiDurationMs?: number
  linesAdded?: number
  linesRemoved?: number
  turns: number
  compactions: number
  /** unix ms of the last compaction */
  compactedAt?: number
  /** unix ms of the event `contextTokens` came from */
  contextAt?: number
  startedAt?: string
}

/**
 * One message sent through Inyon. The hub knows the exact text it typed, so it is the one that
 * matches it to the agent's record; every device reads the outcome from here.
 * sending: being typed · sent: submitted, not yet in the agent's record · queued: in the agent's
 * queue while it works · delivered: in the conversation (`uuid` is that item) · failed: `error`
 */
export interface SendState {
  id: string
  text: string
  at: number
  state: 'sending' | 'sent' | 'queued' | 'delivered' | 'failed'
  uuid?: string
  error?: string
  /** a change made with the app's controls (model, effort, goal…): followed, but not shown as a message */
  setting?: boolean
}

/** A conversation the agent saved in a pane's folder, which it can be started again with. */
export interface PastConversation {
  /** what the agent's command line takes to resume it */
  id: string
  /** the agent's own title for it, a name given in Inyon, or else the first thing you asked */
  title: string
  /** last activity, ms since the epoch */
  at: number
  /** the conversation open in the pane now */
  current?: boolean
}

/**
 * Something the agent is writing right now, before it's in its record: a reply, its thinking, a
 * plan, or a running command's output (Codex, from 0.17.6). The record's item has the same id,
 * so the finished message takes the live one's place without a jump.
 */
export interface LiveItem {
  id: string
  kind: 'text' | 'thinking' | 'plan' | 'output'
  text: string
  turnId?: string
}

/** A message Inyon holds for a pane and types in once its agent is free (from 0.22): it can still
 * be edited, moved or dropped. `error` says why the last try didn't go, and holding stops there. */
export interface HeldPrompt {
  id: string
  text: string
  at: number
  error?: string
}

/** Controls picked in the app while the agent couldn't take them (from 0.22): Inyon makes each
 * pick once the agent is free, the last of each kind winning. The values are what was picked: a
 * model as the agent's `/model` takes it, an effort level, a mode id. `error` says why the last try
 * didn't take, and nothing more is tried until it's picked again or dropped. */
export interface ChosenControls {
  model?: string
  effort?: string
  mode?: string
  error?: string
}

export interface ChatInfo {
  /** Live metrics from transcript/API adapters. */
  status?: AgentStatus
  hostId: string
  paneId: string
  agent?: AgentKind
  title?: string
  sessionId?: string
  transcriptPath?: string
  cwd?: string
  model?: string
  gitBranch?: string
  version?: string
  /** the agent process is (still) running in the pane */
  running: boolean
  mappedBy?: 'hook' | 'cwd' | 'resume' | 'process'
  error?: string
  goal?: { condition: string; met: boolean; reason?: string; ts: string }
  rc?: { url: string; ts: string }
  /** the conversation's public link, while it's shared (OpenCode's /share) */
  share?: string
  /** messages typed while the agent was busy, not yet picked up */
  queue: string[]
  /** messages Inyon holds until the agent is free, in the order they go (from 0.22) */
  held?: HeldPrompt[]
  /** a model, effort or mode waiting for the agent to be free (from 0.22) */
  chosen?: ChosenControls
  /** the turn under way, for the working timer (Codex: from its records and its live events) */
  turn?: { id?: string; startedAt: number }
  /** recent sends and how far each got, matched by the hub against the agent's own record */
  sends?: SendState[]
  tasks: BackgroundTask[]
  subagents: SubagentInfo[]
  artifacts: ArtifactInfo[]
  stats: SessionStats
  mode?: string
  /** Claude Code: ultracode is on for the session */
  ultracode?: boolean
}

export interface PromptOption {
  n: number
  label: string
  detail?: string
  selected: boolean
  /** multi-select checkbox state; undefined for single-select */
  checked?: boolean
  /** what's set now, in a settings picker (Codex marks it "(current)") */
  current?: boolean
}

export interface ScreenPrompt {
  /** text above the options (title, command, question) */
  body: string[]
  /**
   * each body line is a whole line: the hub has joined the ones the terminal wrapped, so a line
   * break here is a real one (show it, don't join the lines into a paragraph)
   */
  wholeLines?: boolean
  options: PromptOption[]
  /** AskUserQuestion tab strip */
  tabs?: { label: string; done: boolean; current: boolean }[]
  footer?: string
  multi: boolean
  /** the option number of a free-text "Type something" entry */
  freeTextOption?: number
  /** options aren't numbered on screen: answer by moving the cursor (↑/↓) and pressing Enter */
  navigate?: boolean
  /** the dialog asks for typed input (e.g. "Paste code here if prompted >") */
  input?: { label: string }
  /** URLs shown in the dialog, re-joined when the terminal wrapped them */
  links?: string[]
  /** answered through the agent's API rather than keystrokes (OpenCode permissions and questions) */
  api?: boolean
  /**
   * The choices are toggled one by one and sent together: this is the label of the action that
   * sends them ("Confirm", or "Next" when another question follows). Answered with option 0.
   */
  confirm?: string
}

/** Option number that stands for a prompt's `confirm` action. */
export const CONFIRM_OPTION = 0

/**
 * Which question a prompt is: its text and its choices, whitespace aside (the screen rewraps). An
 * answer carries the key of the question it was given for, so a tap that arrives after that
 * question was answered elsewhere never lands on the next one, which often has the same numbers.
 */
export const promptKey = (p: Pick<ScreenPrompt, 'body' | 'options'>) =>
  [...p.body, ...p.options.map((o) => `${o.n}.${o.label}`)].join('\n').replace(/\s+/g, '')

export interface ScreenState {
  /** A recognizable input box is present, even with a custom footer. */
  inputVisible?: boolean
  /** a menu/prompt is waiting for an answer */
  prompt?: ScreenPrompt
  working: boolean
  /** spinner line, e.g. "Undulating… (12s · thinking)" */
  activity?: string
  /** when the current turn started (unix ms), read off the spinner's elapsed time (Codex) */
  since?: number
  mode?: string
  /** text sitting in Claude's input box (typed but not sent) */
  draft?: string
  /** Remote Control as the screen last reported it: its link while on, null once turned off (agy) */
  remote?: string | null
  /**
   * background shells, monitors or agents listed under the input box, which die if the agent
   * restarts: null when the footer was read and lists none, undefined when it wasn't visible
   */
  background?: string | null
  /** messages waiting to be sent after the current turn, when only the screen shows them (Antigravity) */
  queue?: string[]
  /** questions Codex asked while it works, waiting to be answered (it opens them with shift+←) */
  questions?: number
  /** model and effort from the agent's own footer, for sessions that don't report live numbers */
  model?: string
  effort?: string
  /** Claude Code's ultracode switch, as its input box shows it; undefined while the box isn't visible */
  ultracode?: boolean
  /** context and cost as the agent's own status row or sidebar prints them (OpenCode) */
  contextPct?: number
  contextUsed?: number
  costUsd?: number
  /** a menu of the agent's own open over the box, by its title ("Select model", "Select variant") */
  dialog?: string
  width: number
}

// ---------- OpenCode providers ----------

/** A model provider OpenCode knows, and whether this machine can use it. */
export interface OpenCodeProvider {
  id: string
  name: string
  connected: boolean
  /** a saved credential (`opencode auth login`), or an API key in the machine's environment */
  via?: 'credential' | 'env'
  /** the credential's kind, as `opencode auth list` prints it (api, oauth) */
  type?: string
  /** environment variables that connect it without a saved credential */
  env?: string[]
}

export interface ProvidersReport {
  providers: OpenCodeProvider[]
  updatedAt: number
  /** the provider list couldn't be read (OpenCode missing, or its server didn't start) */
  error?: string
}

// ---------- Phones paired with a hub ----------

/** A one-time code the hub's web app shows (as a QR, and as text to type) for a phone to pair with. */
export interface PhonePairCode {
  /** 8 characters of Crockford base32, shown as "K7Q4-MX2T" (format_short_id in crates/inyon-link) */
  id: string
  /** 6 digits */
  code: string
  /** what the QR holds: inyon://hub?u=<hub origin>&c=<id:code>&n=<hub name> */
  link: string
  /** unix ms; ten minutes after the code was made */
  expiresAt: number
}

/** POST /api/phones/claim: the phone hands in the code and says what to call it. */
export interface PhoneClaim {
  /** "ID:DIGITS", the id in any case */
  code: string
  name: string
}

export interface PhoneClaimResult {
  /** sent as `Authorization: Bearer` from then on; the hub keeps only its hash */
  token: string
  hub: { id: string; name: string; version: string }
}

/** GET /api/phones: a phone that paired, as the hub lists it. */
export interface PairedPhone {
  id: string
  name: string
  pairedAt: number
  lastSeen: number
  /** set on the phone that asked */
  thisPhone?: true
}

// ---------- Companion apps (Legato and the like), contract version 1 ----------

/** What a companion may do: read the overview, watch chats, answer prompts, send text. Nothing else. */
export type CompanionScope = 'overview' | 'chat' | 'answer' | 'send'

/** A short one-time code for a companion to type: six characters, no 0, O, 1, I or L. */
export interface CompanionPairCode {
  code: string
  /** unix ms; five minutes after the code was made */
  expiresAt: number
  /** from 1.1: the SHA-256 of the certificate the hub serves itself (its DER, lowercase hex), for the pairing link */
  tlsFingerprint?: string
}

/** POST /api/companions/claim, sent with `X-Inyon-Contract`. */
export interface CompanionClaim {
  /** any case, spaces and dashes ignored */
  code: string
  /** what the hub lists it as, e.g. "Legato on abhash-laptop" */
  name: string
  /** the app itself, e.g. "Legato for Windows" */
  app?: string
}

export interface CompanionClaimResult {
  /** sent as `Authorization: Bearer` from then on; the hub keeps only its hash */
  token: string
  hubId: string
  scopes: CompanionScope[]
  contractVersion: number
  /** from 1.1, when the hub serves HTTPS itself: the SHA-256 of its certificate's DER, lowercase hex, to pin */
  tlsFingerprint?: string
}

/** GET /api/companions: a companion app that paired, as the hub lists it. */
export interface PairedCompanion {
  id: string
  name: string
  app?: string
  scopes: CompanionScope[]
  contractVersion: number
  pairedAt: number
  lastSeen: number
}

/** GET /api/companions/me: the companion asking, and the hub it paired with. */
export interface CompanionMe {
  id: string
  name: string
  app?: string
  scopes: CompanionScope[]
  contractVersion: number
  hub: { id: string; name: string; version: string }
}

// ---------- Pages: sites an agent made, served by the hub under a token URL ----------

/** What a page's progress.json says, counted: tasks done of all tasks, and its `now` and `updated` lines. */
export interface PageProgress {
  done: number
  total: number
  /** the "working on" line */
  now?: string
  /** when progress.json was last written, as it says (an ISO time) */
  updated?: string
}

/** GET /api/pages: one page, on one machine. Its id is `<host>/<slug>`. */
export interface PageEntry {
  /** the machine's host id ("local" for the hub's own machine) */
  host: string
  /** `^[a-z0-9][a-z0-9-]{0,62}$`, unique on its machine */
  slug: string
  name: string
  /** the project folder: the git top level of the page's folder, or the folder itself */
  project: string
  /** who added it: an agent ("claude", "codex"…), "cli" for the inyon command, or "app" */
  by: string
  /** unix ms */
  added: number
  /** the site's folder, absolute; it holds index.html */
  folder: string
  /** left out when the site has no readable progress.json */
  progress?: PageProgress
  /** the Markdown notes in the site's `research/` folder, by file name (from 0.22.4); left out when there are none */
  notes?: string[]
  /** the agent conversation that added or found it (Claude Code's session id, Codex's thread id), from 0.22.5: the app shows that session's name over it */
  session?: string
  /** the name Inyon last gave that session, for when it isn't running */
  sessionName?: string
}

/** GET /api/pages */
export interface PageList {
  pages: PageEntry[]
}

/**
 * POST /api/pages/:host/:slug/open. `url` is a path on the hub that called (`/v/<token>/`); the app
 * puts the hub's own address in front of it.
 */
export interface PageOpen {
  url: string
  /** unix ms; twelve hours after it was made */
  expires: number
  /** how long the link lasts from when it was made, in ms; the app counts it on its own clock, which may not agree with the hub's */
  ttl: number
}

/** POST /api/pages/:host: add a folder as a page. The slug comes from the folder's name when left out. */
export interface PageRegister {
  /** absolute, and holds index.html */
  path: string
  name?: string
  slug?: string
}

/** One entry of ~/.inyon/pages.json, the machine's list of pages. */
export interface PageRecord {
  slug: string
  name: string
  path: string
  project: string
  by: string
  /** unix ms */
  added: number
}

/** ~/.inyon/pages.json. Written to a temp file and renamed, under pages.json.lock. */
export interface PagesFile {
  /** always 1 */
  v: number
  pages: PageRecord[]
}

/** What made a page's site; only Blueprint so far. */
export type PageKind = 'blueprint'

/** `<site>/.inyon-page.json`, the marker in a site's own folder. It is a dot file, so never served. */
export interface PageMarker {
  /** always 1 */
  v: number
  slug: string
  name: string
  kind: PageKind
  /** unix ms */
  created: number
}

// ---------- Organize: pins, parking, snoozes, tags, notes and projects, kept by the hub (from 0.24) ----------

/** A project's or a group's colour: the neutral ones, and a few muted ones. The app draws it on the project's mark and nowhere else. */
export type ProjectColor = 'silver' | 'ink' | 'sage' | 'teal' | 'blue' | 'violet' | 'rose'

/**
 * What the hub keeps about one session, and how it finds the session again: by its agent's
 * conversation id when known, else by its tmux pane id in the same tmux session, else by its place
 * (tmux session name, window and pane numbers, folder) after tmux restarted.
 */
export interface SessionMark {
  /** the hub's id for it: "s-" and 12 hex digits */
  id: string
  /** the machine */
  host: string
  /** the tmux pane it was last matched to ("%12") */
  pane: string
  /** matched to a pane on its machine now; the apps join a mark to a pane only while this is set */
  live: boolean
  /** the agent's conversation (Claude Code's session id, Codex's thread id), when known */
  conv?: string
  /** the tmux session's id ("$3") and name where it was last seen */
  tmuxId?: string
  tmuxName?: string
  /** tmux's window and pane numbers there, "2.0" */
  tmuxPlace?: string
  /** its folder when last seen */
  cwd?: string
  /** when it was last seen on its machine, unix ms */
  seen: number
  /** pinned, at this place among the pinned (0 first) */
  pinned?: number
  parked?: boolean
  /** hidden until then, unix ms; the hub clears it once it's past */
  snoozedUntil?: number
  /** raise a notice when the snooze ends */
  snoozeNotice?: boolean
  /** normalised: trimmed, no leading #, spaces squashed, lower case; 8 at most */
  tags?: string[]
  note?: string
  /** shown in place of the session's own name */
  title?: string
  /** a section of its own within its project ("Move to group") */
  section?: string
  /** the project it belongs to, whatever its folder; else it falls into one by its folder */
  project?: string
  /** end the session (as End session does) once its agent has finished its next piece of work */
  closeWhenDone?: boolean
  /** the hub saw it working since closeWhenDone was set, so the next idle ends it */
  closeSawWork?: boolean
}

/** A folder on one machine whose sessions belong to a project. */
export interface ProjectRoot {
  host: string
  /** absolute; a session in it or any folder under it belongs to the project */
  path: string
}

/** A project: a name, a colour and the folders its sessions live in, on any machine. */
export interface Project {
  /** "p-" and 12 hex digits */
  id: string
  name: string
  color: ProjectColor
  icon?: string
  roots: ProjectRoot[]
  /** the group it's in */
  group?: string
}

/** Projects shown together under one heading. */
export interface ProjectGroup {
  /** "g-" and 12 hex digits */
  id: string
  name: string
  color: ProjectColor
}

/** A named set of projects to look at on their own. */
export interface Space {
  /** "w-" and 12 hex digits */
  id: string
  name: string
  icon?: string
  projects: string[]
}

/** A message the hub types into a session at a set time, through the held queue: it waits for the agent to be free, then goes once. */
export interface DelayedSend {
  /** "d-" and 12 hex digits */
  id: string
  host: string
  pane: string
  conv?: string
  tmuxId?: string
  text: string
  /** when it goes, unix ms */
  at: number
  /** why it couldn't go; it stays until dismissed */
  error?: string
}

/** A prompt kept to send again: for every session, or for one project's. */
export interface SavedPrompt {
  /** "q-" and 12 hex digits */
  id: string
  name: string
  text: string
  /** the project it's for; none for every session */
  project?: string
  /** shown first */
  starred?: boolean
}

/** After how many idle minutes an agent is put to sleep (Exit to shell), for a project's sessions or, with scope "all", every other one. Off unless there's a rule. */
export interface SleepRule {
  /** a project's id, or "all" */
  scope: string
  minutes: number
}

/** GET /api/organize, the hub's `organize` record, and the `organize` of a v1 overview (without `prompts`). Lists are in the order the apps show them. */
export interface Organize {
  /** the record's version when the hub wrote it */
  v: number
  sessions: SessionMark[]
  projects: Project[]
  groups: ProjectGroup[]
  spaces: Space[]
  /** from version 2 */
  sends?: DelayedSend[]
  prompts?: SavedPrompt[]
  sleep?: SleepRule[]
}

// ---------- WebSocket protocol ----------

export type ClientMsg =
  | { t: 'sub.overview' }
  | { t: 'sub.chat'; host: string; pane: string }
  | { t: 'unsub.chat'; host: string; pane: string }
  | { t: 'chat.more'; host: string; pane: string; before: number }
  | { t: 'draft'; host: string; pane: string; text: string }
  | { t: 'term.open'; id: string; host: string; pane: string; cols: number; rows: number }
  | { t: 'term.in'; id: string; d: string }
  | { t: 'term.resize'; id: string; cols: number; rows: number }
  | { t: 'term.close'; id: string }
  | { t: 'ping' }
  /** the app went to the background (true) or came back: overviews only when an agent's state changes while it's quiet */
  | { t: 'quiet'; on: boolean }

export type ServerMsg =
  /** `organize` (from 0.24): how the sessions are organised; an older hub leaves it out */
  | { t: 'overview'; hosts: HostState[]; organize?: Organize }
  | { t: 'chat.reset'; key: string; info: ChatInfo; items: ChatItem[]; hasMore: boolean }
  | { t: 'chat.append'; key: string; items: ChatItem[] }
  | { t: 'chat.more'; key: string; items: ChatItem[]; hasMore: boolean }
  | { t: 'chat.info'; key: string; info: ChatInfo }
  | { t: 'chat.screen'; key: string; screen: ScreenState }
  /** what the agent is writing right now (the whole list each time; empty when nothing is) */
  | { t: 'chat.live'; key: string; items: LiveItem[] }
  /**
   * Reading what was said while nobody was watching (bytes of the transcript read so far, of how
   * many). The chat shows what it had until a chat.reset brings everything at once.
   */
  | { t: 'chat.progress'; key: string; done: number; total: number }
  /** the unsent message for a pane, shared by the app (every device) and the terminal's input box */
  | { t: 'chat.draft'; key: string; text: string; source: 'app' | 'terminal'; at: number }
  | { t: 'term.data'; id: string; d: string }
  | { t: 'term.exit'; id: string; reason?: string }
  | { t: 'notify'; host: string; pane: string; kind: 'waiting' | 'done' | 'offline' | 'online'; title: string; body: string; agent?: AgentKind }
  /** a page's progress board moved on: a task on it is done (from 0.22.4) */
  | { t: 'page.notify'; host: string; slug: string; title: string; body: string }
  | { t: 'pong' }
  | { t: 'companion.reply'; id: string; host: string; pane: string; text: string; error?: boolean; at: number }

export const chatKey = (host: string, pane: string) => `${host}:${pane}`
