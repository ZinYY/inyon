import type { AgentKind, ChatInfo, ChatItem, HostConfig, HostState, PaneInfo } from '@inyon/protocol'

// Small builders for transcript items, machines and panes. Each test only spells out what it checks.

let seq = 0
const base = (ts?: string) => ({ seq: ++seq, uuid: `u${seq}`, ts: ts ?? new Date(Date.UTC(2026, 9, 11, 12, 0, seq)).toISOString() })

export const item = {
  user: (text: string, more: Partial<Extract<ChatItem, { kind: 'user' }>> = {}): ChatItem => ({ kind: 'user', ...base(), text, images: 0, ...more }),
  text: (text: string): ChatItem => ({ kind: 'assistant', ...base(), msgId: 'm', block: { type: 'text', text } }),
  thinking: (text: string): ChatItem => ({ kind: 'assistant', ...base(), msgId: 'm', block: { type: 'thinking', text } }),
  tool: (id: string, name = 'Bash', input: any = { command: 'ls' }): ChatItem => ({ kind: 'assistant', ...base(), msgId: 'm', block: { type: 'tool_use', id, name, input } }),
  result: (toolUseId: string, text = 'ok'): ChatItem => ({ kind: 'tool_result', ...base(), toolUseId, isError: false, text }),
  end: (durationMs = 1000): ChatItem => ({ kind: 'turn_end', ...base(), durationMs }),
  command: (name: string, args = '', output?: string, ts?: string): ChatItem => ({ kind: 'command', ...base(ts), name, args, output }),
  notice: (subtype: string, text = subtype, level: 'info' | 'warn' | 'error' = 'info'): ChatItem => ({ kind: 'notice', ...base(), subtype, text, level }),
  goal: (condition: string, more: { met?: boolean; reason?: string; cleared?: boolean } = {}): ChatItem => ({ kind: 'goal', ...base(), condition, met: false, ...more }),
}

export function pane(id: string, more: Partial<PaneInfo> = {}): PaneInfo {
  return { id, index: 0, active: true, command: 'zsh', cwd: '/home/me/code', title: '', width: 80, height: 24, dead: false, ...more }
}

export function agentPane(id: string, kind: AgentKind, more: Partial<PaneInfo> = {}): PaneInfo {
  return pane(id, { command: kind, agent: { kind, pid: 1, state: 'idle' }, ...more })
}

/** A machine with one tmux session holding the given panes, one window each. */
export function host(id: string, more: Partial<HostState> & { config?: Partial<HostConfig>; panes?: PaneInfo[] } = {}): HostState {
  const { config, panes = [], ...rest } = more
  return {
    config: { id, name: id, kind: 'ssh', target: `me@${id}`, ...config },
    status: 'online',
    integration: 'installed',
    tmux: 'ok',
    agents: ['claude'],
    updatedAt: 0,
    sessions: [{ id: '$1', name: 'main', attached: 0, windows: panes.map((p, i) => ({ id: `@${i}`, index: i, name: '', active: i === 0, activity: 0, panes: [p] })) }],
    ...rest,
  }
}

export function chatInfo(more: Partial<ChatInfo> = {}): ChatInfo {
  return {
    hostId: 'box',
    paneId: '%1',
    running: true,
    queue: [],
    tasks: [],
    subagents: [],
    artifacts: [],
    stats: { turns: 0, compactions: 0 },
    ...more,
  }
}

/** A deep copy: equal in every field, but every object is new, as after a poll. */
export const fresh = <T,>(v: T): T => structuredClone(v)
