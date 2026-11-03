import path from 'node:path'
import { opencode } from '../lib/agents.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { onlineWith } from '../lib/wait.ts'

const PANE = '%7'
const KEY = `${BOX}:${PANE}`
const P = `/api/hosts/${BOX}/panes/7`

export default {
  name: 'opencode',
  title: 'An OpenCode session: in the overview, its chat read from its database, its models and commands',
  machine: (m) => void opencode(m, { id: PANE, cwd: path.join(m.home, 'code/hello'), pid: 4300001 }),
  async run(ctx) {
    await ctx.signIn()
    await ctx.step('the machine connects with an OpenCode pane')
    await ctx.connectNode()
    const app = await ctx.connect()
    app.send({ t: 'sub.overview' })
    await app.until('box online', onlineWith(BOX, 1))

    await ctx.step('the app opens the chat')
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.reset', (m) => m.t === 'chat.reset' && m.key === KEY)
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)

    await ctx.step('its models and commands')
    await ctx.rest('GET', `${P}/models`)
    await ctx.rest('GET', `${P}/commands`)
    await ctx.rest('GET', `${P}/conversations`)
  },
} satisfies Scenario
