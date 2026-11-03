import path from 'node:path'
import { Claude, claudeHistory } from '../lib/agents.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { onlineWith } from '../lib/wait.ts'

export default {
  name: 'overview',
  title: 'A machine connects; the overview lists its sessions, a Claude pane and a shell',
  machine(m) {
    const cwd = path.join(m.home, 'code/inyon')
    new Claude(m, { id: '%1', sessionId: '11111111-2222-4333-8444-555555555555', cwd, pid: 4100001, history: claudeHistory(cwd, '11111111-2222-4333-8444-555555555555') })
    m.addPane({ id: '%2', session: { id: '$1', name: 'work' }, window: { id: '@2', index: 1, name: 'shell', activity: 1790000000, active: false }, cwd, title: 'box', shell: { pid: 4100010, comm: 'zsh' } })
  },
  async run(ctx) {
    await ctx.signIn()
    await ctx.step('the app subscribes before the machine connects')
    const app = await ctx.connect()
    app.send({ t: 'sub.overview' })
    await app.until('overview', (m) => m.t === 'overview')
    await ctx.step('the machine connects and is probed')
    await ctx.connectNode()
    await app.until('box online', onlineWith(BOX, 2))
    await ctx.step('pull to refresh')
    await ctx.rest('POST', '/api/refresh')
    await ctx.step('the machine list and settings')
    await ctx.rest('GET', '/api/hosts')
    await ctx.rest('GET', '/api/settings')
    await ctx.step('ping')
    app.send({ t: 'ping' })
    await app.until('pong', (m) => m.t === 'pong')
  },
} satisfies Scenario
