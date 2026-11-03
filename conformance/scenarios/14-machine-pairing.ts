import { BOX, HELLO, type Scenario } from '../lib/scenario.ts'
import { hostStatus, onlineWith } from '../lib/wait.ts'

type Paired = { id: string; token: string }

export default {
  name: 'machine-pairing',
  title: 'Adding a machine: the pairing command, the one-time swap of its token, the machine connecting, pairing again, removal and what a removed machine hears',
  machine: () => {},
  async run(ctx) {
    await ctx.signIn()
    await ctx.step('box is online; the app watches the overview')
    await ctx.connectNode()
    const app = await ctx.connect()
    app.send({ t: 'sub.overview' })
    await app.until('box online', onlineWith(BOX, 0))

    await ctx.step('the app asks for a pairing command, twice with the same name')
    const a = (await ctx.rest('POST', '/api/hosts/pair', { name: 'Work Laptop' })).body as Paired
    ctx.norm.learn(a.token, 'laptop-token')
    const b = (await ctx.rest('POST', '/api/hosts/pair', { name: 'Work Laptop', origin: 'http://hub.example.net:8080' })).body as Paired
    ctx.norm.learn(b.token, 'laptop-2-token')
    // no name: called Laptop (transcripts are normalized at the end, so learning a value after its response is enough)
    const d = (await ctx.rest('POST', '/api/hosts/pair', {})).body as Paired
    ctx.norm.learn(d.token, 'laptop-3-token')

    await ctx.step('the laptop runs the command and connects')
    const laptop = await ctx.connectOtherNode('work-laptop', a.token, { ...HELLO, hostname: 'laptop' })
    await app.until('laptop online', hostStatus('work-laptop', 'online'))

    await ctx.step('inyon join swaps the pasted token for one the hub hands out once')
    const claimed = (await ctx.rest('POST', '/node/claim', undefined, { as: { bearer: d.token } })).body as { token: string }
    ctx.norm.learn(claimed.token, 'laptop-3-claimed-token')
    await ctx.rest('POST', '/node/claim', undefined, { as: { bearer: d.token } })
    await ctx.refusedUpgrade('pasted token after the swap', '/node', { authorization: `Bearer ${d.token}` })
    // a machine that already connected with its token (an older inyon) keeps it
    await ctx.rest('POST', '/node/claim', undefined, { as: { bearer: a.token } })
    await ctx.rest('POST', '/node/claim', undefined, { as: 'anon' })

    await ctx.step('pairing again: refused while connected, for a machine that is not there, for this hub itself')
    await ctx.rest('POST', '/api/hosts/work-laptop/pair', {})
    await ctx.rest('POST', '/api/hosts/nowhere/pair', {})
    await ctx.rest('POST', '/api/hosts/local/pair', {})

    await ctx.step('a new command for the machine that never connected replaces its token')
    const c = (await ctx.rest('POST', '/api/hosts/work-laptop-2/pair', {})).body as Paired
    ctx.norm.learn(c.token, 'laptop-2-new-token')
    await ctx.refusedUpgrade('old token', '/node', { authorization: `Bearer ${b.token}` })
    await ctx.refusedUpgrade('unknown token', '/node', { authorization: 'Bearer not-a-real-token' })
    await ctx.refusedUpgrade('no token', '/node')
    await ctx.rest('GET', '/node', undefined, { as: { bearer: c.token } })

    await ctx.step('the app removes the laptop: its connection is closed and it is told to stop')
    await ctx.rest('DELETE', '/api/hosts/work-laptop')
    await ctx.nodeClosed('work-laptop', laptop)
    await ctx.refusedUpgrade('removed machine', '/node', { authorization: `Bearer ${a.token}` })
    await ctx.rest('DELETE', '/api/hosts/local')
    await ctx.rest('DELETE', '/api/hosts/work-laptop-2')
    await ctx.rest('GET', '/api/hosts')
  },
} satisfies Scenario
