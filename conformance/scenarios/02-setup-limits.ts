import type { Scenario } from '../lib/scenario.ts'

export default {
  name: 'setup-limits',
  title: 'Wrong setup codes are slowed down and capped at eight a minute, even before the right one',
  async run(ctx) {
    const code = ctx.hub.setupCode()
    ctx.norm.learn(code, 'setup-code')
    await ctx.step('eight wrong setup codes')
    for (let i = 0; i < 8; i++) await ctx.rest('POST', '/api/setup', { password: 'correct horse battery', code: `bad${i}` }, { as: 'anon', origin: ctx.hub.base })
    await ctx.step('the ninth try is refused, even with the right code')
    await ctx.rest('POST', '/api/setup', { password: 'correct horse battery', code }, { as: 'anon', origin: ctx.hub.base })
    await ctx.rest('GET', '/api/session', undefined, { as: 'anon' })
  },
} satisfies Scenario
