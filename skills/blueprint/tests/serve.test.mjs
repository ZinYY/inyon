// bun test skills/blueprint/tests
// lib.mjs serve() answers like Inyon's hub does for /v/<token>/. Requests go out as raw bytes, since
// fetch would tidy dot segments and encodings before they reach the server.
import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { connect } from 'node:net';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { serve } from '../scripts/lib.mjs';

const tmp = mkdtempSync(join(tmpdir(), 'bp-serve-'));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

function raw(url, { method = 'GET', target, headers = {} }) {
  const { port } = new URL(url);
  return new Promise((ok, no) => {
    const s = connect(+port, '127.0.0.1');
    const chunks = [];
    s.on('data', (c) => chunks.push(c));
    s.on('error', no);
    s.on('end', () => {
      const buf = Buffer.concat(chunks);
      const cut = buf.indexOf('\r\n\r\n');
      const [line, ...hs] = buf.subarray(0, cut).toString('latin1').split('\r\n');
      const h = {};
      for (const x of hs) { const i = x.indexOf(':'); h[x.slice(0, i).toLowerCase()] = x.slice(i + 1).trim(); }
      ok({ status: +line.split(' ')[1], headers: h, body: buf.subarray(cut + 4) });
    });
    const lines = [`${method} ${target} HTTP/1.1`, ...Object.entries({ connection: 'close', ...headers }).map(([k, v]) => `${k}: ${v}`)];
    s.write(lines.join('\r\n') + '\r\n\r\n');
  });
}
const IGNORED = ['date', 'content-length', 'transfer-encoding', 'connection', 'keep-alive'];
const strip = (h) => Object.fromEntries(Object.entries(h).filter(([k]) => !IGNORED.includes(k)));

describe('serve() on the template site', () => {
  let srv, site, csp;
  beforeAll(async () => {
    site = join(tmp, 'tpl');
    cpSync(fileURLToPath(new URL('../assets/site', import.meta.url)), site, { recursive: true });
    writeFileSync(join(site, '.inyon-page.json'), '{"v":1}');
    writeFileSync(join(site, '.env'), 'SECRET=1');
    mkdirSync(join(site, 'research'));
    writeFileSync(join(site, 'research/notes.md'), '# notes');
    srv = await serve(site);
  });
  afterAll(() => srv.close());
  const get = (target, o = {}) => raw(srv.url, { target, ...o, headers: { host: 'plan.test:8790', ...o.headers } });

  test('every answer has the sandbox and the same headers', async () => {
    const answers = await Promise.all(['/', '/kit/kit.js', '/progress.json', '/mockups', '/nope', '/.env'].map((t) => get(t)));
    answers.push(await get('/', { method: 'POST' }));
    for (const r of answers) {
      csp = r.headers['content-security-policy'];
      expect(csp).toStartWith('sandbox allow-scripts allow-forms allow-popups allow-downloads; ');
      expect(csp).not.toContain('allow-same-origin');
      expect(csp).toContain("script-src http://plan.test:8790/ 'unsafe-inline'");
      expect(csp).not.toContain('frame-ancestors');
      expect(r.headers['access-control-allow-origin']).toBe('*');
      expect(r.headers['referrer-policy']).toBe('no-referrer');
      expect(r.headers['x-content-type-options']).toBe('nosniff');
      expect(r.headers['set-cookie']).toBeUndefined();
    }
  });

  test('a folder without its slash gets a 301, keeping a plain query', async () => {
    const r = await get('/mockups?theme=dark');
    expect(r.status).toBe(301);
    expect(r.headers.location).toBe('/mockups/?theme=dark');
    expect(r.headers['cache-control']).toBe('no-store');
    expect((await get('/mockups/')).headers['content-type']).toBe('text/html;charset=utf-8');
  });

  test('files, types, ETags and progress.json', async () => {
    const js = await get('/kit/kit.js');
    expect(js.status).toBe(200);
    expect(js.headers['content-type']).toBe('text/javascript;charset=utf-8');
    expect(js.headers['cache-control']).toBe('no-cache');
    expect(js.body.equals(readFileSync(join(site, 'kit/kit.js')))).toBe(true);
    const again = await get('/kit/kit.js', { headers: { 'if-none-match': js.headers.etag } });
    expect(again.status).toBe(304);
    expect(again.body.length).toBe(0);
    expect(again.headers['content-type']).toBeUndefined();
    const p = await get('/progress.json', { headers: { 'if-none-match': '*' } });
    expect(p.status).toBe(200);
    expect(p.headers['cache-control']).toBe('no-store');
    expect(p.headers.etag).toBeUndefined();
    expect((await get('/research/notes.md')).headers['content-type']).toBe('text/plain;charset=utf-8');
    expect((await get('/kit/fonts/geist-latin-wght-normal.woff2')).headers['content-type']).toBe('font/woff2');
    const head = await get('/', { method: 'HEAD' });
    expect(head.status).toBe(200);
    expect(head.body.length).toBe(0);
  });

  test('no dot files, no listings, nothing outside, read only', async () => {
    for (const t of ['/.inyon-page.json', '/.env', '/%2eenv', '/research/', '/../../etc/passwd', '/%2e%2e/%2e%2e/etc/passwd', '/kit%2fkit.js', '/a%00b', '/%zz']) {
      expect([t, (await get(t)).status]).toEqual([t, 404]);
    }
    const post = await get('/', { method: 'POST' });
    expect(post.status).toBe(405);
    expect(post.headers.allow).toBe('GET, HEAD');
    expect((await get('/', { headers: { host: 'bad host; script-src *' } })).status).toBe(421);
  });
});

// The hub's own cases, from fixtures/pages/responses.json, with /v/<token> taken off the front. Only the
// cases where the hub accepted the token apply: there is no token here.
const fixtures = fileURLToPath(new URL('../../../fixtures/pages/', import.meta.url));
const haveFixtures = existsSync(join(fixtures, 'responses.json'));
describe.skipIf(!haveFixtures || process.platform === 'win32')('serve() matches fixtures/pages/responses.json', () => {
  const spec = haveFixtures ? JSON.parse(readFileSync(join(fixtures, 'responses.json'), 'utf8')) : { cases: [] };
  let srv, dir;
  beforeAll(async () => {
    dir = join(tmp, 'fx');
    const site = join(dir, 'site');
    cpSync(join(fixtures, spec.setup.copy), site, { recursive: true });
    mkdirSync(join(dir, 'outside'));
    for (const [name, text] of Object.entries(spec.setup.outside)) writeFileSync(join(dir, 'outside', name), text);
    for (const [name, target] of Object.entries(spec.setup.symlinks)) symlinkSync(target, join(site, name));
    const t = spec.setup.mtimeMs / 1000;
    const touch = (p) => { for (const f of readdirSync(p, { withFileTypes: true })) { const full = join(p, f.name); if (f.isSymbolicLink()) continue; if (f.isDirectory()) touch(full); else utimesSync(full, t, t); } };
    touch(dir);
    srv = await serve(site);
  });
  afterAll(() => srv?.close());

  const claims = (tok) => { try { return JSON.parse(Buffer.from(tok.split('.')[1], 'base64url').toString()); } catch { return null; } };
  const usable = spec.cases.filter((c) => {
    const m = /^\/v\/([^/?]+)(\/.*)$/.exec(c.request.target);
    const csp = c.response.headers['content-security-policy'] || '';
    const cl = m && claims(m[1]);
    return m && ['GET', 'HEAD'].includes(c.request.method) && c.request.headers.host === spec.context.host && csp.includes(`/v/${m[1]}/`) && cl?.h === spec.context.site.h && cl?.s === spec.context.site.s;
  });

  test('enough cases apply', () => expect(usable.length).toBeGreaterThan(30));
  for (const c of usable) {
    test(c.name, async () => {
      const [, tok, rest] = /^\/v\/([^/?]+)(\/.*)$/.exec(c.request.target);
      const r = await raw(srv.url, { method: c.request.method, target: rest, headers: c.request.headers });
      const csp = (v) => v.split(`/v/${tok}/`).join('/');
      const want = Object.fromEntries(Object.entries(c.response.headers).map(([k, v]) => [k, k === 'location' ? v.replace(`/v/${tok}`, '') : k === 'content-security-policy' ? csp(v) : v]));
      expect(r.status).toBe(c.response.status);
      expect(strip(r.headers)).toEqual(want);
      if (c.response.body === null) expect(r.body.length).toBe(0);
      else if (c.response.body.file) expect(r.body.equals(readFileSync(join(dir, c.response.body.file)))).toBe(true);
      else expect(r.body.length).toBeGreaterThan(0);
    });
  }
});
