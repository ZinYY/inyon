import fs from 'node:fs'
import path from 'node:path'
import { REPO } from '../lib/hub.ts'
import type { Context } from '../lib/context.ts'
import { type Scenario } from '../lib/scenario.ts'

/** Every file of the copied site gets this mtime, so its ETags are the same on every run (a January 2026 date the normalizer keeps). */
const MTIME = new Date('2026-01-15T12:00:00Z')

/**
 * fixtures/pages/site copied under the hub's home, with a progress.json dated like the other
 * fixtures, a symlink that leads out of the site, and every mtime fixed. Returns the site's folder.
 */
function layOutSite(ctx: Context) {
  const site = path.join(ctx.hub.home, 'plans', 'fixture')
  fs.cpSync(path.join(REPO, 'fixtures/pages/site'), site, { recursive: true })
  fs.writeFileSync(
    path.join(site, 'progress.json'),
    JSON.stringify({ updated: '2026-01-15T12:00:00Z', now: 'Writing the sub page', releases: [{ v: '0.1', r: 'R1', title: 'Fixture', tasks: [{ id: 'a', status: 'done' }, { id: 'b', status: 'doing' }, { id: 'c', status: 'todo' }] }], log: [] }),
  )
  fs.writeFileSync(path.join(ctx.hub.home, 'plans', 'secret.txt'), 'outside the site\n')
  fs.symlinkSync('../secret.txt', path.join(site, 'leak.txt'))
  const touch = (p: string) => {
    for (const e of fs.readdirSync(p, { withFileTypes: true })) if (e.isDirectory()) touch(path.join(p, e.name))
    fs.utimesSync(p, MTIME, MTIME)
  }
  touch(site)
  for (const f of fs.readdirSync(site, { recursive: true }) as string[]) fs.lutimesSync(path.join(site, f), MTIME, MTIME)
  // a folder with no index.html, refused as a page
  fs.mkdirSync(path.join(ctx.hub.home, 'plans', 'empty'), { recursive: true })
  return site
}

export default {
  name: 'pages',
  title: 'Pages: adding a site, the list, a token link and what /v/ serves under it, refusals, framed API calls, revoking and removing',
  async run(ctx) {
    const site = layOutSite(ctx)
    const list = path.join(ctx.hub.home, '.inyon', 'pages.json')
    await ctx.signIn()

    await ctx.step('no pages yet')
    await ctx.rest('GET', '/api/pages')

    await ctx.step('folders that cannot be pages')
    await ctx.rest('POST', '/api/pages/local', { path: 'plans/fixture' })
    await ctx.rest('POST', '/api/pages/local', { path: ctx.hub.home })
    await ctx.rest('POST', '/api/pages/local', { path: path.join(ctx.hub.home, '.ssh', 'site') })
    await ctx.rest('POST', '/api/pages/local', { path: path.join(ctx.hub.home, 'plans', 'empty') })
    await ctx.rest('POST', '/api/pages/local', { path: site, slug: 'Not A Slug' })
    await ctx.rest('POST', '/api/pages/laptop', { path: site })
    await ctx.rest('POST', '/api/pages/local', undefined, { raw: '{not json', headers: { 'content-type': 'application/json' } })

    await ctx.step('the site is added; its marker gives the slug')
    await ctx.rest('POST', '/api/pages/local', { path: site, name: 'Fixture plan' })
    await ctx.rest('POST', '/api/pages/local', { path: site, name: 'Fixture plan' })
    await ctx.rest('POST', '/api/pages/local', { path: site, slug: 'other' })
    await ctx.rest('GET', '/api/pages')
    ctx.entry({ file: 'pages.json', pages: JSON.parse(fs.readFileSync(list, 'utf8')).pages })

    await ctx.step('a link to it, and to a page that is not there')
    const opened = await ctx.rest('POST', '/api/pages/local/fixture/open')
    const url = (opened.body as { url: string }).url
    ctx.norm.learn(url.slice(3, -1), 'page-token')
    await ctx.rest('POST', '/api/pages/local/nope/open')
    await ctx.rest('POST', '/api/pages/laptop/fixture/open')

    await ctx.step('what the link serves, with no cookie')
    await ctx.raw('GET', url.slice(0, -1))
    const index = await ctx.raw('GET', url)
    await ctx.raw('GET', `${url}?theme=dark`)
    await ctx.raw('GET', `${url}sub`)
    await ctx.raw('GET', `${url}sub/`)
    await ctx.raw('GET', `${url}style.css`)
    await ctx.raw('GET', `${url}notes.md`)
    await ctx.raw('GET', `${url}progress.json`)
    await ctx.raw('HEAD', `${url}style.css`)
    await ctx.raw('GET', url, { headers: { 'if-none-match': index.headers.get('etag') ?? '' } })

    await ctx.step('what it never serves')
    await ctx.raw('GET', `${url}.inyon-page.json`)
    await ctx.raw('GET', `${url}.hidden/note.html`)
    await ctx.raw('GET', `${url}%2ehidden/note.html`)
    await ctx.raw('GET', `${url}empty/`)
    await ctx.raw('GET', `${url}missing.html`)
    await ctx.raw('GET', `${url}leak.txt`)
    // dot segments resolve before anything else, so climbing out of the site lands on another
    // "token" (403); one that stays inside is just a path
    await ctx.raw('GET', `${url}sub/../../secret.txt`)
    await ctx.raw('GET', `${url}sub/%2e%2e/%2E%2e/secret.txt`)
    await ctx.raw('GET', `${url}sub\\..\\..\\secret.txt`)
    await ctx.raw('GET', `${url}sub/%2e%2e/style.css`)
    await ctx.raw('GET', `${url}sub%2f..%2f..%2fsecret.txt`)
    await ctx.raw('POST', `${url}index.html`)

    // routes match the path as it came (Bun's router does), so a dot segment into /api is no
    // route at all, cookie or not; only /v/ resolves them
    await ctx.step('a dot segment into /api is no route')
    await ctx.raw('GET', '/api/nothing/../pages', { as: 'browser' })
    await ctx.raw('GET', '/api/%2e%2E/api/pages', { as: 'browser' })
    await ctx.raw('GET', '/v/x/../../api/pages')

    await ctx.step('a bad token, a cookie, a wrong Host')
    await ctx.raw('GET', '/v/v1.bad.token/')
    // the same token with its signature's last character changed
    const altered = `${url.slice(3, -2)}${url.at(-2) === 'A' ? 'B' : 'A'}`
    ctx.norm.learn(altered, 'page-token-altered')
    await ctx.raw('GET', `/v/${altered}/`)
    await ctx.raw('GET', '/v/v1.bad.token/', { as: 'browser' })
    await ctx.raw('GET', url, { as: 'browser' })
    await ctx.raw('GET', url, { headers: { host: 'evil.example' } })

    await ctx.step('a framed page or another site calling the API with the cookie')
    await ctx.rest('GET', '/api/session', undefined, { origin: 'null' })
    await ctx.rest('GET', '/api/pages', undefined, { origin: 'https://evil.example', headers: { 'sec-fetch-site': 'cross-site' } })
    await ctx.rest('POST', '/api/pages/local/fixture/open', undefined, { origin: 'null' })
    await ctx.rest('GET', '/api/pages', undefined, { headers: { 'sec-fetch-site': 'same-origin' } })

    await ctx.step('revoking ends the link; a new one works')
    await ctx.rest('POST', '/api/pages/local/fixture/revoke')
    await ctx.raw('GET', url)
    const again = await ctx.rest('POST', '/api/pages/local/fixture/open')
    const url2 = (again.body as { url: string }).url
    ctx.norm.learn(url2.slice(3, -1), 'page-token-2')
    await ctx.raw('GET', url2)
    await ctx.rest('POST', '/api/pages/local/nope/revoke')

    await ctx.step('another program removes it and adds it again: the old link ends')
    const file = JSON.parse(fs.readFileSync(list, 'utf8'))
    file.pages[0].added = Date.parse('2026-01-16T00:00:00Z')
    fs.writeFileSync(list, JSON.stringify(file, null, 2))
    await ctx.raw('GET', url2)
    const third = await ctx.rest('POST', '/api/pages/local/fixture/open')
    const url3 = (third.body as { url: string }).url
    ctx.norm.learn(url3.slice(3, -1), 'page-token-3')
    await ctx.raw('GET', url3)

    await ctx.step('removing it from the app')
    await ctx.rest('DELETE', '/api/pages/local/fixture')
    await ctx.rest('DELETE', '/api/pages/local/fixture')
    await ctx.raw('GET', url3)
    await ctx.rest('POST', '/api/pages/local/fixture/open')
    await ctx.rest('GET', '/api/pages')
    ctx.entry({ file: 'pages.json', pages: JSON.parse(fs.readFileSync(list, 'utf8')).pages })
  },
} satisfies Scenario
