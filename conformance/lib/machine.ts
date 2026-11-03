import fs from 'node:fs'
import path from 'node:path'

/**
 * The machine behind the fake node: a throwaway home folder where the hub's shell commands really
 * run (/bin/sh, with the agents' records as files), and a tmux that exists only here. Every `tmux`,
 * `ps` and `pgrep` the hub's commands call is a stub that asks this object, so panes, screens and
 * keystrokes are under the scenario's control and no real tmux is touched.
 */

/** What the hub typed into a pane: literal text (send-keys -l), a named key, or a bracketed paste. */
export type Key = { text: string } | { key: string } | { paste: string }

export interface TuiOptions {
  /** named screens; each gets the input box's current text */
  screens: Record<string, (draft: string) => string[]>
  view: string
  /** a key the scenario handles itself; return true to skip the default (editing the box) */
  onKey?: (k: Key, tui: Tui) => boolean | void
  /** Enter on the input box with this text in it (the box is emptied first) */
  onSubmit?: (text: string, tui: Tui) => void
}

/** An agent's terminal UI, as far as the hub can see and type into it. */
export class Tui {
  draft = ''
  view: string
  pane?: Pane

  constructor(private o: TuiOptions) {
    this.view = o.view
  }

  screen() {
    const make = this.o.screens[this.view]
    if (!make) throw new Error(`no screen named ${this.view}`)
    return make(this.draft)
  }

  /** Shows another screen (as the agent redrawing), and tells tmux's control clients. */
  show(view: string) {
    this.view = view
    this.pane?.changed()
  }

  key(k: Key) {
    if (this.o.onKey?.(k, this)) return this.pane?.changed()
    if ('text' in k) this.draft += k.text
    else if ('paste' in k) this.draft += k.paste
    else if (k.key === 'C-u') this.draft = this.draft.includes('\n') ? this.draft.slice(0, this.draft.lastIndexOf('\n') + 1) : ''
    else if (k.key === 'BSpace') this.draft = this.draft.slice(0, -1)
    else if (k.key === 'C-j') this.draft += '\n'
    else if (k.key === 'Enter' && this.draft.trim()) {
      const text = this.draft
      this.draft = ''
      this.o.onSubmit?.(text, this)
    }
    this.pane?.changed()
  }
}

export interface PaneOptions {
  id: string
  session: { id: string; name: string }
  window: { id: string; index: number; name: string; activity: number; active?: boolean }
  index?: number
  active?: boolean
  cwd: string
  title?: string
  width?: number
  height?: number
  /** the pane's own process (its shell) */
  shell?: { pid: number; comm: string }
  /** a program started from that shell: the agent */
  agent?: { pid: number; comm: string; args: string; etimes?: number }
  tui?: Tui
}

export class Pane {
  constructor(
    readonly o: PaneOptions,
    private machine: Machine,
  ) {
    if (o.tui) o.tui.pane = this
  }

  get id() {
    return this.o.id
  }

  screen(): string[] {
    return this.o.tui?.screen() ?? ['$ ']
  }

  changed() {
    this.machine.output(this.id)
  }

  /** tmux's format variables for this pane */
  vars(): Record<string, string> {
    const o = this.o
    const shell = o.shell ?? { pid: 0, comm: 'zsh' }
    return {
      session_id: o.session.id,
      session_name: o.session.name,
      session_attached: '0',
      window_id: o.window.id,
      window_index: String(o.window.index),
      window_name: o.window.name,
      window_active: o.window.active === false ? '0' : '1',
      window_activity: String(o.window.activity),
      window_width: String(o.width ?? 100),
      window_height: String(o.height ?? 30),
      pane_id: o.id,
      pane_index: String(o.index ?? 0),
      pane_active: o.active === false ? '0' : '1',
      pane_pid: String(shell.pid),
      pane_current_command: o.agent?.comm ?? shell.comm,
      pane_current_path: o.cwd,
      pane_title: o.title ?? '',
      pane_width: String(o.width ?? 100),
      pane_height: String(o.height ?? 30),
      pane_dead: '0',
      pane_in_mode: '0',
      mouse_any_flag: '0',
      cursor_x: '2',
      cursor_y: '0',
      cursor_flag: '1',
      client_control_mode: '0',
      socket_path: '/tmp/tmux-conformance/default',
    }
  }
}

/** Something the hub did to the machine (not a read), for the transcript. */
export interface Action {
  /** which pane or terminal it concerns, for grouping */
  stream: string
  did: Record<string, unknown>
}

/** tmux commands that only read: the hub polls these all the time, so they aren't recorded */
const READS = new Set(['list-panes', 'list-clients', 'list-sessions', 'list-windows', 'capture-pane', 'display-message', 'display', 'show-options', 'show', 'show-window-options', 'showw', 'has-session', 'list-buffers', 'list-keys', '-V'])

/** `opencode models --verbose` and `opencode agent list`, in the shapes OpenCode prints them */
const OPENCODE = `case "$1 $2" in
  "models --verbose") cat <<'JSON'
opencode/deepseek-v4.1-flash-free
{
  "id": "deepseek-v4.1-flash-free",
  "providerID": "opencode",
  "name": "DeepSeek V4.1 Flash Free",
  "limit": { "context": 128000, "output": 8000 },
  "status": "active",
  "variants": { "high": {}, "low": {} }
}
opencode/big-pickle
{
  "id": "big-pickle",
  "providerID": "opencode",
  "name": "Big Pickle",
  "limit": { "context": 200000, "output": 32000 },
  "status": "active",
  "variants": {}
}
JSON
  ;;
  "agent list") printf 'build (primary)\\n  []\\nplan (primary)\\n  []\\ntitle (primary)\\n  []\\n' ;;
  *) echo "opencode (conformance stub)" ;;
esac`

/** tmux's flags that take a value, so the parser knows what follows them */
const VALUE_FLAGS = new Set(['-t', '-F', '-b', '-s', '-n', '-c', '-x', '-y', '-S', '-E', '-T', '-f', '-e', '-N'])

export class Machine {
  readonly home: string
  readonly bin: string
  readonly panes = new Map<string, Pane>()
  /** processes besides the panes' own, for ps */
  readonly procs: { pid: number; ppid: number; etimes: number; comm: string; args: string }[] = []
  readonly actions: Action[] = []
  /** when the hub last did something here (an action), for settling */
  activityAt = 0
  private outputs = new Set<(pane: string) => void>()
  private ctl?: ReturnType<typeof Bun.serve>

  constructor(readonly root: string) {
    this.home = path.join(root, 'home')
    this.bin = path.join(this.home, '.local', 'bin')
  }

  /** The machine's environment for the commands the hub sends it. */
  env(): Record<string, string> {
    return {
      HOME: this.home,
      PATH: `${this.bin}:/usr/bin:/bin`,
      LANG: 'C.UTF-8',
      LC_ALL: 'C.UTF-8',
      TZ: 'UTC',
      USER: 'dev',
      SHELL: '/bin/sh',
      CONFORMANCE_CTL: `http://127.0.0.1:${this.ctl!.port}`,
    }
  }

  /** Makes the home folder, the stubs and the line the probe would otherwise work out from /sys. */
  async start() {
    fs.mkdirSync(this.bin, { recursive: true })
    fs.mkdirSync(path.join(this.home, '.inyon'), { recursive: true })
    // what MACHINE_KIND_SH (crates/inyon-core/src/tmux/machine_kind.rs) caches: a server, read once a day
    fs.writeFileSync(path.join(this.home, '.inyon', 'machine-kind'), 'Linux|1||kvm|0|0|Conformance VM|\n')
    const stub = path.join(import.meta.dir, 'stub.ts')
    const write = (name: string, body: string) => fs.writeFileSync(path.join(this.bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 })
    for (const name of ['tmux', 'ps', 'pgrep', 'sqlite3']) write(name, `exec '${process.execPath}' '${stub}' ${name} "$@"`)
    write('hostname', 'echo box')
    write('uname', 'case "$1" in -m) echo x86_64;; *) echo Linux;; esac')
    // no network from the machine: Claude's usage check and OpenCode's server calls fail at once
    write('curl', 'echo "curl: (7) Failed to connect (conformance machine has no network)" >&2; exit 7')
    // agent programs exist (command -v finds them) but are never started for real
    for (const name of ['claude', 'codex']) write(name, `echo "${name} (conformance stub)"; exit 0`)
    // OpenCode answers the two listings the hub asks it for (crates/inyon-core/src/agents/opencode_models.rs)
    write('opencode', OPENCODE)
    // the probe's Copilot sign-in check starts node; there is no Copilot here
    write('node', 'exit 1')
    this.ctl = Bun.serve({ port: 0, hostname: '127.0.0.1', fetch: (req) => this.serve(req) })
  }

  stop() {
    this.ctl?.stop(true)
  }

  private async serve(req: Request) {
    const { name, args, input } = (await req.json()) as { name: string; args: string[]; input?: string }
    try {
      if (name === 'tmux') return Response.json(this.tmux(args, input))
      if (name === 'ps') return Response.json({ code: 0, stdout: this.ps() })
      // pgrep: no agent process can be looked up in /proc here; the panes' folders stand in
      if (name === 'pgrep') return Response.json({ code: 1 })
      return Response.json({ code: 127, stderr: `${name}: not on the conformance machine\n` })
    } catch (e) {
      return Response.json({ code: 1, stderr: `${(e as Error).message}\n` })
    }
  }

  addPane(o: PaneOptions) {
    const p = new Pane(o, this)
    this.panes.set(o.id, p)
    return p
  }

  /** Writes a file in the machine's home (relative path), optionally dated `mtime` (ms). */
  file(rel: string, content: string, mtime?: number) {
    const f = path.join(this.home, rel)
    fs.mkdirSync(path.dirname(f), { recursive: true })
    fs.writeFileSync(f, content)
    if (mtime) fs.utimesSync(f, mtime / 1000, mtime / 1000)
    return f
  }

  append(rel: string, content: string) {
    fs.appendFileSync(path.join(this.home, rel), content)
  }

  onOutput(fn: (pane: string) => void) {
    this.outputs.add(fn)
    return () => void this.outputs.delete(fn)
  }

  output(pane: string) {
    for (const fn of this.outputs) fn(pane)
  }

  record(stream: string, did: Record<string, unknown>) {
    this.actions.push({ stream, did })
    this.activityAt = Date.now()
  }

  ps() {
    const rows: string[] = []
    for (const p of this.panes.values()) {
      const shell = p.o.shell
      if (!shell) continue
      rows.push(`${shell.pid} 1 3600 ${shell.comm} -${shell.comm}`)
      const a = p.o.agent
      if (a) rows.push(`${a.pid} ${shell.pid} ${a.etimes ?? 600} ${a.comm} ${a.args}`)
    }
    for (const x of this.procs) rows.push(`${x.pid} ${x.ppid} ${x.etimes} ${x.comm} ${x.args}`)
    return rows.map((r) => `${r}\n`).join('')
  }

  private target(t: string | undefined): Pane | undefined {
    if (!t) return [...this.panes.values()][0]
    const p = this.panes.get(t)
    if (p) return p
    // a session or window: its first pane
    const bare = t.replace(/:$/, '')
    return [...this.panes.values()].find((x) => x.o.session.id === bare || x.o.session.name === bare || x.o.window.id === bare)
  }

  /** One `tmux …` call from a hub command. Several commands joined with `\;` run in turn. */
  tmux(argv: string[], input?: string): { code: number; stdout?: string; stderr?: string } {
    const args = argv[0] === '-u' ? argv.slice(1) : argv
    let stdout = ''
    for (const cmd of split(args)) {
      const r = this.command(cmd, input)
      if (r.code !== 0) return { ...r, stdout: stdout + (r.stdout ?? '') }
      stdout += r.stdout ?? ''
    }
    return { code: 0, stdout }
  }

  private command(args: string[], input?: string): { code: number; stdout?: string; stderr?: string } {
    const [sub = '', ...rest] = args
    const flags: Record<string, string | true> = {}
    const words: string[] = []
    for (let i = 0; i < rest.length; i++) {
      const a = rest[i]!
      if (a === '--') {
        words.push(...rest.slice(i + 1))
        break
      }
      if (/^-[A-Za-z]+$/.test(a) && words.length === 0) {
        if (VALUE_FLAGS.has(a) && i + 1 < rest.length) flags[a] = rest[++i]!
        else for (const c of a.slice(1)) flags[`-${c}`] = true
      } else words.push(a)
    }
    const pane = this.target(typeof flags['-t'] === 'string' ? flags['-t'] : undefined)
    // a buffer loaded from a temp file: the file's name is random, what was in it is what matters
    // the Rust hub rests agent windows nobody shows at 200×50 and the Bun hub doesn't: sizing a
    // window isn't part of what the two are compared on
    const sizing = sub === 'resize-window' || (/^set(-option|w|-window-option)?$/.test(sub) && words.includes('window-size'))
    if (sub === 'load-buffer') this.record('buffers', { tmux: [...args.slice(0, -1), '<file>'], text: input ?? '' })
    else if (!READS.has(sub) && !sizing) this.record(typeof flags['-t'] === 'string' ? (pane?.id ?? flags['-t']) : 'tmux', { tmux: args })
    switch (sub) {
      case 'list-panes': {
        const fmt = typeof flags['-F'] === 'string' ? flags['-F'] : '#{pane_id}'
        if (!this.panes.size) return { code: 1, stderr: 'no server running on /tmp/tmux-conformance/default\n' }
        return { code: 0, stdout: [...this.panes.values()].map((p) => `${format(fmt, p.vars())}\n`).join('') }
      }
      case 'list-clients':
      case 'list-buffers':
        return { code: 0, stdout: '' }
      case 'list-sessions': {
        const fmt = typeof flags['-F'] === 'string' ? flags['-F'] : '#{session_name}'
        const seen = new Set<string>()
        let out = ''
        for (const p of this.panes.values()) if (!seen.has(p.o.session.id)) seen.add(p.o.session.id), (out += `${format(fmt, p.vars())}\n`)
        return { code: 0, stdout: out }
      }
      case 'has-session':
        return pane ? { code: 0 } : { code: 1, stderr: "can't find session\n" }
      case 'capture-pane':
        if (!pane) return { code: 1, stderr: "can't find pane\n" }
        return { code: 0, stdout: `${pane.screen().join('\n')}\n` }
      case 'display':
      case 'display-message':
        if (!pane) return { code: 1, stderr: "can't find pane\n" }
        return { code: 0, stdout: flags['-p'] ? `${format(words.join(' '), pane.vars())}\n` : '' }
      case 'show':
      case 'show-options':
      case 'show-window-options':
      case 'showw':
        return { code: 0, stdout: '' }
      case 'send-keys': {
        if (!pane) return { code: 1, stderr: "can't find pane\n" }
        const keys = flags['-l'] ? [{ text: words.join(' ') }] : words.map((key) => ({ key }))
        for (const k of keys) pane.o.tui?.key(k)
        return { code: 0 }
      }
      case 'load-buffer':
        this.buffers.set(String(flags['-b'] ?? 'buffer0'), input ?? '')
        return { code: 0 }
      case 'paste-buffer': {
        const name = String(flags['-b'] ?? 'buffer0')
        const text = this.buffers.get(name)
        if (text === undefined || !pane) return { code: 1, stderr: 'no buffer\n' }
        if (flags['-d']) this.buffers.delete(name)
        pane.o.tui?.key({ paste: text })
        return { code: 0 }
      }
      case 'delete-buffer':
        this.buffers.delete(String(flags['-b'] ?? ''))
        return { code: 0 }
      // changes the hub makes that this machine only notes (they're in the transcript)
      case 'set-option':
      case 'set':
      case 'set-window-option':
      case 'setw':
      case 'resize-window':
      case 'resize-pane':
      case 'select-pane':
      case 'select-window':
      case 'rename-window':
      case 'bind-key':
      case 'refresh-client':
        return { code: 0 }
      default:
        return { code: 1, stderr: `conformance tmux: ${sub} is not modelled\n` }
    }
  }

  private buffers = new Map<string, string>()
}

/** argv split at `;` / `\;` into tmux commands. */
function split(args: string[]): string[][] {
  const out: string[][] = [[]]
  for (const a of args) {
    if (a === ';' || a === '\\;') out.push([])
    else out.at(-1)!.push(a)
  }
  return out.filter((c) => c.length)
}

/** tmux's #{name} substitution, plus the #{||:a,b} the hub's view bindings use. */
export function format(fmt: string, vars: Record<string, string>) {
  return fmt.replace(/#\{([a-z_]+)\}/g, (_, k: string) => vars[k] ?? '')
}
