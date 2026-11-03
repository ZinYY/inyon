import type { Scenario } from '../lib/scenario.ts'

const PASSWORD = 'correct horse battery'

export default {
  name: 'setup-signin',
  title: 'First run: setup code and password, the session cookie, sign out, sign in, change the password',
  async run(ctx) {
    const code = ctx.hub.setupCode()
    ctx.norm.learn(code, 'setup-code')
    ctx.norm.learn(code.toUpperCase(), 'setup-code')

    await ctx.step('a new hub asks for a password and a setup code')
    await ctx.rest('GET', '/api/session', undefined, { as: 'anon' })
    await ctx.rest('GET', '/api/hosts', undefined, { as: 'anon' })

    await ctx.step('setup is refused: wrong code, short password, another site')
    await ctx.rest('POST', '/api/setup', { password: PASSWORD, code: 'nope' }, { as: 'anon', origin: ctx.hub.base })
    await ctx.rest('POST', '/api/setup', { password: 'short', code }, { as: 'anon', origin: ctx.hub.base })
    await ctx.rest('POST', '/api/setup', { password: PASSWORD, code }, { as: 'anon', origin: 'https://evil.example' })
    await ctx.rest('POST', '/api/setup', undefined, { as: 'anon', origin: ctx.hub.base, raw: '{not json' })

    await ctx.step('setup with the code from the hub signs this browser in')
    const r = await ctx.rest('POST', '/api/setup', { password: PASSWORD, code: code.toUpperCase() }, { as: 'anon', origin: ctx.hub.base })
    ctx.takeCookie(r)
    const first = ctx.cookie
    await ctx.rest('GET', '/api/session')
    await ctx.rest('POST', '/api/setup', { password: PASSWORD, code }, { as: 'anon', origin: ctx.hub.base })

    await ctx.step('the app connects with the cookie; a cookieless or foreign connection is refused')
    await ctx.connect('app')
    await ctx.refusedUpgrade('no-cookie', '/ws', { origin: ctx.hub.base })
    await ctx.refusedUpgrade('foreign', '/ws', { cookie: ctx.cookie, origin: 'https://evil.example' })
    await ctx.refusedUpgrade('bad-ticket', '/ws?ticket=nope', {})

    await ctx.step('signing out ends the session everywhere, and its connections')
    const app = ctx.sockets.find((s) => s.name === 'app')!
    await ctx.rest('POST', '/api/logout')
    ctx.entry({ ws: 'app', closed: await app.untilClosed() })
    await ctx.rest('GET', '/api/session', undefined, { as: { cookie: first } })
    await ctx.rest('GET', '/api/hosts', undefined, { as: { cookie: first } })

    await ctx.step('sign in with the password')
    await ctx.rest('POST', '/api/login', { password: 'not it' }, { as: 'anon', origin: ctx.hub.base })
    ctx.takeCookie(await ctx.rest('POST', '/api/login', { password: PASSWORD }, { as: 'anon', origin: ctx.hub.base }))
    await ctx.rest('GET', '/api/session')

    await ctx.step('change the password: the old cookie stops working, this browser gets a new one')
    const before = ctx.cookie
    await ctx.rest('POST', '/api/password', { current: 'wrong one', next: 'another good password' })
    await ctx.rest('POST', '/api/password', { current: PASSWORD, next: 'short' })
    ctx.takeCookie(await ctx.rest('POST', '/api/password', { current: PASSWORD, next: 'another good password' }))
    await ctx.rest('GET', '/api/session', undefined, { as: { cookie: before } })
    await ctx.rest('GET', '/api/session')
    await ctx.rest('POST', '/api/login', { password: PASSWORD }, { as: 'anon', origin: ctx.hub.base })
  },
} satisfies Scenario
