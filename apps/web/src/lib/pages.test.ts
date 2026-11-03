import { beforeEach, describe, expect, test } from 'bun:test'
import type { PageEntry } from '@inyon/protocol'
import {
  RENEW_BEFORE_MS, agoText, appOrigins, changedAt, fitScale, folderKey, frameMessage, frameSrc, groupName, groupPages, isBridged, isChanged, leftText, linkedPages, loadPages, localDeadline, markSeen, matchesPage,
  pageHosts, pageId, previewRefusal, projectName, renewIn, runningNames, sessionPages, shouldRenew, usePages, type Page,
} from './pages.ts'
import { api } from '../test/fakes.ts'
import { ApiError } from './api.ts'

const entry = (slug: string, more: Partial<PageEntry> = {}): Page => {
  const e: PageEntry = { host: 'local', slug, name: slug, project: '/home/me/code/app', by: 'claude', added: 1_000, folder: `/home/me/code/app/private/plans/${slug}`, ...more }
  return { ...e, id: pageId(e.host, e.slug) }
}
const names: Record<string, string> = { local: 'studio', 'hub:h1/local': 'arm', laptop: 'air' }
const nameOf = (h: string) => names[h] ?? h

beforeEach(() => usePages.setState({ pages: null, outdated: [], error: undefined, seen: {}, unreachable: [], forgotten: [] }))

describe('grouping', () => {
  test('machine, then project, then page; the hub machine first, the newest change on top', () => {
    const groups = groupPages(
      [
        entry('old', { added: 10 }),
        entry('win', { host: 'laptop', project: 'C:\\demo', added: 5 }),
        entry('busy', { added: 1, progress: { done: 1, total: 2, updated: new Date(50).toISOString() } }),
        entry('site', { project: '/home/me/code/site', added: 20 }),
      ],
      nameOf,
    )
    expect(groups.map((g) => g.host)).toEqual(['local', 'laptop'])
    expect(groups[0]!.projects.map((p) => [p.name, p.pages.map((x) => x.slug)])).toEqual([
      ['app', ['busy', 'old']],
      ['site', ['site']],
    ])
    // a Windows path names its project by its last folder too
    expect(groups[1]!.projects[0]!.name).toBe('demo')
  })

  test("other machines follow by name, and a paired hub's own machine counts as a hub machine", () => {
    const groups = groupPages([entry('a', { host: 'zeta' }), entry('b', { host: 'hub:h1/local' }), entry('c', { host: 'alpha' })], nameOf)
    expect(groups.map((g) => g.host)).toEqual(['hub:h1/local', 'alpha', 'zeta'])
  })

  test('the filter looks at the name, the project and the machine', () => {
    const p = entry('plan', { name: 'Inyon plan', project: '/srv/custom-tmux' })
    expect(matchesPage(p, 'studio', 'inyon')).toBe(true)
    expect(matchesPage(p, 'studio', 'CUSTOM')).toBe(true)
    expect(matchesPage(p, 'studio', 'stud')).toBe(true)
    expect(matchesPage(p, 'studio', 'handy')).toBe(false)
    expect(matchesPage(p, 'studio', '  ')).toBe(true)
    expect(projectName('/a/b/c/')).toBe('c')
  })

  test("a session's pages are grouped under the name the session has now", () => {
    const pages = [entry('plan', { session: 'chat-1', sessionName: 'Handy redesign' }), entry('mock', { session: 'chat-1' }), entry('other'), entry('gone', { session: 'chat-9' })]
    // renamed while it runs: the group follows the running session's name
    const running = new Map([[pageId('local', 'chat-1'), 'Legato development']])
    const groups = groupPages(pages, nameOf, running)
    expect(groups[0]!.projects.map((p) => [p.name, p.pages.map((x) => x.slug).sort()])).toEqual([
      ['app', ['gone', 'other']],
      ['Legato development', ['mock', 'plan']],
    ])
    // not running: the name Inyon last gave it, else the project folder
    expect(groupName(pages[0]!)).toBe('Handy redesign')
    expect(groupName(pages[1]!)).toBe('app')
    expect(matchesPage(pages[1]!, 'studio', 'legato', running)).toBe(true)
    expect(matchesPage(pages[2]!, 'studio', 'legato', running)).toBe(false)
  })

  test("running sessions name their pages by the title they show, not the agent's own name", () => {
    const pane = (id: string, title: string, sessionId?: string) => ({
      id, index: 0, active: true, command: 'claude', cwd: '/x', title, width: 80, height: 24, dead: false, agent: { kind: 'claude' as const, pid: 1, state: 'idle' as const, sessionId },
    })
    const host = { config: { id: 'local', name: 'studio', kind: 'local' }, hostname: 'studio', status: 'online', sessions: [{ id: '$1', name: 's', attached: 0, windows: [{ id: '@1', index: 0, name: 'claude', active: true, activity: 0, panes: [pane('%1', '✳ Legato development', 'chat-1'), pane('%2', 'Claude Code', 'chat-2'), pane('%3', '✳ No id')] }] }] }
    expect([...runningNames([host as never])]).toEqual([['local/chat-1', 'Legato development']])
  })
})

describe("a session's pages", () => {
  const plan = entry('plan', { added: 5 })
  const site = entry('site', { project: '/home/me/code/site', folder: '/home/me/code/site/docs', added: 9 })
  const scratch = entry('scratch', { project: '/home/me/scratch/x', folder: '/home/me/scratch/x' })
  const win = entry('win', { host: 'laptop', project: 'C:\\Dev\\Robot', folder: 'C:\\Dev\\Robot\\plans' })
  const all = [plan, site, scratch, win]
  test("the session's project, from anywhere inside it", () => {
    expect(sessionPages(all, 'local', '/home/me/code/app').map((p) => p.slug)).toEqual(['plan'])
    expect(sessionPages(all, 'local', '/home/me/code/app/apps/web/').map((p) => p.slug)).toEqual(['plan'])
    // not a folder that only starts the same
    expect(sessionPages(all, 'local', '/home/me/code/application')).toEqual([])
  })
  test('pages in folders under the session, but not every page under a home folder', () => {
    expect(sessionPages(all, 'local', '/home/me/code').map((p) => p.slug)).toEqual(['site', 'plan'])
    expect(sessionPages(all, 'local', '/home/me')).toEqual([])
    expect(sessionPages(all, 'local', '/')).toEqual([])
  })
  test('the same machine only, Windows paths in any case and slash', () => {
    expect(sessionPages(all, 'laptop', 'c:/dev/robot').map((p) => p.slug)).toEqual(['win'])
    expect(sessionPages(all, 'laptop', '/home/me/code/app')).toEqual([])
    expect(sessionPages(all, 'local', undefined)).toEqual([])
    expect(folderKey('C:\\Dev\\')).toBe('c:/dev')
    expect(folderKey('/')).toBe('/')
  })
  test('the pages its conversation added, wherever they are', () => {
    const mine = entry('mine', { project: '/srv/elsewhere', folder: '/srv/elsewhere/plan', session: 'chat-1' })
    expect(sessionPages([...all, mine], 'local', '/home/me/code/app', 'chat-1').map((p) => p.slug)).toEqual(['mine', 'plan'])
    expect(sessionPages([...all, mine], 'local', undefined, 'chat-1').map((p) => p.slug)).toEqual(['mine'])
    expect(sessionPages([...all, mine], 'laptop', 'c:/dev/robot', 'chat-1').map((p) => p.slug)).toEqual(['win'])
  })
})

describe('pages a reply links to', () => {
  test('what inyon pages add prints, each once, and the full address', () => {
    const text = 'Added it. Open it in the app at /pages/local/my-plan\nor `/pages/local/my-plan`, and https://hub.example:8080/pages/laptop/robot.'
    expect(linkedPages(text, 'local')).toEqual(['local/my-plan', 'laptop/robot'])
  })
  test("a session through a paired hub names that hub's machines", () => {
    expect(linkedPages('at /pages/local/my-plan', 'hub:h1/laptop')).toEqual(['hub:h1/local/my-plan'])
  })
  test('not a source path that has pages in it', () => {
    expect(linkedPages('see apps/web/src/pages/Pages.tsx and /home/me/pages/local/x', 'local')).toEqual([])
  })
})

describe('changed since it was opened', () => {
  test('a page never opened counts as changed; opening it clears that until its progress moves', () => {
    const p = entry('plan', { added: 100 })
    expect(isChanged(p, {})).toBe(true)
    markSeen(p, 500)
    const seen = usePages.getState().seen
    expect(isChanged(p, seen)).toBe(false)
    expect(JSON.parse(localStorage.getItem('inyon.pages.seen.v1')!)).toEqual({ 'local/plan': 500 })
    const later = { ...p, progress: { done: 2, total: 9, updated: new Date(900).toISOString() } }
    expect(changedAt(later)).toBe(900)
    expect(isChanged(later, seen)).toBe(true)
  })

  test("a progress time that isn't a date falls back to when the page was added", () => {
    expect(changedAt(entry('x', { added: 42, progress: { done: 0, total: 1, updated: 'soon' } }))).toBe(42)
  })
})

describe('links', () => {
  const now = 1_000_000_000
  test('a link is renewed when less than an hour is left, not before', () => {
    expect(RENEW_BEFORE_MS).toBe(3_600_000)
    expect(shouldRenew(now + 12 * 3_600_000, now)).toBe(false)
    expect(shouldRenew(now + 3_600_001, now)).toBe(false)
    expect(shouldRenew(now + 3_599_999, now)).toBe(true)
    expect(shouldRenew(now - 1, now)).toBe(true)
  })

  test("a link's end is counted on this device's clock from the hub's ttl, whatever the hub's clock says", () => {
    // the hub's clock is two hours ahead of this one
    const hubExpires = now + 2 * 3_600_000 + 12 * 3_600_000
    expect(localDeadline({ url: '/v/t/', expires: hubExpires, ttl: 12 * 3_600_000 }, now).until).toBe(now + 12 * 3_600_000)
    // a hub that doesn't say has only its own expiry
    expect(localDeadline({ url: '/v/t/', expires: hubExpires } as never, now).until).toBe(hubExpires)
  })

  test('a link on screen is renewed by a timer an hour before its end, never sooner than 5 s', () => {
    expect(renewIn(now + 12 * 3_600_000, now)).toBe(11 * 3_600_000)
    expect(renewIn(now + 3_600_000 + 1, now)).toBe(5_000)
    expect(renewIn(now - 1, now)).toBe(5_000)
  })

  test("the frame's address keeps the token path's slash, so the theme survives without a redirect", () => {
    expect(frameSrc('', '/v/v1.abc.def/', 'plan', 'dark')).toBe('/v/v1.abc.def/?theme=dark')
    expect(frameSrc('http://box:4777', '/v/t/', 'mockups', 'light')).toBe('http://box:4777/v/t/mockups/?theme=light')
    expect(frameSrc('', '/v/t/', 'matrix', 'light')).toBe('/v/t/matrix.html?theme=light')
  })

  test('words for how long ago and how long is left', () => {
    expect(agoText(now - 10_000, now)).toBe('just now')
    expect(agoText(now - 3 * 60_000, now)).toBe('3 min ago')
    expect(agoText(now - 2 * 3_600_000, now)).toBe('2 h ago')
    expect(agoText(now - 4 * 86_400_000, now)).toBe('4 d ago')
    expect(leftText(now + (11 * 60 + 52) * 60_000, now)).toBe('11 h 52 m')
    expect(leftText(now + 30 * 60_000, now)).toBe('30 m')
    expect(leftText(now - 5, now)).toBe('0 m')
  })

  test('a device frame shrinks to fit the stage and never grows', () => {
    expect(fitScale(390, 844, 2000, 2000)).toBe(1)
    expect(fitScale(1440, 900, 744, 1000)).toBeCloseTo(0.5)
    expect(fitScale(390, 844, 800, 446)).toBeCloseTo(0.5)
  })
})

describe('messages from the frame', () => {
  const frame = {} as Window
  test('only from the page\u2019s own frame, and only a kind the app knows', () => {
    expect(frameMessage({ source: frame, data: { type: 'theme?' } }, frame)).toEqual({ type: 'theme?' })
    // another window, a frame inside the page, or no frame at all
    expect(frameMessage({ source: {} as Window, data: { type: 'theme?' } }, frame)).toBeNull()
    expect(frameMessage({ source: null, data: { type: 'theme?' } }, frame)).toBeNull()
    expect(frameMessage({ source: frame, data: { type: 'theme?' } }, null)).toBeNull()
    // unknown kinds and odd shapes
    for (const data of [{ type: 'navigate', url: '/api/session' }, { type: 'theme' }, 'theme?', null, { type: 7 }, ['theme?']]) expect(frameMessage({ source: frame, data }, frame)).toBeNull()
  })

  test('nothing but the type is passed on', () => {
    expect(frameMessage({ source: frame, data: { type: 'theme?', html: '<script>alert(1)</script>' } }, frame)).toEqual({ type: 'theme?' })
  })
})

describe('the Preview tab', () => {
  test("refuses the app's own address and each paired hub's, whatever the path", () => {
    const own = ['http://localhost:5173', 'http://box:4777']
    expect(previewRefusal('http://localhost:5173/', own)).toMatch(/Inyon itself/)
    expect(previewRefusal('http://localhost:5173/v/v1.a.b/', own)).toMatch(/Inyon itself/)
    expect(previewRefusal('http://box:4777/settings?x=1', own)).toMatch(/Inyon itself/)
    expect(previewRefusal('http://localhost:3000/', own)).toBeNull()
    expect(previewRefusal('https://localhost:5173/', own)).toBeNull()
    expect(previewRefusal('not a url', own)).toBeNull()
  })

  test("the app's origins: this page's and the paired hubs'", () => {
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'arm', url: 'http://box:4777', token: 't', pairedAt: 1 }]))
    expect(appOrigins()).toEqual(['http://localhost:5173', 'http://box:4777'])
  })
})

describe('the list', () => {
  test('in a browser it comes from the hub the app came from', async () => {
    api.pages.mockResolvedValue({ pages: [{ host: 'local', slug: 'plan', name: 'Plan', project: '/p', by: 'cli', added: 1, folder: '/p/plan' }] } as never)
    await loadPages()
    expect(usePages.getState().pages!.map((p) => p.id)).toEqual(['local/plan'])
    expect(pageHosts()).toEqual(['local'])
  })

  test('the phone app with no hub lists the pages of the computers paired with it, from its own core, and names those too old to say', async () => {
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    api.pages.mockResolvedValue({ pages: [{ host: 'laptop', slug: 'plan', name: 'Plan', project: '/p', by: 'cli', added: 1, folder: '/p/plan' }], outdated: ['old-box', 7] } as never)
    try {
      await loadPages()
      expect(usePages.getState()).toMatchObject({ outdated: ['old-box'], error: undefined })
      expect(usePages.getState().pages!.map((p) => p.id)).toEqual(['laptop/plan'])
      expect(api.pages).toHaveBeenCalledTimes(1)
      expect(isBridged('laptop')).toBe(true)
      expect(isBridged('hub:h1/local')).toBe(false)
      // folders can be added on each paired hub's own machine and on each computer that is online
      localStorage.setItem('inyon.hubs.v1', JSON.stringify([{ id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 }]))
      const computer = (id: string, kind: string, status: string) => ({ config: { id, kind, name: id }, status }) as never
      expect(pageHosts([computer('laptop', 'link', 'online'), computer('nas', 'ssh', 'online'), computer('off', 'link', 'offline'), computer('hub:h1/box', 'node', 'online')])).toEqual(['hub:h1/local', 'laptop', 'nas'])
    } finally {
      delete (window as any).Capacitor
    }
  })

  test("the core out of reach keeps the computers' pages it had; it and every hub out of reach is an error", async () => {
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    api.pages.mockResolvedValue({ pages: [{ host: 'laptop', slug: 'plan', name: 'Plan', project: '/p', by: 'cli', added: 1, folder: '/p/plan' }], outdated: [] } as never)
    try {
      await loadPages()
      api.pages.mockRejectedValue(new Error('The core stopped') as never)
      await loadPages()
      expect(usePages.getState().pages!.map((p) => p.id)).toEqual(['laptop/plan'])
      expect(usePages.getState().error).toBe('The core stopped')
    } finally {
      delete (window as any).Capacitor
    }
  })

  test("the phone app asks each paired hub, naming the hub's machines as the app does; one out of reach keeps the others", async () => {
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([
      { id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 },
      { id: 'h2', name: 'nas', url: 'http://nas:4777', token: 't2', pairedAt: 2 },
    ]))
    api.hubPages.mockImplementation((async (hub: { id: string }) => {
      if (hub.id === 'h2') throw new Error("Couldn't reach nas")
      return { pages: [{ host: 'local', slug: 'plan', name: 'Plan', project: '/p', by: 'cli', added: 1, folder: '/p/plan' }] }
    }) as never)
    try {
      await loadPages()
      expect(usePages.getState().pages!.map((p) => [p.id, p.host])).toEqual([['hub:h1/local/plan', 'hub:h1/local']])
      expect(usePages.getState().error).toBeUndefined()
      expect(pageHosts()).toEqual(['hub:h1/local', 'hub:h2/local'])
      expect(usePages.getState().unreachable).toEqual(['h2'])
    } finally {
      delete (window as any).Capacitor
    }
  })

  test('a hub that refuses the phone (forgotten there) drops its pages and says so, never kept as out of reach', async () => {
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([
      { id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 },
      { id: 'h2', name: 'nas', url: 'http://nas:4777', token: 't2', pairedAt: 2 },
    ]))
    const answer: Record<string, number> = { h1: 200, h2: 200 }
    api.hubPages.mockImplementation((async (hub: { id: string }) => {
      if (answer[hub.id] !== 200) throw new ApiError(answer[hub.id]!, 'Unauthorized')
      return { pages: [{ host: 'local', slug: `${hub.id}-plan`, name: 'Plan', project: '/p', by: 'cli', added: 1, folder: '/p/plan' }] }
    }) as never)
    const ids = () => usePages.getState().pages!.map((p) => p.id).sort()
    try {
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/h1-plan', 'hub:h2/local/h2-plan'])
      for (const status of [401, 403]) {
        answer.h2 = status
        await loadPages()
        expect(ids()).toEqual(['hub:h1/local/h1-plan'])
        expect(usePages.getState()).toMatchObject({ forgotten: ['h2'], unreachable: [], error: undefined })
      }
      // paired again: its list is back and the line goes
      answer.h2 = 200
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/h1-plan', 'hub:h2/local/h2-plan'])
      expect(usePages.getState().forgotten).toEqual([])
    } finally {
      delete (window as any).Capacitor
    }
  })

  test('signed out, the list goes with the sign-in; a request that never got an answer keeps it', async () => {
    api.pages.mockResolvedValue({ pages: [entry('plan')] } as never)
    await loadPages()
    expect(usePages.getState().pages!.map((p) => p.slug)).toEqual(['plan'])
    api.pages.mockRejectedValue(new TypeError('Failed to fetch') as never)
    await loadPages()
    expect(usePages.getState()).toMatchObject({ error: 'Failed to fetch' })
    expect(usePages.getState().pages!.map((p) => p.slug)).toEqual(['plan'])
    api.pages.mockRejectedValue(new ApiError(401, 'Sign in first') as never)
    await loadPages()
    expect(usePages.getState()).toMatchObject({ pages: [], error: 'Sign in first' })
  })

  test("the phone app keeps a hub's pages through a failed poll, marked unreachable, until the hub answers", async () => {
    ;(window as any).Capacitor = { isNativePlatform: () => true, getPlatform: () => 'android', Plugins: {} }
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([
      { id: 'h1', name: 'arm', url: 'http://arm:4777', token: 't1', pairedAt: 1 },
      { id: 'h2', name: 'nas', url: 'http://nas:4777', token: 't2', pairedAt: 2 },
    ]))
    const down = new Set<string>()
    const listed: Record<string, string[]> = { h1: ['plan'], h2: ['nas-plan'] }
    api.hubPages.mockImplementation((async (hub: { id: string }) => {
      if (down.has(hub.id)) throw new Error(`Couldn't reach ${hub.id}`)
      return { pages: listed[hub.id]!.map((slug) => ({ host: 'local', slug, name: slug, project: '/p', by: 'cli', added: 1, folder: `/p/${slug}` })) }
    }) as never)
    const ids = () => usePages.getState().pages!.map((p) => p.id).sort()
    try {
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/plan', 'hub:h2/local/nas-plan'])
      // a blip on one hub: its pages stay, the other's list moves on
      down.add('h2')
      listed.h1 = ['plan', 'new']
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/new', 'hub:h1/local/plan', 'hub:h2/local/nas-plan'])
      expect(usePages.getState()).toMatchObject({ unreachable: ['h2'], error: undefined })
      // every hub out of reach: nothing is wiped, and it says so
      down.add('h1')
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/new', 'hub:h1/local/plan', 'hub:h2/local/nas-plan'])
      expect(usePages.getState().error).toBe("Couldn't reach h1")
      // back: only an answer changes a hub's list
      down.clear()
      listed.h2 = []
      await loadPages()
      expect(ids()).toEqual(['hub:h1/local/new', 'hub:h1/local/plan'])
      expect(usePages.getState()).toMatchObject({ unreachable: [], error: undefined })
    } finally {
      delete (window as any).Capacitor
    }
  })
})
