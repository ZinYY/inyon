// Made-up machines and sessions for the app's browser checks: nothing from a real account.
export function overview(now = Date.now()) {
  const s = Math.floor(now / 1000)
  const limits = { fiveHour: { pct: 22, resetsAt: s + 2 * 3600 + 900 }, sevenDay: { pct: 41, resetsAt: s + 3 * 86400 } }
  let n = 10
  const pane = (title: string, cwd: string, command: string, agent: Record<string, unknown>) => {
    n++
    return { id: `%${n}`, index: 0, active: true, command, cwd, title, width: 80, height: 40, dead: false, agent: { pid: 4000 + n, startedAt: now - 3600_000, mappedBy: 'hook', ...agent } }
  }
  const session = (name: string, win: string, p: ReturnType<typeof pane>, ago: number) => ({
    id: `$${n}`,
    name,
    attached: 0,
    windows: [{ id: `@${n}`, index: 1, name: win, active: true, activity: s - ago, panes: [p] }],
  })
  const host = (id: string, name: string, kind: string, sessions: unknown[], extra: Record<string, unknown> = {}) => ({
    config: { id, name, kind },
    status: 'online',
    hostname: name,
    integration: 'installed',
    tmux: 'ok',
    agents: ['claude', 'codex', 'opencode'],
    os: 'Linux',
    latencyMs: 38,
    sessions,
    updatedAt: now,
    lastSeen: now,
    ...extra,
  })
  return {
    t: 'overview',
    hosts: [
      host('local', 'studio', 'local', [
        session('storefront', 'claude', pane('✳ Fix flaky checkout test', '/home/sam/code/storefront', 'claude', {
          kind: 'claude', state: 'working', activity: 'Running the checkout suite… (2m 14s · ↓ 3.1k tokens)',
          status: { model: 'Sonnet 5.5', effort: 'medium', contextPct: 6.1, contextUsed: 61200, contextSize: 1000000, costUsd: 3.42, updatedAt: now - 4000, ...limits },
        }), 3),
        session('accounts', 'codex', pane('Migrate auth to passkeys', '/home/sam/code/accounts', 'node', {
          kind: 'codex', state: 'waiting', waitingFor: 'Run bun test src/auth?', options: [{ n: 1, label: 'Yes' }, { n: 2, label: 'No' }],
          status: { model: 'GPT-6-Astra', effort: 'medium', contextPct: 18.7, contextUsed: 48300, contextSize: 258400, sevenDay: { pct: 27, resetsAt: s + 5 * 86400 }, updatedAt: now - 9000 },
        }), 40),
      ], { machineKind: 'server' }),
      host('build-box', 'build-box', 'node', [
        session('docs', 'claude', pane('✳ Write release notes', '/home/sam/code/docs-site', 'claude', {
          kind: 'claude', state: 'idle',
          status: { model: 'Opus 5.5', effort: 'high', contextPct: 9.2, contextUsed: 92400, contextSize: 1000000, costUsd: 1.86, updatedAt: now - 60000 },
        }), 260),
        session('billing', 'opencode', pane('Tidy the billing API types', '/home/sam/code/billing', 'opencode', {
          kind: 'opencode', state: 'idle', background: '1 shell',
          status: { model: 'Qwen3 Coder', contextPct: 11.6, contextUsed: 30500, contextSize: 262144, costUsd: 0.74, updatedAt: now - 60000 },
        }), 900),
      ], { machineKind: 'laptop', nodeVersion: '0.23.1' }),
    ],
  }
}

// ---- conversations, for the session screen ----

type Item = Record<string, unknown>

/** A chat's items as the hub's record has them, numbered from `seq`. */
function transcript(start: number, now: number) {
  let seq = start
  const items: Item[] = []
  const ts = (ago: number) => new Date(now - ago * 1000).toISOString()
  const add = (kind: string, ago: number, rest: Item) => items.push({ kind, seq: seq++, uuid: `u${seq}`, ts: ts(ago), ...rest })
  return {
    items,
    user: (ago: number, text: string) => add('user', ago, { text, images: 0 }),
    text: (ago: number, text: string) => add('assistant', ago, { msgId: `m${seq}`, block: { type: 'text', text } }),
    thinking: (ago: number, text: string) => add('assistant', ago, { msgId: `m${seq}`, block: { type: 'thinking', text } }),
    tool: (ago: number, id: string, name: string, input: Item) => add('assistant', ago, { msgId: `m${seq}`, block: { type: 'tool_use', id, name, input } }),
    result: (ago: number, id: string, text: string, extra: Item = {}) => add('tool_result', ago, { toolUseId: id, isError: false, text, ...extra }),
    end: (ago: number, ms: number) => add('turn_end', ago, { durationMs: ms }),
  }
}

export const info = (host: string, pane: string, agent: string, extra: Item = {}) => ({
  hostId: host, paneId: pane, agent, running: true, cwd: '/home/sam/code/storefront', queue: [], tasks: [], subagents: [], artifacts: [], stats: { turns: 3, compactions: 0 }, ...extra,
})

/** Claude Code fixing a flaky test: a long history (so the list has to page), tool calls, a reply
 * with an indented code block, and a message whose own indentation has to survive. */
export function storefront(now = Date.now()) {
  const t = transcript(1, now)
  // older, quieter exchanges, so the list is longer than any screen
  for (let i = 0; i < 30; i++) {
    t.user(7200 - i * 200, `Check step ${i + 1} of the cart refactor`)
    t.text(7190 - i * 200, `Step ${i + 1} is in: the cart keeps its items in one map now, and the old tests pass.`)
    t.end(7185 - i * 200, 9000 + i * 300)
  }
  t.user(900, 'The checkout test fails about one run in five. It stops here:\n\n    expect(total).toBe(4200)\n      at checkout.test.ts:88\n\nCan you find out why?')
  t.thinking(890, 'A failure one run in five smells like a race. The total is probably read before something async settles.')
  t.tool(880, 't1', 'Bash', { command: 'bun test src/checkout --rerun-each 20', description: 'Run the checkout suite 20 times' })
  t.result(820, 't1', '4 fail, 16 pass\n  ✗ total includes the discount\n    expected 4200, got 4600')
  t.tool(815, 't2', 'Read', { file_path: '/home/sam/code/storefront/src/checkout/total.ts' })
  t.result(814, 't2', 'export function total(cart: Cart) {\n  applyDiscount(cart)\n  return cart.items.reduce((sum, item) => sum + item.price * item.qty, 0)\n}')
  t.text(800, "Found it. `applyDiscount` is async, and `total()` reads the cart before it settles, so a slow discount lookup leaves the old price in.\n\n```ts\nexport async function total(cart: Cart) {\n  const discounted = await applyDiscount(cart)\n  return discounted.items.reduce(\n    (sum, item) => sum + item.price * item.qty,\n    0,\n  )\n}\n```\n\nI'll make `total()` wait for it and run the suite again.")
  t.tool(790, 't3', 'Edit', { file_path: '/home/sam/code/storefront/src/checkout/total.ts', old_string: 'export function total(cart: Cart) {\n  applyDiscount(cart)', new_string: 'export async function total(cart: Cart) {\n  const discounted = await applyDiscount(cart)' })
  t.result(789, 't3', 'The file was updated.')
  t.tool(785, 't4', 'Bash', { command: 'bun test src/checkout --rerun-each 20', description: 'Run the checkout suite 20 times' })
  t.result(700, 't4', '20 pass, 0 fail')
  t.text(690, 'All 20 runs pass now. The test was right; `total()` had a race with the discount lookup.\n\n- `total()` waits for `applyDiscount`\n- its two callers already awaited it, so nothing else changes')
  t.end(689, 211000)
  t.user(140, 'Nice. Run the whole suite once more before you commit.')
  t.tool(130, 't5', 'Bash', { command: 'bun test', description: 'Run the whole suite' })
  return {
    items: t.items,
    older: (() => {
      const o = transcript(-2, now)
      o.user(9000, 'Start the cart refactor: one map for the items')
      o.text(8990, 'Started. The cart keeps a map from product id to line now.')
      return o.items
    })(),
    state: {
      info: info('local', '%11', 'claude', { title: 'Fix flaky checkout test', model: 'claude-sonnet-5-5' }),
      screen: { working: true, activity: 'Running the whole suite… (2m 14s · esc to interrupt)', model: 'Sonnet 5.5', effort: 'medium', mode: 'default', width: 120 },
      hasMore: true,
      loaded: true,
    },
  }
}

/** Codex asking before it runs a command. */
export function accounts(now = Date.now()) {
  const t = transcript(1, now)
  t.user(600, 'Move sign-in to passkeys and keep the password path for old accounts.')
  t.text(560, 'The passkey routes are in. Before I go on I want to run the auth tests.')
  return {
    items: t.items,
    state: {
      info: info('local', '%12', 'codex', { title: 'Migrate auth to passkeys', cwd: '/home/sam/code/accounts', model: 'gpt-6-astra' }),
      screen: {
        working: false,
        width: 120,
        model: 'GPT-6-Astra',
        effort: 'medium',
        prompt: {
          body: ['Would you like to run the following command?', '╌', ' $ bun test src/auth', '╌', 'Reason: check the passkey routes before going on'],
          options: [
            { n: 1, label: 'Yes, proceed', selected: true },
            { n: 2, label: "Yes, and don't ask again for this command", selected: false },
            { n: 3, label: 'No, and tell Codex what to do differently', selected: false },
          ],
          multi: false,
        },
      },
      hasMore: false,
      loaded: true,
    },
  }
}

/** The next thing it asks: two questions on tabs. */
export const twoQuestions = {
  body: ['Which store should the passkeys live in?', 'The accounts table has room, or they can have their own.'],
  options: [
    { n: 1, label: 'In the accounts table', detail: 'One join fewer on sign-in', selected: true },
    { n: 2, label: 'A passkeys table', detail: 'Several keys per account', selected: false },
    { n: 3, label: 'Type something', selected: false },
  ],
  tabs: [{ label: 'Storage', done: false, current: true }, { label: 'Fallback', done: false, current: false }, { label: 'Submit', done: false, current: false }],
  multi: false,
  freeTextOption: 3,
}
