import { promptKey, type ScreenState } from '@inyon/protocol'
import type { Claude } from '../lib/agents.ts'
import { claudeBox, KEY, online, P, PANE } from '../lib/claude-box.ts'
import type { Msg } from '../lib/record.ts'
import { BOX, type Scenario } from '../lib/scenario.ts'
import { agentState } from '../lib/wait.ts'

let claude: Claude

/** a chat.screen with a prompt on it, as the app sees it */
const prompted = (m: Msg) => m.t === 'chat.screen' && m.key === KEY && !!(m.screen as ScreenState).prompt
const keyOf = (m: Msg | undefined) => (m ? promptKey((m.screen as ScreenState).prompt!) : '')

export default {
  name: 'claude-approval',
  title: 'Claude Code asks: a permission and a question card answered from the app, a stale answer, a send while it waits',
  machine: (m) =>
    void (claude = claudeBox(m, {
      // Claude takes a number on its prompts and gets back to work
      onKey: (k, tui) => {
        if (!('key' in k) || !/^[1-9]$/.test(k.key) || (tui.view !== 'permission' && tui.view !== 'question')) return
        tui.show('working')
        return true
      },
    })),
  async run(ctx) {
    const app = await online(ctx)
    app.send({ t: 'sub.chat', host: BOX, pane: PANE })
    await app.until('chat.screen', (m) => m.t === 'chat.screen' && m.key === KEY)

    await ctx.step('Claude asks to run a command: the machine is waiting and the app is told')
    claude.took('Write hello-inyon to out.txt', 20)
    claude.tui.show('permission')
    const permission = await app.until('permission on screen', prompted)
    await app.until('waiting', agentState(BOX, PANE, 'waiting'))
    await app.until('waiting notice', (m) => m.t === 'notify' && m.kind === 'waiting')

    await ctx.step('a message sent while the prompt is up')
    await ctx.rest('POST', `${P}/send`, { text: 'hello?', requestId: 'while-asking' })

    await ctx.step('an answer meant for another question is refused')
    await ctx.rest('POST', `${P}/answer`, { option: 1, question: 'Some other question?1.Yes2.No' })

    await ctx.step('the app answers Yes: Claude carries on')
    await ctx.rest('POST', `${P}/answer`, { option: 1, question: keyOf(permission) })
    await app.until('working', agentState(BOX, PANE, 'working'))

    await ctx.step('Claude finishes the turn')
    claude.replied('Wrote out.txt.', 30)
    await app.until('done notice', (m) => m.t === 'notify' && m.kind === 'done')
    await app.until('idle', agentState(BOX, PANE, 'idle'))

    await ctx.step('an answer once nothing is asked')
    await ctx.rest('POST', `${P}/answer`, { option: 1 })

    await ctx.step('Claude asks a question with choices')
    claude.took('Ask me which colour I like', 40)
    claude.tui.show('question')
    const question = await app.until('question on screen', prompted)
    await app.until('waiting', agentState(BOX, PANE, 'waiting'))

    await ctx.step('a choice that is not on the card')
    await ctx.rest('POST', `${P}/answer`, { option: 9, question: keyOf(question) })

    await ctx.step('the app picks Green')
    await ctx.rest('POST', `${P}/answer`, { option: 2, question: keyOf(question) })
    await app.until('working', agentState(BOX, PANE, 'working'))
    claude.replied('Green it is.', 50)
    await app.until('idle', agentState(BOX, PANE, 'idle'))
  },
} satisfies Scenario
