import fs from 'node:fs'
import path from 'node:path'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { onlineWith } from '../lib/wait.ts'

export default {
  name: 'local-cli',
  title: 'The inyon command on the hub machine lists the machines with the local token; anyone else is refused',
  machine: () => {},
  async run(ctx) {
    const token = ctx.hub.localToken()
    ctx.norm.learn(token, 'local-token')
    // readable only by this user, and long enough not to guess
    const mode = fs.statSync(path.join(ctx.hub.dataDir, 'local-token')).mode & 0o777
    ctx.entry({ file: 'local-token', mode: mode.toString(8), atLeast32Characters: token.length >= 32 })
    await ctx.signIn()
    await ctx.step('box connects')
    await ctx.connectNode()
    const app = await ctx.connect()
    app.send({ t: 'sub.overview' })
    await app.until('box online', onlineWith(BOX, 0))

    await ctx.step('with the token, as a header or in the query')
    await ctx.rest('GET', '/api/local/nodes', undefined, { as: { bearer: token } })
    await ctx.rest('GET', `/api/local/nodes?token=${token}`, undefined, { as: 'anon' })
    await ctx.step('without it, with a wrong one, with only the browser sign-in')
    await ctx.rest('GET', '/api/local/nodes', undefined, { as: 'anon' })
    await ctx.rest('GET', '/api/local/nodes', undefined, { as: { bearer: 'not-the-local-token' } })
    await ctx.rest('GET', `/api/local/nodes?token=${'x'.repeat(token.length)}`, undefined, { as: 'anon' })
    await ctx.rest('GET', '/api/local/nodes')
  },
} satisfies Scenario
