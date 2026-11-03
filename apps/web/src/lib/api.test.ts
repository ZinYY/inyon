import { afterAll, afterEach, beforeEach, describe, expect, mock, spyOn, test } from 'bun:test'
import { realApi as api, realApiModule } from '../test/fakes.ts'

// bun keeps a spy in place for every test file after this one
afterAll(() => mock.restore())

// The real request code (the rest of the suite gets a fake api), against a fake fetch.
const { ApiError, isTransient, paneSlug, paneFromSlug } = realApiModule
const realFetch = globalThis.fetch
let reply: () => Response
const fetchMock = mock(async (_req: Request) => reply())
const ok = (body: unknown = {}) => () => new Response(JSON.stringify(body), { status: 200 })

beforeEach(() => {
  fetchMock.mockClear()
  reply = ok()
  globalThis.fetch = fetchMock as any
})
afterEach(() => {
  globalThis.fetch = realFetch
})

const sent = () => fetchMock.mock.calls[0][0]
const json = async () => JSON.parse(await sent().text())
const failure = (p: Promise<unknown>) => p.then(() => { throw new Error('expected a failure') }, (e) => e as InstanceType<typeof ApiError>)

describe('requests', () => {
  test('a GET goes to the hub with the session cookie and no body', async () => {
    reply = ok({ authed: true })
    expect(await api.session()).toEqual({ authed: true })
    const req = sent()
    expect(req.method).toBe('GET')
    expect(req.url).toBe('http://localhost:5173/api/session')
    expect(req.credentials).toBe('same-origin')
    expect(req.headers.get('content-type')).toBeNull()
  })

  test("a machine's setup report names the agent catalog this app knows, so a hub leaves out newer agents", async () => {
    await api.setup('m1')
    expect(new URL(sent().url).searchParams.get('catalog')).toBe('2')
  })

  test('a body goes as JSON', async () => {
    await api.login('hunter22')
    expect(sent().method).toBe('POST')
    expect(sent().headers.get('content-type')).toBe('application/json')
    expect(await json()).toEqual({ password: 'hunter22' })
  })

  test('pane paths drop the % and encode the machine id', async () => {
    await api.send('my box', '%12', 'hello', 'r1')
    expect(sent().url).toBe('http://localhost:5173/api/hosts/my%20box/panes/12/send')
    expect(await json()).toEqual({ text: 'hello', requestId: 'r1' })
  })

  test('renaming sends the new name with PUT', async () => {
    await api.rename('box', '%3', 'Login fix')
    expect([sent().method, sent().url]).toEqual(['PUT', 'http://localhost:5173/api/hosts/box/panes/3/name'])
    expect(await json()).toEqual({ name: 'Login fix' })
  })

  test('editing a server PATCHes only what changed', async () => {
    await api.editServer('srv/1', { name: 'Work' })
    expect([sent().method, sent().url]).toEqual(['PATCH', 'http://localhost:5173/api/hosts/srv%2F1'])
    expect(await json()).toEqual({ name: 'Work' })
  })

  test('query strings are encoded', async () => {
    await api.readConfig('box', '/home/me/.claude/settings.json?x=1&y')
    const url = new URL(sent().url)
    expect(url.pathname).toBe('/api/hosts/box/config')
    expect(url.searchParams.get('path')).toBe('/home/me/.claude/settings.json?x=1&y')
  })

  test('pairing tells the machine where the hub is', async () => {
    await api.pair('Laptop')
    expect(await json()).toEqual({ name: 'Laptop', origin: 'http://localhost:5173' })
  })

  test('a success without JSON resolves to an empty object', async () => {
    reply = () => new Response('', { status: 204 })
    expect(await api.interrupt('box', '%1')).toEqual({})
  })
})

describe('temp files', () => {
  test('list, files and preview are GETs with the path in the query', async () => {
    await api.tempList('my box')
    expect([sent().method, sent().url]).toEqual(['GET', 'http://localhost:5173/api/hosts/my%20box/temp'])
    fetchMock.mockClear()
    await api.tempFiles('box', '/tmp/claude-1000/-home-me/a b')
    expect(sent().url).toBe('http://localhost:5173/api/hosts/box/temp/files?path=%2Ftmp%2Fclaude-1000%2F-home-me%2Fa+b')
    fetchMock.mockClear()
    await api.tempPreview('box', '/tmp/x.png', 1500000)
    expect(sent().url).toBe('http://localhost:5173/api/hosts/box/temp/preview?path=%2Ftmp%2Fx.png&max=1500000')
    fetchMock.mockClear()
    await api.tempPreview('box', '/tmp/x.png')
    expect(sent().url).toBe('http://localhost:5173/api/hosts/box/temp/preview?path=%2Ftmp%2Fx.png')
  })

  test('delete POSTs the paths', async () => {
    reply = ok({ removed: ['/tmp/a'], freed: 10, failed: [] })
    expect(await api.tempDelete('box', ['/tmp/a', '/tmp/b'])).toEqual({ removed: ['/tmp/a'], freed: 10, failed: [] })
    expect([sent().method, sent().url]).toEqual(['POST', 'http://localhost:5173/api/hosts/box/temp/delete'])
    expect(await json()).toEqual({ paths: ['/tmp/a', '/tmp/b'] })
  })

  test("the app waits longer than the hub waits on the machine, and less than the server's idle limit", async () => {
    const { TEMP_WAIT, TEMP_DELETE_WAIT } = realApiModule
    // the hub gives up on the machine after 60 s (100 s for a delete); the server drops a quiet
    // connection after 120 s
    expect(TEMP_WAIT).toBeGreaterThan(60_000)
    expect(TEMP_DELETE_WAIT).toBeGreaterThan(100_000)
    expect(TEMP_DELETE_WAIT).toBeLessThan(120_000)
    const timeout = spyOn(AbortSignal, 'timeout')
    try {
      await api.tempList('box')
      await api.tempFiles('box', '/tmp/a')
      await api.tempPreview('box', '/tmp/a')
      await api.tempDelete('box', ['/tmp/a'])
      expect(timeout.mock.calls.map((c) => c[0])).toEqual([TEMP_WAIT, TEMP_WAIT, TEMP_WAIT, TEMP_DELETE_WAIT])
    } finally {
      timeout.mockRestore()
    }
  })

  test("a machine whose inyon is too old fails with the hub's words", async () => {
    reply = () => new Response(JSON.stringify({ error: 'Update Inyon on box to see its temp files' }), { status: 409 })
    const e = await failure(api.tempList('box'))
    expect([e.status, e.message]).toEqual([409, 'Update Inyon on box to see its temp files'])
  })
})

describe('errors', () => {
  test("the hub's message and status come through", async () => {
    reply = () => new Response(JSON.stringify({ error: 'No such pane' }), { status: 404 })
    const err = await failure(api.interrupt('box', '%1'))
    expect(err).toBeInstanceOf(ApiError)
    expect([err.status, err.message]).toEqual([404, 'No such pane'])
  })

  test('without one, a generic message with the status', async () => {
    reply = () => new Response('<html>Bad gateway</html>', { status: 502 })
    const err = await failure(api.interrupt('box', '%1'))
    expect(err.message).toBe('Request failed (502)')
  })

  test('a 401 signs the app out, except for a wrong password at login', async () => {
    const out = mock(() => {})
    window.addEventListener('inyon:signed-out', out)
    try {
      reply = () => new Response('{}', { status: 401 })
      await api.configs('box').catch(() => {})
      expect(out).toHaveBeenCalledTimes(1)
      await api.login('wrong').catch(() => {})
      expect(out).toHaveBeenCalledTimes(1)
    } finally {
      window.removeEventListener('inyon:signed-out', out)
    }
  })

  test('logout never throws', async () => {
    reply = () => new Response('{}', { status: 500 })
    expect(await api.logout()).toBeUndefined()
  })
})

describe('upload', () => {
  /** Just enough XMLHttpRequest to answer one upload. */
  class FakeXHR {
    static last: FakeXHR
    static answer = { status: 200, body: '' }
    upload: { onprogress?: (e: { lengthComputable: boolean; loaded: number; total: number }) => void } = {}
    onload?: () => void
    onerror?: () => void
    status = 0
    responseText = ''
    url = ''
    form?: FormData
    open(_method: string, url: string) {
      this.url = url
    }
    send(form: FormData) {
      FakeXHR.last = this
      this.form = form
      queueMicrotask(() => {
        this.upload.onprogress?.({ lengthComputable: true, loaded: 5, total: 10 })
        this.status = FakeXHR.answer.status
        this.responseText = FakeXHR.answer.body
        if (this.status) this.onload?.()
        else this.onerror?.()
      })
    }
  }
  const realXHR = globalThis.XMLHttpRequest
  beforeEach(() => void (globalThis.XMLHttpRequest = FakeXHR as any))
  afterEach(() => void (globalThis.XMLHttpRequest = realXHR))
  const file = () => new File(['hi'], 'a.txt', { type: 'text/plain' })

  const saved = { name: 'a.txt', path: '/up/a.txt', size: 2, type: 'text/plain' }
  /** A hub from 0.24: starts, takes the chunk, finishes. */
  const chunked = (req: Request) => {
    const path = new URL(req.url).pathname
    if (path.endsWith('/start')) return Response.json({ id: 'u1', received: 0, chunk: 1048576, size: 2 })
    if (req.method === 'PUT') return Response.json({ id: 'u1', received: 2, chunk: 1048576, size: 2 })
    return Response.json(saved)
  }

  test('goes a chunk at a time to a hub from 0.24, with progress, and resolves to the saved file', async () => {
    fetchMock.mockImplementation(async (req: Request) => chunked(req))
    const progress: number[] = []
    expect(await api.upload('box', '%4', file(), (f) => progress.push(f))).toEqual(saved)
    const reqs = fetchMock.mock.calls.map((c) => c[0] as Request)
    expect(reqs.map((r) => `${r.method} ${new URL(r.url).pathname}`)).toEqual(['POST /api/hosts/box/uploads/start', 'PUT /api/hosts/box/uploads/u1/0', 'POST /api/hosts/box/uploads/u1/finish'])
    expect(await reqs[0]!.json()).toEqual({ name: 'a.txt', size: 2, type: 'text/plain', pane: '4' })
    expect((await reqs[2]!.json()).sha256).toBe('8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4')
    expect(progress.at(-1)).toBe(1)
    fetchMock.mockImplementation(async (_req: Request) => reply())
  })

  test("fails with the hub's message", async () => {
    reply = () => Response.json({ error: 'Too large' }, { status: 413 })
    expect(await api.upload('box', undefined, file()).catch((e) => [e.status, e.message])).toEqual([413, 'Too large'])
  })

  test('goes the old way to a hub from before 0.24', async () => {
    reply = () => Response.json({ error: 'Not found' }, { status: 404 })
    FakeXHR.answer = { status: 200, body: JSON.stringify({ files: [saved] }) }
    const progress: number[] = []
    expect(await api.upload('box', '%4', file(), (f) => progress.push(f))).toEqual(saved)
    expect(progress).toEqual([0.5])
    expect(FakeXHR.last.url).toBe('/api/hosts/box/uploads')
    expect(FakeXHR.last.form!.get('pane')).toBe('4')
    FakeXHR.answer = { status: 0, body: '' }
    expect(await api.upload('box', undefined, file()).catch((e) => e.message)).toBe('Network error during upload')
  })
})

describe("a paired hub's machines", () => {
  const hub = { id: 'h1', name: 'studio', url: 'http://box:4777', token: 'tok', pairedAt: 1 }
  const machine = 'hub:h1/local'
  /** What reached the hub: Chromium's rules, where a body handed over as a stream only goes over HTTP/2. */
  const calls: { url: string; method: string; headers: Headers; credentials?: string; body?: string }[] = []
  const chromium = async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : undefined
    const body = init?.body ?? req?.body
    // a hub speaks HTTP/1.1: net::ERR_ALPN_NEGOTIATION_FAILED, which a page sees as a TypeError
    if (body instanceof ReadableStream) throw new TypeError('Failed to fetch')
    calls.push({
      url: req?.url ?? String(input),
      method: init?.method ?? req?.method ?? 'GET',
      headers: new Headers(init?.headers ?? req?.headers),
      credentials: init?.credentials ?? req?.credentials,
      body: body === undefined || body === null ? undefined : new TextDecoder().decode(body as ArrayBuffer),
    })
    return reply()
  }
  beforeEach(() => {
    calls.length = 0
    localStorage.setItem('inyon.hubs.v1', JSON.stringify([hub]))
    globalThis.fetch = mock(chromium) as any
  })
  afterEach(() => localStorage.removeItem('inyon.hubs.v1'))

  test('every action on a session reaches the hub, under its own name for the machine, with the token', async () => {
    const actions: [string, () => Promise<unknown>, string, unknown][] = [
      ['send', () => api.send(machine, '%3', 'hello', 'r1'), 'POST /panes/3/send', { text: 'hello', requestId: 'r1' }],
      ['goal', () => api.setting(machine, '%3', '/goal tests pass'), 'POST /panes/3/send', { text: '/goal tests pass', setting: true }],
      ['model', () => api.setting(machine, '%3', '/model opus'), 'POST /panes/3/send', { text: '/model opus', setting: true }],
      ['mode', () => api.mode(machine, '%3', 'plan'), 'POST /panes/3/mode', { mode: 'plan' }],
      ['answer', () => api.answer(machine, '%3', 2, undefined, 'q1'), 'POST /panes/3/answer', { option: 2, question: 'q1' }],
      ['typed answer', () => api.input(machine, '%3', 'abc'), 'POST /panes/3/input', { text: 'abc' }],
      ['keys', () => api.keys(machine, '%3', ['Escape']), 'POST /panes/3/keys', { keys: ['Escape'] }],
      ['rewind', () => api.rewind(machine, '%3', 'that one'), 'POST /panes/3/rewind', { text: 'that one' }],
      ['remote control', () => api.rc(machine, '%3', true), 'POST /panes/3/rc', { on: true }],
      ['ultracode', () => api.ultracode(machine, '%3', true), 'POST /panes/3/ultracode', { on: true }],
      ['rename', () => api.rename(machine, '%3', 'Login fix'), 'PUT /panes/3/name', { name: 'Login fix' }],
      ['start', () => api.start(machine, '%3', 'claude'), 'POST /panes/3/start', { agent: 'claude' }],
      ['stop a task', () => api.stopTask(machine, { id: 't1' }), 'POST /tasks/stop', { id: 't1' }],
      ['new session', () => api.launch(machine, { agent: 'claude', cwd: '/w' }), 'POST /launch', { agent: 'claude', cwd: '/w' }],
      ['stop', () => api.interrupt(machine, '%3'), 'POST /panes/3/interrupt', undefined],
      ['end', () => api.end(machine, '%3'), 'POST /panes/3/end', undefined],
      ['models', () => api.models(machine, '%3'), 'GET /panes/3/models', undefined],
    ]
    for (const [name, run, route, body] of actions) {
      calls.length = 0
      await run()
      const c = calls[0]
      if (!c) throw new Error(`${name} never reached the hub`)
      const [method, rest] = route.split(' ')
      expect([name, c.method, c.url]).toEqual([name, method, `http://box:4777/api/hosts/local${rest}`])
      expect(c.headers.get('authorization')).toBe('Bearer tok')
      expect(c.credentials).toBe('omit')
      expect(c.body === undefined ? undefined : JSON.parse(c.body)).toEqual(body)
    }
  })

  test('an attachment goes to the hub a chunk at a time, with the token', async () => {
    reply = () => {
      const c = calls.at(-1)!
      if (c.url.endsWith('/start')) return Response.json({ id: 'u1', received: 0, chunk: 1048576, size: 2 })
      if (c.method === 'PUT') return Response.json({ id: 'u1', received: 2, chunk: 1048576, size: 2 })
      return Response.json({ name: 'a.txt', path: '/up/a.txt', size: 2, type: 'text/plain' })
    }
    expect((await api.upload(machine, '%4', new File(['hi'], 'a.txt', { type: 'text/plain' }))).path).toBe('/up/a.txt')
    expect(calls.map((c) => `${c.method} ${c.url} ${c.headers.get('authorization')}`)).toEqual([
      'POST http://box:4777/api/hosts/local/uploads/start Bearer tok',
      'PUT http://box:4777/api/hosts/local/uploads/u1/0 Bearer tok',
      'POST http://box:4777/api/hosts/local/uploads/u1/finish Bearer tok',
    ])
    expect(calls[1]!.headers.get('content-type')).toBe('application/octet-stream')
    expect(calls[1]!.body).toBe('hi')
  })

  test('an older hub takes the attachment in pieces, the way it knows', async () => {
    reply = () => (calls.at(-1)!.url.endsWith('/start') ? Response.json({ error: 'Not found' }, { status: 404 }) : new Response(JSON.stringify({ files: [{ name: 'a.txt', path: '/up/a.txt', size: 2, type: 'text/plain' }] })))
    expect((await api.upload(machine, '%4', new File(['hi'], 'a.txt', { type: 'text/plain' }))).path).toBe('/up/a.txt')
    expect([calls[1]!.method, calls[1]!.url, calls[1]!.headers.get('authorization')]).toEqual(['POST', 'http://box:4777/api/hosts/local/uploads', 'Bearer tok'])
    expect(calls[1]!.headers.get('content-type')).toStartWith('multipart/form-data; boundary=')
    expect(calls[1]!.body).toContain('hi')
  })

  test("its pages: the list, links, revoking, adding and removing reach that hub with the token, under the hub's own name for the machine", async () => {
    const actions: [string, () => Promise<unknown>, string, unknown][] = [
      ['list', () => api.hubPages(hub), 'GET http://box:4777/api/pages', undefined],
      ['open', () => api.openPage(machine, 'ghostex'), 'POST http://box:4777/api/pages/local/ghostex/open', undefined],
      ['revoke', () => api.revokePage(machine, 'ghostex'), 'POST http://box:4777/api/pages/local/ghostex/revoke', undefined],
      ['add', () => api.addPage(machine, { path: '/srv/site', name: 'Site' }), 'POST http://box:4777/api/pages/local', { path: '/srv/site', name: 'Site' }],
      ['remove', () => api.removePage(machine, 'ghostex'), 'DELETE http://box:4777/api/pages/local/ghostex', undefined],
    ]
    for (const [name, run, route, body] of actions) {
      calls.length = 0
      await run()
      const c = calls[0]
      if (!c) throw new Error(`${name} never reached the hub`)
      expect([name, `${c.method} ${c.url}`]).toEqual([name, route])
      expect(c.headers.get('authorization')).toBe('Bearer tok')
      expect(c.credentials).toBe('omit')
      expect(c.body === undefined ? undefined : JSON.parse(c.body)).toEqual(body)
    }
  })

  test("a hub that can't be reached is named; one that refuses the token doesn't sign the app out", async () => {
    globalThis.fetch = mock(async () => Promise.reject(new TypeError('Failed to fetch'))) as any
    const err = await failure(api.send(machine, '%3', 'hello'))
    expect([err.status, err.message]).toEqual([0, "Couldn't reach studio. Check this phone's connection to it."])
    expect(isTransient(err)).toBe(true)
    globalThis.fetch = mock(chromium) as any
    const out = mock(() => {})
    window.addEventListener('inyon:signed-out', out)
    try {
      reply = () => new Response(JSON.stringify({ error: 'Not signed in' }), { status: 401 })
      expect((await failure(api.send(machine, '%3', 'hello'))).message).toBe('Not signed in')
      expect(out).not.toHaveBeenCalled()
    } finally {
      window.removeEventListener('inyon:signed-out', out)
    }
  })
})

test("in a browser, a page's calls go to the hub the app came from, with the cookie", async () => {
  await api.openPage('local', 'ghostex')
  expect([sent().method, sent().url, sent().credentials]).toEqual(['POST', 'http://localhost:5173/api/pages/local/ghostex/open', 'same-origin'])
  fetchMock.mockClear()
  await api.pages()
  expect([sent().method, sent().url]).toEqual(['GET', 'http://localhost:5173/api/pages'])
})

test('pane slugs', () => {
  expect(paneSlug('%12')).toBe('12')
  expect(paneFromSlug('12')).toBe('%12')
  expect(paneFromSlug('%12')).toBe('%12')
})
