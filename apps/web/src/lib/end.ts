import { agentDef } from '@inyon/protocol'
import { api } from './api.ts'
import { hidePane, toast } from './store.ts'
import { haptic } from './haptics.ts'
import { paneTitle, terminalName, type PaneRef } from './overview.ts'
import { tmuxTarget } from './groups.ts'
import { confirmAction } from '../components/Confirm.tsx'

/** What the confirm sheet says ending this pane will do. */
export function endText(r: PaneRef) {
  const agent = r.pane.agent && agentDef(r.pane.agent.kind)?.name
  const also =
    r.window.panes.length > 1
      ? ''
      : r.session.windows.length > 1
        ? `, and window ${r.session.name}:${r.window.index} with it, since nothing else is in it`
        : `, and the tmux session “${r.session.name}” with it, since nothing else is in it`
  return `${agent ? `${agent} exits, then its` : 'Its'} tmux pane closes${also}. Your other windows stay as they are.`
}

/**
 * Asks, then ends a session: it leaves every list at once with a notice, while the machine stops
 * the agent and closes its pane. A failure brings it back with the reason.
 */
export async function endSession(r: PaneRef, after?: () => void) {
  const host = r.host.config.id
  const ok = await confirmAction({
    title: 'End this session?',
    details: [
      ['Session', r.pane.agent ? paneTitle(r) : (terminalName(r) ?? r.pane.command)],
      ['Machine', r.host.config.name],
      ['tmux', tmuxTarget(r)],
    ],
    body: endText(r),
    yes: 'End session',
    danger: true,
  })
  if (!ok) return false
  hidePane(host, r.pane.id)
  haptic('success')
  toast({ title: 'Session ended', tone: 'success' })
  after?.()
  try {
    await api.end(host, r.pane.id)
  } catch (e) {
    hidePane(host, r.pane.id, false)
    toast({ title: 'Could not end the session', body: (e as Error).message, tone: 'error' })
  }
  return true
}
