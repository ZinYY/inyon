import { promptKey, type ScreenState } from '@inyon/protocol'
import type { Claude } from '../lib/agents.ts'
import { claudeBox, KEY, online, P, PANE } from '../lib/claude-box.ts'
import type { Msg } from '../lib/record.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'

let claude: Claude

const screenWith = (pred: (s: ScreenState) => boolean) => (m: Msg) => m.t === 'chat.screen' && m.key === KEY && pred(m.screen as ScreenState)

export default {
  name: 'claude-model',
  title: "Changing Claude Code's model and effort from the app's controls, with the confirmation Claude asks for",
  machine: (m) =>
    void (claude = claudeBox(m, {
      // the slash commands the app's controls send, as Claude takes them
      onSubmit: (text) => {
        if (text === '/model sonnet') claude.tui.show('switch-model')
        else if (text === '/effort low') claude.tui.show('sonnet-low')
        else claude.took(text, 20)
      },
      onKey: (k, tui) => {
        if (tui.view !== 'switch-model' || !('key' in k) || k.key !== '1') return
        tui.show('sonnet')
        return true
      },
    })),
  async run(ctx) {
    const app = await online(ctx)
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)
    await ctx.rest('GET', `${P}/models`)

    await ctx.step('the app picks Sonnet: a setting, not a message; Claude asks to confirm')
    await ctx.rest('POST', `${P}/send`, { text: '/model sonnet', setting: true })
    const ask = await app.until('switch prompt', screenWith((s) => !!s.prompt))

    await ctx.step('the app confirms the switch')
    await ctx.rest('POST', `${P}/answer`, { option: 1, question: ask ? promptKey((ask.screen as ScreenState).prompt!) : '' })
    // Claude's screen doesn't name the model (its footer is the user's own status line): the prompt going away is what shows
    await app.until('prompt gone', screenWith((s) => !s.prompt && !!s.inputVisible))

    await ctx.step('the app lowers the effort')
    await ctx.rest('POST', `${P}/send`, { text: '/effort low', setting: true })
    await app.until('low effort', screenWith((s) => s.effort === 'low'))
    await ctx.rest('GET', `${P}/models`)
  },
} satisfies Scenario
