import type { Machine } from './machine.ts'

/**
 * A machine running inyon, as the hub sees it on /node: one WebSocket with the pairing token as a
 * bearer, a hello frame, then the hub's operations (see crates/inyon-core/src/hosts/node_link.rs and
 * crates/inyon-engine/src/ops.rs):
 *
 *   hub → node   { id, op: 'exec' | 'stream' | 'pty' | 'write' | 'resize' | 'kill', ... }
 *   node → hub   { id, type: 'exit' | 'line' | 'data' | 'error', ... }
 *
 * exec and stream run the hub's shell snippet with /bin/sh on the fake machine, as the real node
 * does. Two things are answered here instead: tmux control mode (`tmux -C attach-session`), which
 * reports a pane's output when the scenario changes its screen, and terminals (pty), which echo
 * what they're sent and record it.
 */

const CONTROL = /-C attach-session -t '([^']*)'/

type Proc = ReturnType<typeof Bun.spawn>

export class FakeNode {
  private ws?: WebSocket
  private procs = new Map<string, { proc?: Proc; kill: () => void; write?: (d: string) => void; resize?: (c: number, r: number) => void }>()
  /** open terminals: op id → pane */
  private ptys = new Map<string, string>()
  private everything = new Set<Proc>()
  /** commands answered so far, for the run summary */
  execs = 0
  closed?: Promise<{ code: number; reason: string }>

  constructor(
    readonly machine: Machine,
    readonly hello: Record<string, unknown>,
  ) {}

  /** Connects with `token`; resolves with the upgrade's outcome (open, or the HTTP refusal). */
  connect(hub: string, token: string): Promise<'open' | 'refused'> {
    const ws = new WebSocket(`${hub.replace(/^http/, 'ws')}/node`, { headers: { authorization: `Bearer ${token}` } } as any)
    this.ws = ws
    let closed!: (v: { code: number; reason: string }) => void
    this.closed = new Promise((r) => (closed = r))
    return new Promise((resolve) => {
      ws.onopen = () => {
        ws.send(JSON.stringify(this.hello))
        resolve('open')
      }
      ws.onerror = () => resolve('refused')
      ws.onclose = (e) => {
        closed({ code: e.code, reason: e.reason })
        this.stopAll()
      }
      ws.onmessage = (e) => this.message(JSON.parse(String(e.data)))
    })
  }

  close() {
    this.ws?.close()
    this.stopAll()
  }

  private send(m: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(m))
  }

  private spawn(cmd: string, stdin: 'ignore' | 'pipe' | Uint8Array) {
    const p = Bun.spawn(['/bin/sh', '-c', cmd], { cwd: this.machine.home, env: this.machine.env(), stdin, stdout: 'pipe', stderr: 'pipe', detached: true })
    this.everything.add(p)
    p.exited.then(() => this.everything.delete(p))
    return p
  }

  /** Ends a command and its process group (only processes this node started). */
  private killTree(p: Proc) {
    try {
      process.kill(-p.pid, 'SIGTERM')
    } catch {
      p.kill()
    }
  }

  private stopAll() {
    for (const p of this.procs.values()) p.kill()
    this.procs.clear()
    for (const p of this.everything) this.killTree(p)
  }

  private async message(m: any) {
    const id = String(m.id)
    switch (m.op) {
      case 'exec': {
        this.execs++
        const p = this.spawn(m.cmd, m.input ? Buffer.from(m.input, 'base64') : 'ignore')
        this.procs.set(id, { proc: p, kill: () => this.killTree(p) })
        const out = Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text()])
        const code = await p.exited
        // as the real node: output still held open by an escaped child gets a second, then the answer goes
        const [stdout, stderr] = await Promise.race([out, Bun.sleep(1000).then(() => ['', 'output held open'])])
        this.procs.delete(id)
        return this.send({ id, type: 'exit', code, stdout, stderr })
      }
      case 'stream': {
        const control = CONTROL.exec(m.cmd)
        if (control) return this.control(id, control[1]!)
        const p = this.spawn(m.cmd, m.keepStdin ? 'pipe' : 'ignore')
        this.procs.set(id, { proc: p, kill: () => this.killTree(p) })
        void (async () => {
          const dec = new TextDecoder()
          let carry = ''
          for await (const chunk of p.stdout as ReadableStream<Uint8Array>) {
            const lines = (carry + dec.decode(chunk, { stream: true })).split('\n')
            carry = lines.pop() ?? ''
            for (const l of lines) if (l) this.send({ id, type: 'line', data: l })
          }
          if (carry) this.send({ id, type: 'line', data: carry })
          this.send({ id, type: 'exit', code: await p.exited })
          this.procs.delete(id)
        })().catch(() => {})
        return
      }
      case 'pty':
        return this.pty(id, m)
      case 'write':
        return this.procs.get(id)?.write?.(m.data)
      case 'resize':
        return this.procs.get(id)?.resize?.(m.cols, m.rows)
      case 'kill': {
        const p = this.procs.get(id)
        this.procs.delete(id)
        return p?.kill()
      }
    }
  }

  /** tmux control mode on one session: a line of %output whenever one of its panes redraws. */
  private control(id: string, session: string) {
    const off = this.machine.onOutput((pane) => {
      const p = this.machine.panes.get(pane)
      if (p && (p.o.session.id === session || p.o.session.name === session)) this.send({ id, type: 'line', data: `%output ${pane} x` })
    })
    this.procs.set(id, { kill: off })
  }

  /** The terminals on `pane` end on the machine's side (the pane closed, or someone killed the view). */
  endTerminals(pane: string) {
    for (const [id, p] of this.ptys) {
      if (p !== pane) continue
      this.ptys.delete(id)
      this.procs.delete(id)
      this.send({ id, type: 'exit', code: 0 })
    }
  }

  /** A terminal on a pane: it shows the pane's screen, echoes what it's sent, and records it all. */
  private pty(id: string, m: { cmd: string; cols: number; rows: number }) {
    const pane = /select-pane -t '([^']+)'/.exec(m.cmd)?.[1] ?? '?'
    const stream = `pty ${pane}`
    this.machine.record(stream, { pty: 'open', cols: m.cols, rows: m.rows, cmd: m.cmd })
    const data = (s: string) => this.send({ id, type: 'data', data: Buffer.from(s).toString('base64') })
    const screen = this.machine.panes.get(pane)?.screen() ?? []
    data(`\x1b[H\x1b[2J${screen.join('\r\n')}`)
    this.ptys.set(id, pane)
    this.procs.set(id, {
      kill: () => (this.ptys.delete(id), this.machine.record(stream, { pty: 'kill' })),
      write: (d) => {
        this.machine.record(stream, { pty: 'write', data: d })
        data(d)
      },
      resize: (cols, rows) => this.machine.record(stream, { pty: 'resize', cols, rows }),
    })
  }
}
