import { useMemo } from 'react'
import { agentDef, AGENTS, chatKey, type AgentKind, type AgentStatus } from '@inyon/protocol'
import { api } from '../../lib/api.ts'
import { toast, useStore } from '../../lib/store.ts'
import { allPanes } from '../../lib/overview.ts'
import { copyText } from '../../lib/clipboard.ts'
import { useAgentCatalog } from './useAgentCatalog.ts'
import { confirmAction } from '../Confirm.tsx'
import { endSession } from '../../lib/end.ts'

/** Everything a session view needs: pane ref, live status, transcript summary, and actions. */
export function useSession(host: string, pane: string) {
  const hosts = useStore((s) => s.hosts)
  const chat = useStore((s) => s.chats[chatKey(host, pane)])
  const ref = useMemo(() => allPanes(hosts).find((r) => r.host.config.id === host && r.pane.id === pane), [hosts, host, pane])
  const agent: AgentKind | undefined = ref?.pane.agent?.kind ?? chat?.info?.agent
  const catalog = useAgentCatalog(host, pane, agent)
  // the machine's own lists when the agent has them: OpenCode's models and agents (custom ones too)
  const def = useMemo(() => agent ? { ...agentDef(agent), models: catalog?.models ?? agentDef(agent).models, modes: catalog?.modes ?? agentDef(agent).modes } : undefined, [agent, catalog])
  const info = chat?.info
  const live = ref?.pane.agent?.status
  const reported: AgentStatus | undefined = agent === 'copilot'
    ? info?.status ?? live
    : agent === 'opencode'
      ? openCodeStatus(info?.status, live)
      : live
  // a session that isn't reporting still has a known model, and so a known window size
  const selectedModel = def?.models.find((m) => m.id === currentModelId(def, reported, info?.model))
  const status = useMemo(() => {
    if (!reported) return reported
    const size = !reported.contextSize && selectedModel?.context
    // "default" on the footer, or no level at all from a session that reports, means the model's own
    // default level (Claude Code names one only once you pick it): show that rather than "Default"
    const effort = (!reported.effort || reported.effort === 'default') && selectedModel?.defaultEffort
    return size || effort ? { ...reported, ...(size && { contextSize: size }), ...(effort && { effort }) } : reported
  }, [reported, selectedModel])
  const screen = chat?.screen
  const efforts = def?.set.effort || selectedModel?.variants ? selectedModel?.efforts ?? def?.efforts ?? [] : []
  // the switch as the screen shows it, else as the hub last read it
  const ultracode = screen?.ultracode ?? ref?.pane.agent?.ultracode ?? info?.ultracode

  const selectModel = (id: string) => {
    const model = def?.models.find((m) => m.id === id)
    const variants = model?.variants
    const variant = variants?.[status?.effort ?? ''] ?? Object.values(variants ?? {})[0]
    return def?.set.model && setting(def.set.model(variant ?? id, status?.effort), 'Could not switch model')
  }

  const run = async (label: string, fn: () => Promise<unknown>) => {
    try {
      await fn()
    } catch (e) {
      toast({ title: label, body: (e as Error).message, tone: 'error' })
    }
  }
  const send = (text: string, label = 'Could not send') => run(label, () => api.send(host, pane, text))
  // a setting goes to the agent as a slash command, but the chat doesn't show it as a message
  const setting = (text: string, label: string) => run(label, () => api.setting(host, pane, text))

  const actions = {
    send,
    setModel: selectModel,
    setEffort: (level: string) => selectedModel?.variants?.[level]
      ? setting(`/model ${selectedModel.variants[level]}`, 'Could not change effort')
      : def?.set.effort && setting(def.set.effort(level), 'Could not change effort'),
    setMode: (mode: string) => run('Could not switch mode', () => api.mode(host, pane, mode)),
    /** forgets a model, effort or mode picked while the agent couldn't take it */
    dropChosen: () => run('Could not cancel the switch', () => api.dropChosen(host, pane)),
    setGoal: (text: string) => def?.set.goal && setting(def.set.goal(text), 'Could not set goal'),
    clearGoal: () => def?.set.clearGoal && setting(def.set.clearGoal, 'Could not clear goal'),
    compact: () => def?.set.compact && send(def.set.compact, 'Could not compact'),
    clear: () => def?.set.clear && send(def.set.clear, 'Could not clear'),
    rc: (on: boolean) =>
      run(on ? 'Could not turn on Remote Control' : 'Could not turn off Remote Control', async () => {
        await api.rc(host, pane, on)
        toast({ title: on ? 'Remote Control on' : 'Remote Control off', tone: 'success' })
      }),
    ultracode: (on: boolean) =>
      run(on ? 'Could not turn on Ultracode' : 'Could not turn off Ultracode', async () => {
        await api.ultracode(host, pane, on)
        toast({ title: on ? 'Ultracode on' : 'Ultracode off', body: on ? 'A dynamic workflow for every task, in this session.' : undefined, tone: 'success' })
      }),
    rewind: async () => {
      if (agent === 'claude' || agent === 'antigravity') return send('/rewind')
      if (agent === 'opencode') {
        if (!(await confirmAction({ title: 'Undo your last message?', body: 'OpenCode takes it back with its file changes. Redo puts it back.', yes: 'Undo', danger: true }))) return
        return send('/undo', 'Could not undo')
      }
      if (agent !== 'codex' || !(await confirmAction({ title: 'Rewind your last message?', body: 'Codex forgets it and its reply. To go further back, use the rewind button under any message.', yes: 'Rewind', danger: true }))) return
      return run('Could not rewind', async () => {
        await api.rewind(host, pane)
        toast({ title: 'Conversation rewound', tone: 'success' })
      })
    },
    /** OpenCode: puts back what Undo took away */
    redo: () => send('/redo', 'Could not redo'),
    /** OpenCode: a public link to the conversation, or taking it back */
    share: async (on: boolean) => {
      if (on && !(await confirmAction({ title: 'Share this conversation?', body: 'OpenCode puts it at a public link that anyone who has it can open. Stop sharing takes it down.', yes: 'Share' }))) return
      return run(on ? 'Could not share' : 'Could not stop sharing', async () => {
        await api.send(host, pane, on ? '/share' : '/unshare')
        toast({ title: on ? 'Shared' : 'No longer shared', body: on ? 'The link shows under Usage in a moment.' : undefined, tone: 'success' })
      })
    },
    interrupt: () => run('Could not interrupt', () => api.interrupt(host, pane)),
    /** ends the session for good: the agent exits and its tmux pane closes */
    end: (after?: () => void) => ref && endSession(ref, after),
    exit: async () => {
      if (!def || !(await confirmAction({ title: `Exit ${def.name} to the shell?`, body: 'The conversation is saved, and the pane stays open at its shell. To close the pane too, use End session.', yes: 'Exit' }))) return
      return run(`Could not exit ${def.name}`, async () => {
        await api.exit(host, pane)
        toast({ title: `${def.name} exited`, tone: 'success' })
      })
    },
    /** Claude Code, Codex and OpenCode can carry on a copy of the conversation in a new window */
    canFork: (agent === 'claude' || agent === 'codex' || agent === 'opencode') && !!(info?.sessionId ?? ref?.pane.agent?.sessionId),
    fork: (go: (pane: string) => void) =>
      run('Could not fork', async () => {
        const r = await api.fork(host, pane)
        toast({ title: 'Forked into a new window', body: 'Both conversations carry on separately from here.', tone: 'success' })
        go(r.pane)
      }),
    copyId: async () => {
      const id = info?.sessionId ?? ref?.pane.agent?.sessionId
      if (!id) return
      if (await copyText(id)) toast({ title: 'Session ID copied', tone: 'success' })
      else toast({ title: 'Session ID', body: id, tone: 'info' })
    },
  }

  return { ref, agent, def, status, info, screen, chat, actions, efforts, catalog, ultracode }
}

export type SessionModel = ReturnType<typeof useSession>

/**
 * OpenCode's numbers come from its record (the conversation), but the model, variant and agent
 * are whatever its terminal footer shows, which the pane reports (and keeps while a menu hides
 * it): a pick made in the terminal or in the app shows here the moment the footer changes. The
 * footer's own context and cost fill in when the record has none yet.
 */
export function openCodeStatus(record?: AgentStatus, live?: AgentStatus): AgentStatus | undefined {
  if (!record && !live) return undefined
  const out: AgentStatus = { ...(record ?? { updatedAt: 0 }) }
  if (live?.model) {
    out.model = live.model
    out.modelId = live.modelId
    out.effort = live.effort
  }
  if (live?.contextSize) out.contextSize = live.contextSize
  if (out.contextUsed === undefined) {
    if (live?.contextUsed !== undefined) out.contextUsed = live.contextUsed
    if (live?.contextPct !== undefined) out.contextPct = live.contextPct
  }
  if (out.costUsd === undefined && live?.costUsd !== undefined) out.costUsd = live.costUsd
  out.updatedAt = Math.max(out.updatedAt, live?.updatedAt ?? 0)
  return out
}

/** Claude reports "Opus 5.5"; map to the alias we send. An exact id (OpenCode's "provider/model") wins over any name. */
export function currentModelId(def: (typeof AGENTS)[AgentKind] | undefined, status?: AgentStatus, model?: string) {
  if (!def) return undefined
  if (status?.modelId && def.models.some((m) => m.id === status.modelId)) return status.modelId
  const label = (status?.model ?? model ?? '').toLowerCase()
  const exact = label && def.models.find((m) => m.label.toLowerCase() === label || m.label.split(' · ').pop()!.toLowerCase() === label)
  if (exact) return exact.id
  // the longest match, so "gpt-5" never stands in for "gpt-5.1-codex"
  return [...def.models]
    .sort((a, b) => b.id.length - a.id.length)
    .find((m) => (label && (label.includes(m.label.toLowerCase()) || label.includes(m.id.toLowerCase()))) || (status?.modelId ?? '').includes(m.id))?.id
}
