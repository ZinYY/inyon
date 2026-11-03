import { describe, expect, test } from 'bun:test'
import type { AgentState, HostState } from '@inyon/protocol'
import { agentPanes, allPanes, paneHref, paneTitle, terminalName, type PaneRef } from './overview.ts'
import { agentPane, host, pane } from '../test/fixtures.ts'

/** One pane in its own window, for paneTitle and terminalName. */
const ref = (p = pane('%1'), windowName = '', h: Partial<HostState> = {}): PaneRef => {
  const hs = host('box', { panes: [p], ...h })
  const r = allPanes([hs])[0]
  r.window.name = windowName
  return r
}

describe('allPanes', () => {
  test('flattens every machine, session and window', () => {
    const a = host('a', { panes: [pane('%1'), pane('%2')] })
    const b = host('b', { panes: [pane('%7')] })
    expect(allPanes([a, b]).map((r) => `${r.host.config.id}${r.pane.id}`)).toEqual(['a%1', 'a%2', 'b%7'])
    expect(allPanes([])).toEqual([])
  })
})

describe('agentPanes', () => {
  test('only agents, the ones that need you first, then the most recently active', () => {
    const p = (id: string, state: AgentState) => agentPane(id, 'claude', { agent: { kind: 'claude', pid: 1, state } })
    const h = host('a', { panes: [pane('%0'), p('%1', 'idle'), p('%2', 'working'), p('%3', 'waiting'), p('%4', 'unknown'), p('%5', 'idle')] })
    const windows = h.sessions[0].windows
    windows[1].activity = 10
    windows[5].activity = 20
    expect(agentPanes([h]).map((r) => r.pane.id)).toEqual(['%3', '%2', '%5', '%1', '%4'])
  })
})

test('paneHref', () => {
  expect(paneHref('my box', '%12')).toBe('/m/my%20box/p/12')
  expect(paneHref('a/b', '%3', 'terminal')).toBe('/m/a%2Fb/p/3?view=terminal')
})

describe('paneTitle', () => {
  test('a name you gave it wins', () => {
    expect(paneTitle(ref(agentPane('%1', 'claude', { label: 'Mine', title: '✳ Fix login' })))).toBe('Mine')
  })

  test("the agent's terminal title, without its spinner mark", () => {
    expect(paneTitle(ref(agentPane('%1', 'claude', { title: '✳ Fix login bug' })))).toBe('Fix login bug')
    expect(paneTitle(ref(agentPane('%1', 'claude', { title: '⠂ Refactor store' })))).toBe('Refactor store')
  })

  test("titles that only name the agent or the machine don't count", () => {
    expect(paneTitle(ref(agentPane('%1', 'claude', { title: '✳ Claude Code' }), 'work'))).toBe('work')
    expect(paneTitle(ref(agentPane('%1', 'codex', { title: 'codex' }), 'api'))).toBe('api')
    expect(paneTitle(ref(agentPane('%1', 'claude', { title: 'box.local' }), 'api', { hostname: 'box.local' }))).toBe('api')
  })

  test("an agent in a window tmux named itself shows the agent's name", () => {
    expect(paneTitle(ref(agentPane('%1', 'claude'), ''))).toBe('Claude Code')
    expect(paneTitle(ref(agentPane('%1', 'claude'), 'zsh'))).toBe('Claude Code')
    expect(paneTitle(ref(agentPane('%1', 'codex'), 'powershell'))).toBe('Codex')
    expect(paneTitle(ref(agentPane('%1', 'claude', { command: 'node' }), 'node'))).toBe('Claude Code')
    // your own agent shows its own name
    expect(paneTitle(ref(agentPane('%1', 'claude', { agent: { kind: 'claude', custom: 'OpenClaude', pid: 1, state: 'idle' } }), 'sh'))).toBe('OpenClaude')
  })

  test('a plain terminal shows its window name, else its command', () => {
    expect(paneTitle(ref(pane('%1', { command: 'vim', title: 'vim notes.md' }), 'notes'))).toBe('notes')
    expect(paneTitle(ref(pane('%1', { command: 'htop' }), ''))).toBe('htop')
  })
})

test('terminalName is only a name someone chose', () => {
  expect(terminalName(ref(pane('%1', { command: 'zsh' }), 'zsh'))).toBeUndefined()
  expect(terminalName(ref(pane('%1', { command: 'zsh' }), ''))).toBeUndefined()
  expect(terminalName(ref(pane('%1', { command: 'zsh' }), 'logs'))).toBe('logs')
})
