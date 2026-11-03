import { describe, expect, test } from 'bun:test'
import type { HostState } from '@inyon/protocol'
import { groupPanes, matchesQuery, tmuxTarget } from './groups.ts'
import { allPanes } from './overview.ts'
import { agentPane, host, pane } from '../test/fixtures.ts'

/** A machine with two tmux sessions: "api" (two windows) and "web" (one window split in two). */
function machine(id: string, more: Partial<HostState> = {}): HostState {
  const h = host(id, more)
  h.sessions = [
    {
      id: '$1', name: 'api', attached: 0,
      windows: [
        { id: '@1', index: 1, name: '', active: true, activity: 100, panes: [agentPane('%1', 'claude', { title: '✳ Fix the login test', cwd: '/srv/api' })] },
        { id: '@2', index: 2, name: 'logs', active: false, activity: 50, panes: [pane('%2', { command: 'tail' })] },
      ],
    },
    {
      id: '$2', name: 'web', attached: 0,
      windows: [{ id: '@3', index: 1, name: '', active: true, activity: 300, panes: [agentPane('%3', 'codex', { title: 'Tidy the CSS', index: 0 }), pane('%4', { index: 1 })] }],
    },
  ]
  return h
}

describe('grouping sessions', () => {
  test('machine, then tmux session, then panes in window order', () => {
    const [g] = groupPanes([machine('box')])
    expect(g.host.config.id).toBe('box')
    // in the order tmux started them, though "web" was active more recently
    expect(g.sessions.map((s) => s.session.name)).toEqual(['api', 'web'])
    expect(g.sessions[0].panes.map((r) => r.pane.id)).toEqual(['%1', '%2'])
    expect(g.panes).toBe(4)
  })

  // rows moved up and down as agents worked and asked, so clicks landed on the wrong session
  test('nothing moves when an agent asks something, works or goes quiet', () => {
    const order = (h: HostState) => groupPanes([h])[0]!.sessions.map((s) => s.session.name)
    const h = machine('box')
    expect(order(h)).toEqual(['api', 'web'])
    h.sessions[1].windows[0].panes[0].agent!.state = 'waiting'
    h.sessions[1].windows[0].activity = 900
    const [g] = groupPanes([h])
    expect(order(h)).toEqual(['api', 'web'])
    expect([g.waiting, g.sessions[1].waiting]).toEqual([1, 1])
    h.sessions[0].windows[0].panes[0].agent!.state = 'working'
    h.sessions[0].windows[0].activity = 1000
    expect(order(h)).toEqual(['api', 'web'])
  })

  test('sessions follow tmux\'s numbering, not their names; machines keep the order they were added in, online or not', () => {
    const h = machine('box')
    h.sessions.reverse()
    h.sessions[0]!.id = '$10'
    h.sessions[1]!.id = '$9'
    expect(groupPanes([h])[0]!.sessions.map((s) => s.session.id)).toEqual(['$9', '$10'])
    const groups = groupPanes([machine('old', { status: 'error' }), machine('new')])
    expect(groups.map((g) => g.host.config.id)).toEqual(['old', 'new'])
  })

  test('a search looks through every machine and session, by title, session, machine or folder', () => {
    const hosts = [machine('box'), machine('laptop')]
    const ids = (q: string) => groupPanes(hosts, { query: q }).flatMap((g) => g.sessions.flatMap((s) => s.panes.map((r) => `${g.key}${r.pane.id}`)))
    expect(ids('login')).toEqual(['box%1', 'laptop%1'])
    expect(ids('laptop css')).toEqual(['laptop%3'])
    expect(ids('api:2')).toEqual(['box%2', 'laptop%2'])
    expect(ids('/srv/api claude')).toEqual(['box%1', 'laptop%1'])
    expect(ids('nothing like this')).toEqual([])
  })

  test('keep picks which panes count, and empty groups disappear', () => {
    const groups = groupPanes([machine('box')], { keep: (r) => r.pane.agent?.kind === 'codex' })
    expect(groups[0].sessions.map((s) => s.session.name)).toEqual(['web'])
  })

  test('tmux target: session:window, and .pane only in a split window', () => {
    const refs = allPanes([machine('box')])
    expect(refs.map(tmuxTarget)).toEqual(['api:1', 'api:2', 'web:1.0', 'web:1.1'])
  })

  test('every word must match, in any order and case', () => {
    expect(matchesQuery('Fix the login test · box', 'LOGIN box')).toBe(true)
    expect(matchesQuery('Fix the login test', 'login laptop')).toBe(false)
    expect(matchesQuery('anything', '   ')).toBe(true)
  })
})
