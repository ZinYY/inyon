import { claudeBox, online, P, PANE } from '../lib/claude-box.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { hostStatus } from '../lib/wait.ts'

export default {
  name: 'errors',
  title: 'Errors: unknown machine and pane, bad input, sign-in and origin checks, wrong Host, too large, an offline machine',
  machine: (m) => void claudeBox(m),
  async run(ctx) {
    const app = await online(ctx)

    await ctx.step('a machine or pane that is not there')
    await ctx.rest('POST', '/api/hosts/nowhere/panes/1/send', { text: 'hi' })
    await ctx.rest('POST', `/api/hosts/${BOX}/panes/99/send`, { text: 'hi' })
    await ctx.rest('GET', `/api/hosts/nowhere/transcript?pane=${encodeURIComponent(PANE)}`)
    await ctx.rest('PATCH', '/api/hosts/nowhere', { name: 'x' })
    await ctx.rest('POST', '/api/hosts/nowhere/node/update')
    await ctx.rest('GET', '/api/no-such-route')
    await ctx.rest('DELETE', '/api/settings')

    await ctx.step('bad input')
    await ctx.rest('POST', `${P}/send`, undefined, { raw: '{not json', headers: { 'content-type': 'application/json' } })
    await ctx.rest('POST', `${P}/keys`, { keys: Array(21).fill('Up') })
    await ctx.rest('POST', `${P}/keys`, { keys: 'Enter' })
    await ctx.rest('POST', `${P}/send`, { text: 'x'.repeat(1024 * 1024 + 10) })
    await ctx.rest('POST', '/api/hosts', { name: '', target: 'me@host' })
    await ctx.rest('POST', '/api/hosts', { name: 'x', target: '-oProxyCommand=evil' })
    await ctx.rest('POST', '/api/hosts', { name: 'x', target: 'me@host', port: 70000 })
    await ctx.rest('POST', '/api/hosts', { name: 'x', target: 'me@host', identityFile: '~/.ssh/id; rm -rf /' })

    await ctx.step('settings are clamped to their ranges')
    await ctx.rest('PUT', '/api/settings', { autoResume: false, uploads: { retentionDays: 9999, maxMB: 0, maxTotalMB: -5, longPasteLines: 1 } })
    await ctx.rest('PUT', '/api/settings', { autoResume: 'yes', uploads: { retentionDays: 7, maxMB: 100, maxTotalMB: 1000, longPasteLines: 50 } })

    await ctx.step('sign-in and origin checks')
    await ctx.rest('GET', '/api/hosts', undefined, { as: 'anon' })
    await ctx.rest('GET', '/api/hosts', undefined, { as: { cookie: 'inyon_session=forged.value' } })
    await ctx.rest('POST', '/api/refresh', undefined, { origin: 'https://evil.example' })
    await ctx.rest('POST', '/api/refresh', undefined, { origin: null })
    await ctx.refusedUpgrade('cross-site app', '/ws', { origin: 'https://evil.example', cookie: ctx.cookieFor('browser') })

    await ctx.step('a name the hub does not answer to')
    await ctx.rest('GET', '/api/session', undefined, { as: 'anon', headers: { host: 'evil.example' } })
    await ctx.rest('GET', '/api/session', undefined, { as: 'anon', headers: { 'x-forwarded-host': 'evil.example' } })
    await ctx.rest('GET', '/', undefined, { as: 'anon', headers: { host: 'evil.example' } })
    await ctx.rest('GET', '/api/session', undefined, { headers: { host: 'evil.example' } })

    await ctx.step('the machine drops off: it shows as reconnecting, and its panes refuse')
    ctx.node!.close()
    await app.until('box not online', (m) => m.t === 'overview' && !hostStatus(BOX, 'online')(m))
    await ctx.rest('POST', `${P}/send`, { text: 'hello?' })
    await ctx.rest('POST', `${P}/keys`, { keys: ['Enter'] })
    app.send({ t: 'term.open', id: 'off', host: BOX, pane: PANE, cols: 80, rows: 24 })
    await app.until('terminal refused', (m) => m.t === 'term.exit' && m.id === 'off')
  },
} satisfies Scenario
