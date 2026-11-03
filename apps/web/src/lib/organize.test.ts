import { describe, expect, test } from 'bun:test'
import type { Organize, SessionMark } from '@inyon/protocol'
import { allPanes } from './overview.ts'
import { markFor, mergeOrganize, nextWake, parseTags, placeOf, renameOrganize, sendAt, sendsFor, snoozeUntil, sortPanes, timeText } from './organize.ts'
import { agentPane, host } from '../test/fixtures.ts'

const NOW = new Date(2026, 9, 27, 12, 0).getTime()

const mark = (pane: string, more: Partial<SessionMark> = {}): SessionMark => ({ id: `s-${pane}`, host: 'box', pane, live: true, seen: 0, ...more })
const org = (...sessions: SessionMark[]): Organize => ({ v: 1, sessions, projects: [], groups: [], spaces: [] })

const panes = () =>
  allPanes([
    host('box', {
      panes: [
        agentPane('%1', 'claude', { title: 'Fix the login test' }),
        agentPane('%2', 'codex', { title: 'Tidy the CSS' }),
        agentPane('%3', 'claude', { title: 'Write the docs' }),
        agentPane('%4', 'claude', { title: 'Release notes' }),
        agentPane('%5', 'claude', { title: 'Old idea' }),
      ],
    }),
  ])
const ids = (refs: { pane: { id: string } }[]) => refs.map((r) => r.pane.id)

describe('organised sessions', () => {
  test('the pinned come first in their order; parked and snoozed fold away, the soonest back first', () => {
    const o = org(
      mark('%3', { pinned: 1 }),
      mark('%2', { pinned: 0 }),
      mark('%4', { parked: true }),
      mark('%5', { snoozedUntil: NOW + 60_000 }),
      // pinned and snoozed: out of the way until the snooze ends
      mark('%1', { pinned: 2, snoozedUntil: NOW + 30_000 }),
    )
    const s = sortPanes(panes(), o, { now: NOW })
    expect(ids(s.pinned)).toEqual(['%2', '%3'])
    expect(ids(s.sessions)).toEqual([])
    expect(ids(s.folded)).toEqual(['%1', '%5', '%4'])
    // once it's past, it's pinned again, before the hub has even cleared it
    expect(ids(sortPanes(panes(), o, { now: NOW + 31_000 }).pinned)).toEqual(['%2', '%3', '%1'])
    expect(nextWake(o, NOW)).toBe(NOW + 30_000)
  })

  // tmux hands pane ids out again after its server restarts: the hub then keeps the mark, unmatched
  test('a mark the hub no longer matches to a pane is left out', () => {
    const o = org(mark('%1', { pinned: 0, live: false }))
    expect(markFor(o, 'box', '%1')).toBeUndefined()
    expect(placeOf(markFor(o, 'box', '%1'))).toBe('sessions')
    expect(ids(sortPanes(panes(), o, { now: NOW }).sessions)).toEqual(['%1', '%2', '%3', '%4', '%5'])
  })

  test('the search finds sessions by tag and note, wherever they are', () => {
    const o = org(mark('%2', { tags: ['review'] }), mark('%4', { parked: true, note: 'after the launch' }))
    expect(ids(sortPanes(panes(), o, { now: NOW, query: '#review' }).sessions)).toEqual(['%2'])
    expect(ids(sortPanes(panes(), o, { now: NOW, query: 'launch' }).folded)).toEqual(['%4'])
  })

  test("a paired hub's machines carry its id, in the marks and the project folders", () => {
    const theirs: Organize = { ...org(mark('%1', { host: 'laptop' })), projects: [{ id: 'p-1', name: 'Inyon', color: 'sage', roots: [{ host: 'laptop', path: '/code/inyon' }] }] }
    const renamed = renameOrganize(theirs, (id) => `hub:abc:${id}`)
    expect(renamed.sessions[0]!.host).toBe('hub:abc:laptop')
    expect(renamed.projects[0]!.roots[0]!.host).toBe('hub:abc:laptop')
    const both = mergeOrganize([org(mark('%2', { pinned: 0 })), renamed])
    expect(both.sessions.map((m) => m.host)).toEqual(['box', 'hub:abc:laptop'])
    expect(both.projects.map((p) => p.id)).toEqual(['p-1'])
  })

  test("snoozes end by this device's clock: mornings at 9, next week on Monday", () => {
    // a Tuesday at noon
    const tue = new Date(NOW)
    expect(snoozeUntil('hour', tue)).toBe(NOW + 3600_000)
    expect(new Date(snoozeUntil('tomorrow', tue))).toEqual(new Date(2026, 9, 28, 9, 0))
    expect(new Date(snoozeUntil('nextWeek', tue))).toEqual(new Date(2026, 10, 2, 9, 0))
    // from a Monday, the one after
    expect(new Date(snoozeUntil('nextWeek', new Date(2026, 10, 2, 8, 0)))).toEqual(new Date(2026, 10, 9, 9, 0))
  })

  test("messages sent later: the soonest first, a paired hub's renamed, and their times", () => {
    const o: Organize = {
      ...org(),
      sends: [
        { id: 'd-2', host: 'box', pane: '%1', text: 'later', at: NOW + 7200_000 },
        { id: 'd-1', host: 'box', pane: '%1', text: 'sooner', at: NOW + 3600_000 },
        { id: 'd-3', host: 'box', pane: '%2', text: 'elsewhere', at: NOW },
      ],
      sleep: [{ scope: 'all', minutes: 60 }],
    }
    expect(sendsFor(o, 'box', '%1').map((s) => s.id)).toEqual(['d-1', 'd-2'])
    const theirs = renameOrganize(o, (id) => `hub:abc:${id}`)
    expect(sendsFor(theirs, 'hub:abc:box', '%1')).toHaveLength(2)
    const both = mergeOrganize([o, theirs])
    expect(both.sends).toHaveLength(6)
    // each hub's own sleep rules don't mix
    expect(both.sleep).toBeUndefined()
    const tue = new Date(NOW)
    expect(sendAt('halfHour', tue)).toBe(NOW + 30 * 60_000)
    expect(new Date(sendAt('tomorrow', tue))).toEqual(new Date(2026, 9, 28, 9, 0))
  })

  test('a time within the week reads by its weekday, one further off by its date', () => {
    const now = new Date(2026, 9, 27, 10, 0)
    const weekday = (d: Date) => d.toLocaleDateString([], { weekday: 'short' })
    const date = (d: Date) => d.toLocaleDateString([], { day: 'numeric', month: 'short' })
    const today = new Date(2026, 9, 27, 15, 0)
    expect(timeText(today.getTime(), now)).toBe(today.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }))
    const inThree = new Date(2026, 9, 30, 9, 0)
    expect(timeText(inThree.getTime(), now)).toContain(weekday(inThree))
    const inTen = new Date(2026, 10, 6, 9, 0)
    expect(timeText(inTen.getTime(), now)).toContain(date(inTen))
    expect(timeText(inTen.getTime(), now)).not.toContain(weekday(inTen))
  })


  test('tags are typed in one box and come out as the hub keeps them', () => {
    expect(parseTags(' Review, #needs   docs,, review ')).toEqual(['review', 'needs docs'])
    expect(parseTags('')).toEqual([])
  })
})
