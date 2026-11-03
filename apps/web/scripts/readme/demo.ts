// Made-up machines, sessions and conversations for the README's pictures. Nothing here comes from
// a real account: the person (sam), the machines, projects, paths and numbers are all invented.
import type { ChatInfo, ChatItem, HostState, ScreenState } from '@inyon/protocol'

const now = Date.now()
const s = Math.floor(now / 1000)
const ts = (agoSec: number) => new Date(now - agoSec * 1000).toISOString()
const limits = { fiveHour: { pct: 22, resetsAt: s + 2 * 3600 + 900 }, sevenDay: { pct: 41, resetsAt: s + 3 * 86400 }, plan: 'Max 5x' }

const claude = (model: string, modelId: string, effort: string, ctx: number, cost: number, mins: number) => ({
  model, modelId, effort, fast: false, version: '2.1.296', contextPct: Math.round((ctx / 1_000_000) * 100), contextUsed: ctx, contextSize: 1_000_000,
  costUsd: cost, durationMs: mins * 60000, apiDurationMs: mins * 20000, linesAdded: 214, linesRemoved: 61, outputStyle: 'default', updatedAt: now - 4000, ...limits,
})

let n = 10
const pane = (title: string, cwd: string, command: string, agent?: Record<string, unknown>, index = 0) => {
  n++
  return {
    id: '%' + n, index, active: index === 0, command, cwd, title, width: 120, height: 40, dead: false,
    ...(agent ? { agent: { pid: 4000 + n, startedAt: now - 3600_000, mappedBy: 'hook', ...agent } } : {}),
  }
}
const win = (name: string, index: number, ago: number, panes: any[]) => ({ id: '@' + n, index, name, active: index === 1, activity: s - ago, panes })
const session = (name: string, windows: any[]) => ({ id: '$' + n, name, attached: 0, windows })
const host = (id: string, name: string, kind: string, os: string, machineKind: string, sessions: any[], extra: Record<string, unknown> = {}): HostState =>
  ({
    config: { id, name, kind, ...(kind === 'node' ? { pairedAt: now - 20 * 86400_000 } : kind === 'ssh' ? { target: `sam@${name}` } : {}) },
    status: 'online', hostname: name, integration: 'installed', agyIntegration: 'installed', tmux: 'ok',
    agents: ['claude', 'codex', 'opencode', 'gemini', 'copilot'], signedIn: ['claude', 'codex', 'opencode', 'gemini', 'copilot'],
    os, machineKind, latencyMs: id === 'local' ? 4 : 38, nodeVersion: kind === 'node' ? '0.10.0' : undefined, sessions, updatedAt: now, lastSeen: now, ...extra,
  }) as any

export const PANES = {
  checkout: '%11',
  devServer: '%12',
  passkeys: '%13',
  notes: '%14',
  billing: '%15',
  ci: '%16',
  landing: '%17',
} as const

/** The question Codex asks in the passkeys session. */
export const CODEX_OPTIONS = [
  { n: 1, label: 'Yes, run it' },
  { n: 2, label: 'Always for bun test' },
  { n: 3, label: 'No' },
]

export function hosts(): HostState[] {
  n = 10
  return [
    host('local', 'studio', 'local', 'Linux', 'desktop', [
      session('storefront', [
        win('claude', 1, 3, [
          pane('✳ Fix flaky checkout test', '/home/sam/code/storefront', 'claude', {
            kind: 'claude', state: 'working', mode: 'accept edits', activity: 'Running the checkout suite… (2m 14s · ↓ 3.1k tokens)',
            status: claude('Sonnet 5.5', 'claude-sonnet-5-5', 'medium', 61200, 3.42, 38),
          }),
        ]),
        win('dev', 2, 30, [pane('bun dev', '/home/sam/code/storefront', 'bun')]),
      ]),
      session('accounts', [
        win('codex', 1, 40, [
          pane('Migrate auth to passkeys', '/home/sam/code/accounts', 'node', {
            kind: 'codex', state: 'waiting', mode: 'auto', waitingFor: 'Run bun test src/auth?', options: CODEX_OPTIONS,
            status: { model: 'GPT-6-Astra', modelId: 'gpt-6-astra', effort: 'medium', plan: 'plus', contextUsed: 48300, contextSize: 258400, contextPct: 18.7, sevenDay: { pct: 27, resetsAt: s + 5 * 86400 }, updatedAt: now - 9000 },
          }),
        ]),
      ]),
    ]),
    host('build-box', 'build-box', 'node', 'Linux', 'server', [
      session('docs', [
        win('claude', 1, 260, [
          pane('✳ Write release notes', '/home/sam/code/docs-site', 'claude', {
            kind: 'claude', state: 'idle', mode: 'default', status: claude('Opus 5.5', 'claude-opus-5-5', 'high', 92400, 1.86, 21),
          }),
        ]),
      ]),
      session('billing', [
        win('opencode', 1, 900, [
          pane('Tidy the billing API types', '/home/sam/code/billing', 'opencode', {
            kind: 'opencode', state: 'idle', mode: 'build', background: '1 shell',
            status: { model: 'Qwen3 Coder', modelId: 'qwen3-coder', contextUsed: 30500, contextSize: 262144, contextPct: 11.6, costUsd: 0.74, updatedAt: now - 60000 },
          }),
        ]),
        win('copilot', 2, 120, [
          pane('Speed up the CI cache', '/home/sam/code/billing', 'copilot', {
            kind: 'copilot', state: 'working', mode: 'default', activity: 'Reading .github/workflows/ci.yml',
            status: { model: 'Claude Sonnet 5.5', modelId: 'claude-sonnet-5.5', contextUsed: 21800, contextSize: 200000, contextPct: 10.9, updatedAt: now - 5000 },
          }),
        ]),
      ]),
    ]),
    host('air', 'air', 'node', 'macOS', 'laptop', [
      session('site', [
        win('gemini', 1, 1500, [
          pane('Landing page copy pass', '/Users/sam/code/site', 'gemini', {
            kind: 'gemini', state: 'idle', mode: 'default',
            status: { model: 'Gemini 3 Pro', modelId: 'gemini-3-pro', contextUsed: 41200, contextSize: 1048576, contextPct: 3.9, updatedAt: now - 120000 },
          }),
        ]),
      ]),
    ]),
    { ...host('nas', 'nas', 'ssh', 'Linux', 'server', []), status: 'error', error: 'timed out', lastSeen: now - 2 * 86400_000, offlineSince: now - 2 * 86400_000 } as HostState,
  ]
}

// ---------- conversations ----------

let seq = 0
const id = () => `u${++seq}`
const user = (text: string, ago: number): ChatItem => ({ kind: 'user', seq: seq + 1, uuid: id(), ts: ts(ago), text, images: 0 })
const say = (text: string, ago: number, model = 'claude-sonnet-5-5'): ChatItem => ({ kind: 'assistant', seq: seq + 1, uuid: id(), ts: ts(ago), msgId: 'm' + seq, model, block: { type: 'text', text } })
const tool = (toolId: string, name: string, input: any, ago: number): ChatItem => ({ kind: 'assistant', seq: seq + 1, uuid: id(), ts: ts(ago), msgId: 'm' + seq, block: { type: 'tool_use', id: toolId, name, input } })
const result = (toolId: string, text: string, ago: number, meta?: any): ChatItem => ({ kind: 'tool_result', seq: seq + 1, uuid: id(), ts: ts(ago), toolUseId: toolId, isError: false, text, meta })
const end = (ago: number, ms: number): ChatItem => ({ kind: 'turn_end', seq: seq + 1, uuid: id(), ts: ts(ago), durationMs: ms })

function checkoutChat(): ChatItem[] {
  return [
    user('The checkout test fails about one run in five on CI. Find out why and fix it, then run it enough times to be sure.', 620),
    say("I'll run the checkout suite repeatedly to catch the failure first.", 612),
    tool('t1', 'Bash', { command: 'bun test src/checkout --rerun-each 20', description: 'Run the checkout tests 20 times' }, 610),
    result('t1', '', 560, { stdout: 'src/checkout/cart.test.ts:\n✓ adds an item [2.10ms]\n✗ applies the discount before tax [5.02ms]\n  expected 41.40, received 46.00\n\n 76 pass\n 4 fail' }),
    say('Four failures, all in **applies the discount before tax**. The test reads `cart.total` right after `applyCode()`, but the discount lands in a microtask, so the read sometimes wins.', 552),
    tool('t2', 'Read', { file_path: '/home/sam/code/storefront/src/checkout/cart.ts' }, 550),
    result('t2', 'export class Cart {\n  items: Item[] = []\n  …\n  applyCode(code: string) {\n    queueMicrotask(() => (this.discount = rate(code)))\n  }\n}', 548, { numLines: 86 }),
    tool('t3', 'Edit', { file_path: '/home/sam/code/storefront/src/checkout/cart.ts' }, 540),
    result('t3', 'The file was updated.', 538, {
      filePath: '/home/sam/code/storefront/src/checkout/cart.ts',
      patch: [{ oldStart: 41, oldLines: 4, newStart: 41, newLines: 5, lines: [
        '   applyCode(code: string) {',
        '-    queueMicrotask(() => (this.discount = rate(code)))',
        '+    // apply it now: a later read of total must already see it',
        '+    this.discount = rate(code)',
        '+    this.emit(\'change\')',
        '   }',
      ] }],
    }),
    tool('t4', 'TodoWrite', { todos: [
      { content: 'Reproduce the failure', status: 'completed', activeForm: 'Reproducing the failure' },
      { content: 'Apply the discount synchronously', status: 'completed', activeForm: 'Applying the discount synchronously' },
      { content: 'Run the suite 50 times', status: 'in_progress', activeForm: 'Running the suite 50 times' },
      { content: 'Add a regression test', status: 'pending', activeForm: 'Adding a regression test' },
    ] }, 530),
    tool('t5', 'Bash', { command: 'bun test src/checkout --rerun-each 50', description: 'Run the checkout tests 50 times' }, 528),
  ]
}

function passkeysChat(): ChatItem[] {
  return [
    user('Replace the password sign-in with passkeys. Keep the old flow behind a flag for a week.', 900),
    say('Plan:\n\n1. Add WebAuthn registration and sign-in routes.\n2. Store credentials in a new `passkeys` table.\n3. Keep `/login/password` behind `AUTH_PASSWORD_FALLBACK`.\n\nStarting with the routes.', 880, 'gpt-6-astra'),
    tool('c1', 'ApplyPatch', { patch: '*** Begin Patch\n*** Add File: src/auth/passkeys.ts\n+export async function begin(user: User) {\n+  return generateRegistrationOptions({ rpID, userName: user.email })\n+}\n*** End Patch' }, 700),
    result('c1', 'Success. Updated src/auth/passkeys.ts', 698),
    say('Routes and the migration are in. The auth tests should pass before I touch the login page.', 120, 'gpt-6-astra'),
  ]
}

function notesChat(): ChatItem[] {
  return [
    user('Write the release notes for 2.4 from the merged pull requests since 2.3.', 1800),
    tool('n1', 'Bash', { command: 'gh pr list --state merged --search "merged:>2026-09-28" --json title,number', description: 'List merged pull requests' }, 1790),
    result('n1', '', 1780, { stdout: '[{"number":412,"title":"Faster image uploads"},{"number":409,"title":"Dark mode for the editor"},…]' }),
    say('## 2.4\n\n- **Faster image uploads.** Large photos upload in about half the time (#412).\n- **Dark mode in the editor** follows your system setting (#409).\n- Fixed a crash when pasting a table from a spreadsheet (#405).\n\nSaved to `CHANGELOG.md`.', 1500, 'claude-opus-5-5'),
    end(1500, 252_000),
  ]
}

export function chat(paneId: string): { info: ChatInfo; items: ChatItem[]; screen: ScreenState } {
  seq = 0
  const base = { hostId: 'local', paneId, running: true, queue: [], tasks: [], subagents: [], artifacts: [], stats: { turns: 3, compactions: 0 } }
  if (paneId === PANES.passkeys) {
    return {
      info: { ...base, agent: 'codex', title: 'Migrate auth to passkeys', cwd: '/home/sam/code/accounts', model: 'gpt-6-astra', gitBranch: 'passkeys', mode: 'auto' },
      items: passkeysChat(),
      screen: {
        working: false, width: 120,
        prompt: { body: ['Would you like to run the following command?', '$ bun test src/auth'], options: CODEX_OPTIONS.map((o, i) => ({ ...o, selected: i === 0 })), multi: false },
      },
    }
  }
  if (paneId === PANES.notes) {
    return {
      info: { ...base, hostId: 'build-box', agent: 'claude', title: 'Write release notes', cwd: '/home/sam/code/docs-site', model: 'claude-opus-5-5', gitBranch: 'main', mode: 'default' },
      items: notesChat(),
      screen: { working: false, inputVisible: true, width: 120 },
    }
  }
  return {
    info: {
      ...base, agent: 'claude', title: 'Fix flaky checkout test', cwd: '/home/sam/code/storefront', model: 'claude-sonnet-5-5', gitBranch: 'fix/checkout-flake', mode: 'acceptEdits',
      status: { ...claude('Sonnet 5.5', 'claude-sonnet-5-5', 'medium', 61200, 3.42, 38) } as any,
      stats: { turns: 6, compactions: 0, costUsd: 3.42, durationMs: 38 * 60000, linesAdded: 214, linesRemoved: 61, contextTokens: 61200 },
    },
    items: checkoutChat(),
    screen: { working: true, inputVisible: true, activity: 'Running the checkout suite… (2m 14s · ↓ 3.1k tokens)', width: 120 },
  }
}

/** What follows once a step in a walkthrough happens. */
export const followUps = {
  /** Codex runs the tests after "Yes" */
  passkeysRan: (): ChatItem[] => {
    seq = 40
    return [
      tool('c2', 'Bash', { command: 'bun test src/auth', description: 'Run the auth tests' }, 2),
      result('c2', '', 1, { stdout: ' 31 pass\n 0 fail\nRan 31 tests across 6 files. [1.84s]' }),
      say('All 31 auth tests pass. Next I\'ll switch the login page to `startAuthentication()` and keep the password form behind the flag.', 0, 'gpt-6-astra'),
    ]
  },
  /** the checkout run finishes */
  checkoutDone: (): ChatItem[] => {
    seq = 60
    return [
      result('t5', '', 1, { stdout: ' 1900 pass\n 0 fail\nRan 1900 tests across 2 files. [41.2s]' }),
      say('**1,900 runs, no failures.** The discount now applies before `applyCode()` returns, so `total` is right on the next line. I\'ll add a regression test that reads it straight away.', 0),
    ]
  },
  /** a reply to the message typed on the phone */
  reply: (text: string): ChatItem[] => {
    seq = 80
    return [
      { kind: 'user', seq: 81, uuid: 'sent-1', ts: ts(1), text, images: 0 },
      say('Good idea. I\'ll name it **discount applies before the next read** and run the whole suite once more before committing.', 0),
    ]
  },
}

/** A Claude Code screen for the terminal view (ANSI), short lines so it reads on a phone too. */
export function terminal(cols: number): string {
  const dim = (t: string) => `\x1b[2m${t}\x1b[22m`
  const coral = (t: string) => `\x1b[38;2;215;119;87m${t}\x1b[39m`
  const green = (t: string) => `\x1b[32m${t}\x1b[39m`
  const red = (t: string) => `\x1b[31m${t}\x1b[39m`
  const bold = (t: string) => `\x1b[1m${t}\x1b[22m`
  const narrow = cols < 60
  return [
    `${coral('✻')} ${bold('Claude Code')} ${dim('v2.1.296')}`,
    dim('  Sonnet 5.5 · Claude Max'),
    dim('  ~/code/storefront'),
    '',
    `${dim('>')} The checkout test fails about one run in five`,
    `  on CI. Find out why and fix it.`,
    '',
    `${coral('●')} I'll run the checkout suite repeatedly to`,
    `  catch the failure first.`,
    '',
    `${coral('●')} ${bold('Bash')}(bun test src/checkout --rerun-each 20)`,
    `  ⎿  ${green('76 pass')}`,
    `     ${red('4 fail')}`,
    '',
    `${coral('●')} ${bold('Update')}(src/checkout/cart.ts)`,
    `  ⎿  Updated cart.ts with 3 additions and 1 removal`,
    `     ${dim('41')}   applyCode(code: string) {`,
    `     ${dim('42')} ${red(narrow ? '-    queueMicrotask(…)' : '-    queueMicrotask(() => (this.discount = rate(code)))')}`,
    `     ${dim('42')} ${green('+    this.discount = rate(code)')}`,
    `     ${dim('43')} ${green("+    this.emit('change')")}`,
    '',
    `${coral('●')} ${bold('Bash')}(bun test src/checkout --rerun-each 50)`,
    `  ⎿  ${dim('Running…')}`,
    '',
    `${coral('✶')} ${coral('Running the checkout suite…')} ${dim('(2m 14s · esc to interrupt)')}`,
    '',
    `${dim('>')} `,
    `  ${dim('⏵⏵ accept edits on (shift+tab to cycle)')}`,
  ].join('\r\n')
}
