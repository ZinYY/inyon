import type { Claude } from '../lib/agents.ts'
import { claudeBox, KEY, online, P, PANE } from '../lib/claude-box.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { agentState } from '../lib/wait.ts'

let claude: Claude
const DRAFT = 'Please fix the wrapped prompt test'

export default {
  name: 'claude-send',
  title: 'Sending to Claude Code: the draft mirror, the closed-loop send, its delivery receipt, the finished notice',
  machine: (m) => void (claude = claudeBox(m)),
  async run(ctx) {
    const app = await online(ctx)
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.reset', (m) => m.t === 'chat.reset' && m.key === KEY)
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)

    await ctx.step('the app types a draft; the hub mirrors it into Claude\'s input box')
    app.send({ t: 'draft', host: BOX, pane: PANE, text: DRAFT })
    await ctx.untilMachine('draft in the box', () => claude.tui.draft === DRAFT)

    await ctx.step('the app sends it: Enter, and the hub waits for Claude to record it')
    await ctx.rest('POST', `${P}/send`, { text: DRAFT, requestId: 'app-send-1' })
    await app.until('working', agentState(BOX, PANE, 'working'))

    await ctx.step('Claude replies and the turn ends')
    claude.replied('Fixed: the parser now waits for the last wrapped row before joining.', 30)
    await app.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')
    await app.until('idle', agentState(BOX, PANE, 'idle'))

    await ctx.step('the same send again (a retry) is not typed twice')
    await ctx.rest('POST', `${P}/send`, { text: DRAFT, requestId: 'app-send-1' })

    await ctx.step('a message typed into an empty box')
    await ctx.rest('POST', `${P}/send`, { text: 'Thanks. Now run all the tests.', requestId: 'app-send-2' })
    await app.until('working', agentState(BOX, PANE, 'working'))
    claude.replied('All 42 tests pass.', 50)
    await app.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')

    await ctx.step('a message of several lines goes in as one paste')
    await ctx.rest('POST', `${P}/send`, { text: 'Two more things:\n1. update the changelog\n2. commit it', requestId: 'app-send-3' })
    await app.until('working', agentState(BOX, PANE, 'working'))
    claude.replied('Done, committed as "Fix wrapped prompts".', 70)
    await app.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')

    await ctx.step('bad sends: empty, a bad id')
    await ctx.rest('POST', `${P}/send`, { text: '   ' })
    await ctx.rest('POST', `${P}/send`, { text: 'hi', requestId: 'has spaces' })
  },
} satisfies Scenario
