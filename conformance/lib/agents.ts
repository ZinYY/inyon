import fs from 'node:fs'
import { Database } from 'bun:sqlite'
import path from 'node:path'
import { Tui, type Machine, type TuiOptions } from './machine.ts'
import { REPO } from './hub.ts'

/**
 * Agents on the fake machine, built from the captured screens in fixtures/screens (the same files
 * the hub's parser tests read) and transcripts written the way each agent writes its own.
 */

/** A captured screen, rows as tmux printed them. */
export const screen = (agent: string, name: string) => fs.readFileSync(path.join(REPO, 'fixtures/screens', agent, `${name}.txt`), 'utf8').split('\n')

/** Fixed ids and times for records, so transcripts read the same on every run. */
export const uuid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`
/** Records are dated 15 January 2026, well outside the window the normalizer treats as "now". */
export const at = (sec: number) => new Date(Date.UTC(2026, 0, 15, 9, 0, sec)).toISOString()

/** Where Claude keeps a folder's transcripts (claude_slug in crates/inyon-core/src/tmux/tmux.rs). */
export const claudeSlug = (cwd: string) => cwd.replace(/[^a-zA-Z0-9]/g, '-')

export const jsonl = (records: unknown[]) => records.map((r) => `${JSON.stringify(r)}\n`).join('')

// ---------- Claude Code ----------

const IDLE = screen('claude', 'idle-draft')
const SPINNER = screen('claude', 'working')[1]!

/**
 * Claude's screen around its input box, from claude/idle-draft.txt: the box holds `draft`, and the
 * fixture's "← 3 agents" (work left running in the background) is dropped, so a finished turn is
 * finished. `working` puts claude/working.txt's spinner right above the box, as Claude draws it, in
 * place of the last reply.
 */
export function claudeScreen(draft: string, working = false) {
  // a draft of several lines continues under the first, indented as Claude draws it
  const box = draft.split('\n').map((l, i) => (i ? `  ${l}` : `❯ ${l}`))
  const rows = IDLE.flatMap((l) => (l.startsWith('❯ ') ? box : [l.replace(' · ← 3 agents', '')]))
  // working, the last turn's reply isn't drawn right above the spinner: Claude shows your new prompt
  // between them, and a reply over a spinner is one being written now
  return working ? ['', SPINNER, ...rows.slice(3)] : rows
}

export interface ClaudeOptions {
  id: string
  sessionId: string
  cwd: string
  pid: number
  /** what's in the transcript before the scenario starts */
  history: unknown[]
  /** called when a message is submitted (Enter on the box) */
  onSubmit?: TuiOptions['onSubmit']
  onKey?: TuiOptions['onKey']
  /** more named screens: prompts and menus */
  screens?: Record<string, (draft: string) => string[]>
  session?: { id: string; name: string }
  window?: { id: string; index: number; name: string }
}

export class Claude {
  readonly tui: Tui
  readonly file: string
  private n = 100

  constructor(
    readonly m: Machine,
    readonly o: ClaudeOptions,
  ) {
    this.tui = new Tui({
      view: 'idle',
      screens: { idle: (d) => claudeScreen(d), working: (d) => claudeScreen(d, true), ...(o.screens ?? {}) },
      onSubmit: o.onSubmit,
      onKey: o.onKey,
    })
    this.file = `.claude/projects/${claudeSlug(o.cwd)}/${o.sessionId}.jsonl`
    m.file(this.file, jsonl(o.history))
    m.addPane({
      id: o.id,
      session: o.session ?? { id: '$1', name: 'work' },
      window: { ...(o.window ?? { id: '@1', index: 0, name: 'claude' }), activity: 1790000000 },
      cwd: o.cwd,
      title: '✳ Fix the flaky test',
      width: 100,
      height: 30,
      shell: { pid: o.pid - 1, comm: 'zsh' },
      agent: { pid: o.pid, comm: 'claude', args: 'claude' },
      tui: this.tui,
    })
  }

  record(type: string, body: Record<string, unknown>, sec: number) {
    const id = uuid(this.n++)
    const r = { parentUuid: null, isSidechain: false, userType: 'external', cwd: this.o.cwd, sessionId: this.o.sessionId, version: '2.1.0', gitBranch: 'main', type, uuid: id, timestamp: at(sec), ...body }
    this.m.append(this.file, `${JSON.stringify(r)}\n`)
    return id
  }

  /** Claude took a message: it's in the record and Claude is working. */
  took(text: string, sec: number) {
    this.record('user', { message: { role: 'user', content: text } }, sec)
    this.tui.show('working')
  }

  /** Claude replied and the turn is over. */
  replied(text: string, sec: number) {
    this.record('assistant', { message: { id: `msg_${this.n}`, type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'text', text }], stop_reason: 'end_turn', usage: { input_tokens: 1200, output_tokens: 40, cache_read_input_tokens: 30000, cache_creation_input_tokens: 0 } } }, sec)
    this.record('system', { subtype: 'turn_duration', durationMs: 4000 }, sec)
    this.tui.show('idle')
  }
}

/** A short conversation already in a Claude transcript. */
export function claudeHistory(cwd: string, sessionId: string) {
  const base = { isSidechain: false, userType: 'external', cwd, sessionId, version: '2.1.0', gitBranch: 'main' }
  return [
    { ...base, parentUuid: null, type: 'user', uuid: uuid(1), timestamp: at(0), message: { role: 'user', content: 'The parser test fails on wrapped lines. Can you look?' } },
    { ...base, parentUuid: uuid(1), type: 'assistant', uuid: uuid(2), timestamp: at(4), message: { id: 'msg_1', type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'text', text: 'I will run the test first.' }], stop_reason: 'tool_use', usage: { input_tokens: 900, output_tokens: 12, cache_read_input_tokens: 20000, cache_creation_input_tokens: 0 } } },
    { ...base, parentUuid: uuid(2), type: 'assistant', uuid: uuid(3), timestamp: at(5), message: { id: 'msg_1', type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'tool_use', id: 'toolu_1', name: 'Bash', input: { command: 'bun test screen', description: 'Run the screen tests' } }], stop_reason: 'tool_use', usage: { input_tokens: 900, output_tokens: 30, cache_read_input_tokens: 20000, cache_creation_input_tokens: 0 } } },
    { ...base, parentUuid: uuid(3), type: 'user', uuid: uuid(4), timestamp: at(9), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'toolu_1', content: '1 fail, 41 pass', is_error: false }] }, toolUseResult: { stdout: '1 fail, 41 pass', stderr: '', interrupted: false } },
    { ...base, parentUuid: uuid(4), type: 'assistant', uuid: uuid(5), timestamp: at(12), message: { id: 'msg_2', type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'text', text: 'One test fails: the wrapped permission prompt. The parser joins the rows too early.' }], stop_reason: 'end_turn', usage: { input_tokens: 1000, output_tokens: 25, cache_read_input_tokens: 21000, cache_creation_input_tokens: 0 } } },
    { ...base, parentUuid: uuid(5), type: 'system', subtype: 'turn_duration', durationMs: 12000, uuid: uuid(6), timestamp: at(12) },
  ]
}

// ---------- Codex ----------

const CODEX = screen('codex', 'finished-after-spinner')

/**
 * Codex's screen from codex/finished-after-spinner.txt: its composer ("›") holds `draft`; working
 * drops the "• done" line, so the spinner above it is the last word.
 */
export function codexScreen(draft: string, working = false) {
  const rows = CODEX.map((l) => (l === '›' ? (draft ? `› ${draft}` : '›') : l))
  return working ? rows.filter((l) => l !== '• done') : rows
}

export class Codex {
  readonly tui: Tui
  readonly file: string

  constructor(
    readonly m: Machine,
    readonly o: { id: string; threadId: string; cwd: string; pid: number; session?: { id: string; name: string }; window?: { id: string; index: number; name: string } },
  ) {
    this.tui = new Tui({
      view: 'idle',
      screens: { idle: (d) => codexScreen(d), working: (d) => codexScreen(d, true) },
      onSubmit: (text) => this.took(text),
    })
    this.file = `.codex/sessions/2026/01/15/rollout-2026-01-15T09-00-00-${o.threadId}.jsonl`
    const meta = { id: o.threadId, timestamp: at(0), cwd: o.cwd, originator: 'codex_cli_rs', cli_version: '0.170.0', source: 'cli', model_provider: 'openai' }
    m.file(this.file, jsonl([
      { timestamp: at(0), type: 'session_meta', payload: meta },
      { timestamp: at(1), type: 'turn_context', payload: { cwd: o.cwd, approval_policy: 'on-request', sandbox_policy: { type: 'workspace-write' }, model: 'gpt-6.1-sol', effort: 'medium', summary: 'auto' } },
      { timestamp: at(1), type: 'event_msg', payload: { type: 'task_started', turn_id: 'turn-1' } },
      { timestamp: at(1), type: 'response_item', payload: { type: 'message', role: 'user', content: [{ type: 'input_text', text: 'Add a subtract function to calc.py' }] } },
      { timestamp: at(1), type: 'event_msg', payload: { type: 'user_message', message: 'Add a subtract function to calc.py', images: [] } },
      { timestamp: at(6), type: 'response_item', payload: { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Added subtract(a, b) to calc.py.' }] } },
      { timestamp: at(6), type: 'event_msg', payload: { type: 'agent_message', message: 'Added subtract(a, b) to calc.py.' } },
      { timestamp: at(6), type: 'event_msg', payload: { type: 'token_count', info: { total_token_usage: { input_tokens: 9000, cached_input_tokens: 4000, output_tokens: 300, reasoning_output_tokens: 100, total_tokens: 9300 }, last_token_usage: { input_tokens: 9000, cached_input_tokens: 4000, output_tokens: 300, reasoning_output_tokens: 100, total_tokens: 9300 }, model_context_window: 272000 }, rate_limits: { primary: { used_percent: 12, window_minutes: 300, resets_in_seconds: 3600 }, secondary: { used_percent: 30, window_minutes: 10080, resets_in_seconds: 86400 } } } },
      { timestamp: at(6), type: 'event_msg', payload: { type: 'task_complete', turn_id: 'turn-1', last_agent_message: 'Added subtract(a, b) to calc.py.' } },
    ]))
    m.addPane({
      id: o.id,
      session: o.session ?? { id: '$2', name: 'codex' },
      window: { ...(o.window ?? { id: '@3', index: 0, name: 'codex' }), activity: 1790000000 },
      cwd: o.cwd,
      title: 'Add subtract | calc',
      width: 100,
      height: 30,
      shell: { pid: o.pid - 1, comm: 'zsh' },
      agent: { pid: o.pid, comm: 'codex', args: `codex resume ${o.threadId}` },
      tui: this.tui,
    })
  }

  private rec(sec: number, type: string, payload: unknown) {
    this.m.append(this.file, `${JSON.stringify({ timestamp: at(sec), type, payload })}\n`)
  }

  took(text: string, sec = 30) {
    this.rec(sec, 'event_msg', { type: 'task_started', turn_id: 'turn-2' })
    this.rec(sec, 'response_item', { type: 'message', role: 'user', content: [{ type: 'input_text', text }] })
    this.rec(sec, 'event_msg', { type: 'user_message', message: text, images: [] })
    this.tui.show('working')
  }

  replied(text: string, sec = 40) {
    this.rec(sec, 'response_item', { type: 'message', role: 'assistant', content: [{ type: 'output_text', text }] })
    this.rec(sec, 'event_msg', { type: 'agent_message', message: text })
    this.rec(sec, 'event_msg', { type: 'task_complete', turn_id: 'turn-2', last_agent_message: text })
    this.tui.show('idle')
  }
}

// ---------- OpenCode ----------

/**
 * An OpenCode TUI without --port: what it does lands in its SQLite database, which the hub reads
 * through sqlite3 (the stub reads this file with bun:sqlite). Layout as in crates/inyon-core's opencode_db tests.
 */
export function opencode(m: Machine, o: { id: string; cwd: string; pid: number; session?: { id: string; name: string } }) {
  const dir = path.join(m.home, '.local/share/opencode')
  fs.mkdirSync(dir, { recursive: true })
  const db = new Database(path.join(dir, 'opencode.db'))
  db.exec(`create table session (id text primary key, title text, directory text, parent_id text, time_created integer, time_updated integer, time_archived integer, agent text, model text, revert text, cost real, summary_additions integer, summary_deletions integer, summary_files integer, share_url text);
create table message (id text primary key, session_id text, time_created integer, time_updated integer, data text);
create table part (id text primary key, message_id text, session_id text, time_created integer, time_updated integer, data text)`)
  const S = 'ses_conformance000000000001'
  const t = Date.UTC(2026, 0, 15, 9, 0, 0)
  db.query('insert into session values (?, ?, ?, null, ?, ?, null, ?, ?, null, ?, ?, ?, ?, null)').run(S, 'Greeting', o.cwd, t, t + 6000, 'plan', 'opencode/deepseek-v4.1-flash-free', 0, 3, 1, 1)
  db.query('insert into message values (?, ?, ?, ?, ?)').run('msg_001', S, t, t, JSON.stringify({ role: 'user', time: { created: t }, agent: 'plan', model: { providerID: 'opencode', modelID: 'deepseek-v4.1-flash-free' } }))
  db.query('insert into message values (?, ?, ?, ?, ?)').run('msg_002', S, t + 1000, t + 6000, JSON.stringify({ role: 'assistant', time: { created: t + 1000, completed: t + 6000 }, modelID: 'deepseek-v4.1-flash-free', providerID: 'opencode', mode: 'plan', cost: 0, tokens: { input: 36000, output: 898, reasoning: 0, cache: { read: 0, write: 0 } } }))
  db.query('insert into part values (?, ?, ?, ?, ?, ?)').run('prt_001', 'msg_001', S, t, t, JSON.stringify({ type: 'text', text: 'Say hello in one word.' }))
  db.query('insert into part values (?, ?, ?, ?, ?, ?)').run('prt_002', 'msg_002', S, t + 1000, t + 6000, JSON.stringify({ type: 'text', text: 'Hello.' }))
  db.close()
  const tui = new Tui({ view: 'idle', screens: { idle: () => screen('opencode', 'idle-wide'), working: () => screen('opencode', 'working') } })
  m.addPane({
    id: o.id,
    session: o.session ?? { id: '$3', name: 'oc' },
    window: { id: '@4', index: 0, name: 'opencode', activity: 1790000000 },
    cwd: o.cwd,
    title: 'OC | Greeting',
    width: 100,
    height: 40,
    shell: { pid: o.pid - 1, comm: 'zsh' },
    agent: { pid: o.pid, comm: 'opencode', args: 'opencode' },
    tui,
  })
  return { tui, session: S }
}
