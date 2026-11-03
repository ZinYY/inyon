import { claudeBox, online, PANE } from '../lib/claude-box.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'

export default {
  name: 'terminal',
  title: "The terminal: attach to a pane, type, resize, close, a pane that isn't there, the machine's side ending it",
  machine: (m) => void claudeBox(m),
  async run(ctx) {
    const app = await online(ctx)

    await ctx.step('the app opens a terminal on the pane: it gets the screen')
    app.send({ t: 'term.open', id: 't1', host: BOX, pane: PANE, cols: 80, rows: 24 })
    await app.until('first bytes', (m) => m.t === 'term.data' && m.id === 't1')

    await ctx.step('typing goes to the machine')
    app.send({ t: 'term.in', id: 't1', d: 'ls -la\r' })
    await ctx.untilMachine('typed', () => JSON.stringify(ctx.machine!.actions).includes('ls -la'))

    await ctx.step('the phone turns: resize (and a size too small, which is raised to the minimum)')
    app.send({ t: 'term.resize', id: 't1', cols: 120, rows: 40 })
    app.send({ t: 'term.resize', id: 't1', cols: 2, rows: 1 })
    await ctx.untilMachine('resized', () => JSON.stringify(ctx.machine!.actions).includes('"rows":4'))

    await ctx.step('closing it')
    app.send({ t: 'term.close', id: 't1' })
    await ctx.untilMachine('killed', () => JSON.stringify(ctx.machine!.actions).includes('"pty":"kill"'))

    await ctx.step('input and resize for a terminal that is closed are ignored')
    app.send({ t: 'term.in', id: 't1', d: 'x' })
    app.send({ t: 'term.resize', id: 't1', cols: 90, rows: 30 })

    await ctx.step("a pane or machine that isn't there")
    app.send({ t: 'term.open', id: 't2', host: BOX, pane: '%99', cols: 80, rows: 24 })
    await app.until('t2 exit', (m) => m.t === 'term.exit' && m.id === 't2')
    app.send({ t: 'term.open', id: 't3', host: 'nowhere', pane: PANE, cols: 80, rows: 24 })
    await app.until('t3 exit', (m) => m.t === 'term.exit' && m.id === 't3')

    await ctx.step('the view ends on the machine: the app is told')
    app.send({ t: 'term.open', id: 't4', host: BOX, pane: PANE, cols: 80, rows: 24 })
    await app.until('t4 bytes', (m) => m.t === 'term.data' && m.id === 't4')
    ctx.node!.endTerminals(PANE)
    await app.until('t4 exit', (m) => m.t === 'term.exit' && m.id === 't4')
  },
} satisfies Scenario
