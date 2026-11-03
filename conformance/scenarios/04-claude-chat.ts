import type { Claude } from '../lib/agents.ts'
import { claudeBox, KEY, online, P, PANE } from '../lib/claude-box.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'

let claude: Claude

export default {
  name: 'claude-chat',
  title: 'Opening a Claude Code chat: the transcript, the screen, slash commands, models, past conversations',
  machine: (m) => void (claude = claudeBox(m)),
  async run(ctx) {
    const app = await online(ctx)

    await ctx.step('the app opens the chat')
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.reset', (m) => m.t === 'chat.reset' && m.key === KEY)
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)

    await ctx.step('older messages: there are none')
    app.send({ t: 'chat.more', host: BOX, pane: PANE, before: 0 })

    await ctx.step('slash commands, models and past conversations for the pane')
    await ctx.rest('GET', `${P}/commands`)
    await ctx.rest('GET', `${P}/models`)
    await ctx.rest('GET', `${P}/conversations`)
    await ctx.rest('GET', `${P}/files`)

    await ctx.step('the transcript read directly, and its subagents')
    const transcript = `${ctx.machine!.home}/${claude.file}`
    await ctx.rest('GET', `/api/hosts/${BOX}/transcript?path=${encodeURIComponent(transcript)}`)
    await ctx.rest('GET', `/api/hosts/${BOX}/subagents?transcript=${encodeURIComponent(transcript)}`)

    await ctx.step('the app closes the chat; a second subscription to a pane that does not exist is ignored')
    app.send({ t: 'unsub.chat', host: BOX, pane: PANE })
    app.send({ t: 'sub.chat', host: BOX, pane: '%99' })
    app.send({ t: 'sub.chat', host: 'nope', pane: PANE })
  },
} satisfies Scenario
