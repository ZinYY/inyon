import crypto from 'node:crypto'
import { type Scenario } from '../lib/scenario.ts'

/** where the phone app runs: an origin of its own */
const APP = 'https://localhost'

export default {
  name: 'phones',
  title: 'Pairing a phone: the code, wrong digits, the claim, its bearer token and tickets, CORS, removal closing its socket',
  async run(ctx) {
    await ctx.signIn()

    await ctx.step('the web app shows a pairing code')
    await ctx.rest('POST', '/api/phones/pair', { origin: 'not a url' })
    const pair = await ctx.rest('POST', '/api/phones/pair', { origin: ctx.hub.base })
    const { id, code } = pair.body as { id: string; code: string }
    ctx.norm.learn(id, 'pair-id')
    // the phone may send the id in any case; one label for both, as an id of digits only has no lower case
    ctx.norm.learn(id.toLowerCase(), 'pair-id')
    ctx.norm.learn(code, 'pair-digits')
    const wrong = String((Number(code) + 1) % 1_000_000).padStart(6, '0')
    ctx.norm.learn(wrong, 'wrong-digits')
    await ctx.rest('POST', '/api/phones/pair', { origin: ctx.hub.base }, { as: 'anon' })

    await ctx.step('a sign-in from more than ten minutes ago shows no code')
    // signed with the hub's own secret, eleven minutes ago
    const then = Date.now() - 11 * 60_000
    const payload = Buffer.from(JSON.stringify({ exp: Date.now() + 86_400_000, n: 'older', iat: then, at: then })).toString('base64url')
    const older = { cookie: `inyon_session=${payload}.${crypto.createHmac('sha256', ctx.hub.secret() ?? '').update(payload).digest('base64url')}` }
    await ctx.rest('GET', '/api/session', undefined, { as: older })
    await ctx.rest('POST', '/api/phones/pair', { origin: ctx.hub.base }, { as: older, origin: ctx.hub.base })

    await ctx.step('a cookie from before the sign-in time was kept shows no code either, however fresh')
    const legacyPayload = Buffer.from(JSON.stringify({ exp: Date.now() + 86_400_000, n: 'legacy', iat: Date.now() })).toString('base64url')
    const legacy = { cookie: `inyon_session=${legacyPayload}.${crypto.createHmac('sha256', ctx.hub.secret() ?? '').update(legacyPayload).digest('base64url')}` }
    await ctx.rest('POST', '/api/phones/pair', { origin: ctx.hub.base }, { as: legacy, origin: ctx.hub.base })

    await ctx.step('the phone asks before claiming (CORS preflight)')
    await ctx.rest('OPTIONS', '/api/phones/claim', undefined, { as: 'anon', origin: APP, headers: { 'access-control-request-method': 'POST', 'access-control-request-headers': 'content-type' } })

    await ctx.step('wrong digits, an unknown code, then the right one')
    await ctx.rest('POST', '/api/phones/claim', { code: `${id}:${wrong}`, name: 'Pixel' }, { as: 'anon', origin: APP })
    await ctx.rest('POST', '/api/phones/claim', { code: 'ZZZZZZZZ:123456', name: 'Pixel' }, { as: 'anon', origin: APP })
    await ctx.rest('POST', '/api/phones/claim', { code: 'nonsense' }, { as: 'anon', origin: APP })
    const claim = await ctx.rest('POST', '/api/phones/claim', { code: `${id.toLowerCase()}:${code}`, name: 'Pixel' }, { as: 'anon', origin: APP })
    const token = (claim.body as { token: string; hub: { id: string } }).token
    ctx.norm.learn(token, 'phone-token')
    ctx.norm.learn((claim.body as { hub: { id: string } }).hub?.id, 'hub-id')
    await ctx.step('a code works once')
    await ctx.rest('POST', '/api/phones/claim', { code: `${id}:${code}`, name: 'Pixel' }, { as: 'anon', origin: APP })

    await ctx.step('the phone signs in with its token')
    const phone = { bearer: token }
    await ctx.rest('OPTIONS', '/api/hosts', undefined, { as: 'anon', origin: APP, headers: { 'access-control-request-method': 'GET', 'access-control-request-headers': 'authorization' } })
    await ctx.rest('OPTIONS', '/api/hosts', undefined, { as: 'anon', origin: APP, headers: { 'access-control-request-method': 'GET' } })
    await ctx.rest('GET', '/api/session', undefined, { as: phone, origin: APP })
    await ctx.rest('GET', '/api/session', undefined, { as: 'anon', origin: APP })
    await ctx.rest('GET', '/api/hosts', undefined, { as: phone, origin: APP })
    const list = await ctx.rest('GET', '/api/phones', undefined, { as: phone, origin: APP })
    const phoneId = (list.body as { id: string }[])[0]?.id
    ctx.norm.learn(phoneId, 'phone-id')
    await ctx.rest('GET', '/api/phones')
    await ctx.rest('GET', '/api/hosts', undefined, { as: { bearer: 'not-a-real-token' }, origin: APP })

    await ctx.step('what only the browser may do')
    await ctx.rest('POST', '/api/phones/pair', { origin: ctx.hub.base }, { as: phone, origin: APP })
    await ctx.rest('DELETE', `/api/phones/${phoneId}`, undefined, { as: phone, origin: APP })
    await ctx.rest('POST', '/api/password', { current: 'x', next: 'y' }, { as: phone, origin: APP })
    await ctx.rest('POST', '/api/setup', { password: 'x', code: 'x' }, { as: phone, origin: APP })
    await ctx.rest('POST', '/api/login', { password: 'correct horse battery' }, { as: phone, origin: APP })
    await ctx.rest('POST', '/api/phones/ticket', undefined)
    // still paired, and the password unchanged
    await ctx.rest('GET', '/api/session', undefined, { as: phone, origin: APP })
    await ctx.rest('POST', '/api/login', { password: 'correct horse battery' }, { as: 'anon' })

    await ctx.step('a ticket opens a WebSocket once')
    const t = await ctx.rest('POST', '/api/phones/ticket', undefined, { as: phone, origin: APP })
    const ticket = (t.body as { ticket: string }).ticket
    ctx.norm.learn(ticket, 'ticket')
    const ws = await ctx.connect('phone', 'anon', `?ticket=${ticket}`)
    ws.send({ t: 'ping' })
    await ws.until('pong', (m) => m.t === 'pong')
    await ctx.refusedUpgrade('phone again', `/ws?ticket=${ticket}`)
    await ctx.refusedUpgrade('bad bearer', '/ws', { authorization: 'Bearer not-a-real-token' })
    // a bearer is judged on its own, even with the browser's good cookie beside it
    await ctx.refusedUpgrade('bad bearer with a cookie', '/ws', { authorization: 'Bearer not-a-real-token', cookie: ctx.cookie })
    // or with the token itself, from code that can send a header
    const direct = await ctx.connect('phone with its token', phone)
    direct.send({ t: 'ping' })
    await direct.until('pong', (m) => m.t === 'pong')

    await ctx.step('the browser removes the phone: its socket closes and its token stops working')
    await ctx.rest('DELETE', `/api/phones/${phoneId}`)
    await ws.untilClosed()
    await direct.untilClosed()
    await ctx.rest('GET', '/api/hosts', undefined, { as: phone, origin: APP })
    await ctx.rest('DELETE', `/api/phones/${phoneId}`)
  },
} satisfies Scenario
