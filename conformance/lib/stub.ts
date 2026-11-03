// One program behind every stub on the fake machine's PATH (tmux, ps, pgrep, sqlite3): the
// wrapper script passes its own name first. tmux, ps and pgrep ask the harness, which holds the
// machine's state; sqlite3 reads the fake machine's database itself.
import { Database } from 'bun:sqlite'
import fs from 'node:fs'

const [name = '', ...args] = process.argv.slice(2)

if (name === 'sqlite3') {
  // sqlite3 -readonly -noheader -list <db> <query>, the way crates/inyon-core/src/agents/opencode_db.rs asks
  const rest = args.filter((a) => !a.startsWith('-'))
  const [file, query] = rest
  try {
    const db = new Database(file!, { readonly: true })
    const rows = db.query(query!).values() as unknown[][]
    process.stdout.write(rows.map((r) => r.map((v) => (v === null ? '' : String(v))).join('|')).join('\n') + (rows.length ? '\n' : ''))
    process.exit(0)
  } catch (e) {
    process.stderr.write(`Error: ${(e as Error).message}\n`)
    process.exit(1)
  }
}

// a buffer loaded from a file travels with the call, since the harness can't read the caller's temp file later
let input: string | undefined
const lb = args.indexOf('load-buffer')
if (name === 'tmux' && lb >= 0) {
  const file = args.at(-1)!
  input = file === '-' ? await new Response(Bun.stdin.stream()).text() : fs.readFileSync(file, 'utf8')
}

const res = await fetch(`${process.env.CONFORMANCE_CTL}/stub`, {
  method: 'POST',
  body: JSON.stringify({ name, args, input }),
}).catch(() => undefined)
if (!res?.ok) {
  process.stderr.write(`conformance stub ${name}: the harness did not answer\n`)
  process.exit(70)
}
const out = (await res.json()) as { code: number; stdout?: string; stderr?: string }
if (out.stdout) process.stdout.write(out.stdout)
if (out.stderr) process.stderr.write(out.stderr)
process.exit(out.code)
