import path from 'node:path'
import { Codex } from '../lib/agents.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { agentState, onlineWith } from '../lib/wait.ts'

const PANE = '%5'
const KEY = `${BOX}:${PANE}`
const P = `/api/hosts/${BOX}/panes/5`
let codex: Codex

export default {
  name: 'codex',
  title: 'A Codex session: in the overview, its chat from the rollout file, a message sent and answered',
  machine: (m) => void (codex = new Codex(m, { id: PANE, threadId: '019a0000-aaaa-7bbb-8ccc-dddddddddddd', cwd: path.join(m.home, 'code/calc'), pid: 4200001 })),
  async run(ctx) {
    await ctx.signIn()
    await ctx.step('the machine connects with a Codex pane')
    await ctx.connectNode()
    const app = await ctx.connect()
    app.send({ t: 'sub.overview' })
    await app.until('box online', onlineWith(BOX, 1))

    await ctx.step('the app opens the chat')
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.reset', (m) => m.t === 'chat.reset' && m.key === KEY)
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)
    await ctx.rest('GET', `${P}/models`)
    await ctx.rest('GET', `${P}/commands`)

    await ctx.step('the app sends a message: typed, Enter, found in the rollout')
    await ctx.rest('POST', `${P}/send`, { text: 'Now add multiply too', requestId: 'codex-send-1' })
    await app.until('working', agentState(BOX, PANE, 'working'))

    await ctx.step('Codex replies')
    codex.replied('Added multiply(a, b) with a test.', 40)
    await app.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')
    await app.until('idle', agentState(BOX, PANE, 'idle'))
  },
} satisfies Scenario
