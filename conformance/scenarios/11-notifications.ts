import type { Claude } from '../lib/agents.ts'
import { claudeBox, online, PANE } from '../lib/claude-box.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { agentState } from '../lib/wait.ts'

let claude: Claude

export default {
  name: 'notifications',
  title: 'Notices: every open app hears that Claude needs you and that it finished, watching the overview or not',
  machine: (m) => void (claude = claudeBox(m)),
  async run(ctx) {
    const watcher = await online(ctx)
    await ctx.step('a second app is open on another page: no overview, still told')
    const quiet = await ctx.connect('quiet')

    await ctx.step('Claude starts working on the machine itself (no notice)')
    claude.took('Tidy up the imports', 20)
    await watcher.until('working', agentState(BOX, PANE, 'working'))

    await ctx.step('Claude needs permission: both apps are told')
    claude.tui.show('permission')
    await watcher.until('waiting notice', (m) => m.t === 'notify' && m.kind === 'waiting')
    await quiet.until('waiting notice', (m) => m.t === 'notify' && m.kind === 'waiting')

    await ctx.step('someone answers at the machine; Claude carries on (no notice)')
    claude.tui.show('working')
    await watcher.until('working', agentState(BOX, PANE, 'working'))

    await ctx.step('Claude finishes: the notice carries the start of its reply')
    claude.replied('Imports sorted and the unused ones removed. Nothing else changed, and all the tests still pass after the cleanup.', 40)
    await watcher.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')
    await quiet.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')
  },
} satisfies Scenario
