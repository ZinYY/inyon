import type { Scenario } from '../lib/scenario.ts'

export default {
  name: 'login-limits',
  title: 'Wrong passwords (at sign-in and as the current one on a change): eight a minute per client, counted per X-Forwarded-For client behind a local proxy',
  async run(ctx) {
    await ctx.signIn()
    await ctx.step('the current password on a password change counts against the same limit')
    for (let i = 0; i < 9; i++) await ctx.rest('POST', '/api/password', { current: `guess ${i}`, next: 'a whole new password' }, { origin: ctx.hub.base, headers: { 'x-forwarded-for': '203.0.113.20' } })
    await ctx.rest('POST', '/api/logout', undefined, { quiet: true })
    await ctx.step('eight wrong passwords')
    for (let i = 0; i < 8; i++) await ctx.rest('POST', '/api/login', { password: `guess ${i}` }, { as: 'anon', origin: ctx.hub.base })
    await ctx.step('then even the right password waits a minute')
    await ctx.rest('POST', '/api/login', { password: 'correct horse battery' }, { as: 'anon', origin: ctx.hub.base })
    await ctx.step('another client behind a local proxy has its own count')
    await ctx.rest('POST', '/api/login', { password: 'correct horse battery' }, { as: 'anon', origin: ctx.hub.base, headers: { 'x-forwarded-for': '198.51.100.7, 203.0.113.9' } })
    await ctx.step('a password over 1024 characters is not checked')
    await ctx.rest('POST', '/api/login', { password: 'x'.repeat(1025) }, { as: 'anon', origin: ctx.hub.base, headers: { 'x-forwarded-for': '203.0.113.10' } })
  },
} satisfies Scenario
