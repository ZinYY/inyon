import path from 'node:path'
import { Claude, claudeHistory, claudeScreen, screen, type ClaudeOptions } from './agents.ts'
import type { Context } from './context.ts'
import type { Machine } from './machine.ts'
import { BOX } from './scenario.ts'
import { onlineWith } from './wait.ts'

/** The Claude session most scenarios use: pane %1 on "box", with a short conversation behind it. */
export const SESSION = '0c0ffee0-1111-4222-8333-444455556666'
export const PANE = '%1'
export const KEY = `${BOX}:${PANE}`
/** the pane in URLs ("%1" travels as "1") */
export const P = `/api/hosts/${BOX}/panes/1`

/** Claude's screens for the scenarios: its prompts from the fixtures, and the footer after a model or effort change. */
export const CLAUDE_SCREENS = {
  permission: () => screen('claude', 'permission-prompt'),
  question: () => screen('claude', 'question-single'),
  'switch-model': () => screen('claude', 'switch-model'),
  sonnet: (d: string) => claudeScreen(d).map((l) => l.replace('Opus 5.5 · high', 'Sonnet 5.5 · high')),
  /** Sonnet, then low effort */
  'sonnet-low': (d: string) => claudeScreen(d).map((l) => l.replace('Opus 5.5 · high', 'Sonnet 5.5 · low').replace('● high · /effort', '● low · /effort')),
}

export function claudeBox(m: Machine, o: { onSubmit?: (text: string) => void; onKey?: ClaudeOptions['onKey'] } = {}) {
  const cwd = path.join(m.home, 'code/inyon')
  const claude: Claude = new Claude(m, {
    id: PANE,
    sessionId: SESSION,
    cwd,
    pid: 4100001,
    history: claudeHistory(cwd, SESSION),
    screens: CLAUDE_SCREENS,
    onSubmit: (text) => (o.onSubmit ? o.onSubmit(text) : claude.took(text, 20)),
    onKey: o.onKey,
  })
  return claude
}

/** Signs in, connects the machine and an app watching the overview, until the machine shows `panes` panes. */
export async function online(ctx: Context, panes = 1) {
  await ctx.signIn()
  await ctx.step('the machine connects; the app watches the overview')
  await ctx.connectNode()
  const app = await ctx.connect()
  app.send({ t: 'sub.overview' })
  await app.until(`${BOX} online with ${panes} panes`, onlineWith(BOX, panes))
  return app
}
