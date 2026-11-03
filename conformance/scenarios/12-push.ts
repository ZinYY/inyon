import { type Scenario } from '../lib/scenario.ts'

/** nothing listens on port 1: a push to it fails at once, the same way every run */
const ENDPOINT = 'https://127.0.0.1:1/push/conformance'

export default {
  name: 'push',
  title: 'Web Push registration routes: bad registrations, a good one, a test push that cannot be delivered, removal',
  async run(ctx) {
    // the browser's push keys, as its push service hands them out, made for this run only
    const browser = await crypto.subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, true, ['deriveBits'])
    const KEYS = {
      p256dh: Buffer.from(await crypto.subtle.exportKey('raw', browser.publicKey)).toString('base64url'),
      auth: Buffer.from(crypto.getRandomValues(new Uint8Array(16))).toString('base64url'),
    }
    ctx.norm.learn(KEYS.p256dh, 'p256dh')
    ctx.norm.learn(KEYS.auth, 'auth')
    // and its own VAPID key
    const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign'])
    const jwk = await crypto.subtle.exportKey('jwk', pair.privateKey)
    ctx.norm.learn(jwk.d, 'vapid-d')
    ctx.norm.learn(jwk.x, 'vapid-x')
    ctx.norm.learn(jwk.y, 'vapid-y')
    const vapid = { privateJwk: { kty: 'EC', crv: 'P-256', d: jwk.d, x: jwk.x, y: jwk.y }, subject: 'mailto:conformance@example.com' }
    const good = { subscription: { endpoint: ENDPOINT, keys: KEYS }, vapid, done: true }

    await ctx.signIn()
    await ctx.step('registrations the hub refuses')
    await ctx.rest('POST', '/api/push', { ...good, subscription: { endpoint: 'http://push.example.net/x', keys: KEYS } })
    await ctx.rest('POST', '/api/push', { ...good, subscription: { endpoint: ENDPOINT } })
    await ctx.rest('POST', '/api/push', { ...good, vapid: { ...vapid, privateJwk: { ...vapid.privateJwk, crv: 'P-384' } } })
    await ctx.rest('POST', '/api/push', { ...good, vapid: { ...vapid, subject: 'conformance@example.com' } })
    await ctx.rest('POST', '/api/push', { ...good, padding: 'x'.repeat(17 * 1024) })
    await ctx.rest('POST', '/api/push', good, { as: 'anon' })

    await ctx.step('a good registration, and a test push that cannot reach its service')
    await ctx.rest('POST', '/api/push', good)
    await ctx.rest('POST', '/api/push/test', { endpoint: ENDPOINT })
    await ctx.rest('POST', '/api/push/test', { endpoint: 'https://127.0.0.1:1/not-registered' })
    await ctx.rest('POST', '/api/push/test', {})

    await ctx.step('notifications turned off: the registration goes')
    await ctx.rest('DELETE', '/api/push', { endpoint: ENDPOINT })
    await ctx.rest('DELETE', '/api/push', {})
    await ctx.rest('POST', '/api/push/test', { endpoint: ENDPOINT })
  },
} satisfies Scenario
