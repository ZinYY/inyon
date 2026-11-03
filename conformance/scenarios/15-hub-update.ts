import { type Scenario } from '../lib/scenario.ts'

export default {
  name: 'hub-update',
  title: "The hub update check: status, a forced refresh, and starting an update on a hub that can't run one",
  async run(ctx) {
    await ctx.rest('GET', '/api/hub/update', undefined, { as: 'anon' })
    await ctx.signIn()
    await ctx.step('the app asks whether the hub can update (this hub is a development build, which updates with git and cargo)')
    await ctx.rest('GET', '/api/hub/update')
    await ctx.rest('GET', '/api/hub/update?refresh=1')
    await ctx.step('the app starts an update anyway')
    await ctx.rest('POST', '/api/hub/update')
  },
} satisfies Scenario
