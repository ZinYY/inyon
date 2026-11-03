// Shared helpers for the Blueprint scripts: find Playwright and a Chromium, optional sharp, a static server.
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { createReadStream, existsSync } from 'node:fs';
import { realpath, stat } from 'node:fs/promises';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Playwright comes from, in order: BLUEPRINT_PLAYWRIGHT (a folder whose node_modules has it), this folder,
// or the current project. Install it once with `bun add -d playwright` or `npm i -D playwright` anywhere.
function req(name) {
  const tries = [process.env.BLUEPRINT_PLAYWRIGHT, fileURLToPath(new URL('.', import.meta.url)), process.cwd()].filter(Boolean);
  for (const dir of tries) {
    try { return createRequire(join(resolve(dir), 'package.json'))(name); } catch {}
  }
  return null;
}
export function chromium() {
  const pw = req('playwright') || req('playwright-core');
  if (!pw) { console.error('Playwright not found. Run `bun add -d playwright` (or npm) in this folder or your project, or set BLUEPRINT_PLAYWRIGHT.'); process.exit(2); }
  return pw.chromium;
}
// Browsers already on the machine, tried when Playwright has none of its own downloaded.
const SYSTEM_BROWSERS = {
  win32: ['C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Google/Chrome/Application/chrome.exe'],
  darwin: ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge', '/Applications/Chromium.app/Contents/MacOS/Chromium'],
  linux: ['/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome', '/usr/bin/microsoft-edge'],
};
// CHROME_PATH picks a browser binary; otherwise Playwright's own download (`bunx playwright install
// chromium`), then Edge or Chrome where they usually live.
export async function launch() {
  const opts = { args: ['--disable-gpu'] };
  if (process.env.CHROME_PATH) return chromium().launch({ ...opts, executablePath: process.env.CHROME_PATH });
  try {
    return await chromium().launch(opts);
  } catch (e) {
    if (!/Executable doesn't exist/i.test(String(e?.message))) throw e;
    const found = (SYSTEM_BROWSERS[process.platform] || []).find((p) => existsSync(p));
    if (found) return chromium().launch({ ...opts, executablePath: found });
    console.error('No browser for Playwright. Run `bunx playwright install chromium`, or set CHROME_PATH to Chrome or Edge.');
    process.exit(2);
  }
}

// Loads every lazy image now and waits for them, so a full-page shot has no blank pictures.
export async function loadImages(page) {
  await page.evaluate(async () => {
    for (const img of document.querySelectorAll('img[loading="lazy"]')) img.loading = 'eager';
    await Promise.all([...document.images].map((img) => (img.complete ? null : new Promise((r) => { img.addEventListener('load', r, { once: true }); img.addEventListener('error', r, { once: true }); setTimeout(r, 5000); }))));
  });
}
export const sharp = () => req('sharp');

// ---------- the static server ----------
// serve() answers the way Inyon's hub answers /v/<token>/ for a page, so a site that works here works in
// Inyon: the same headers in the same order, the same sandbox, a 301 that adds a folder's slash, ETags,
// no folder listings and no dot files. The header list and the rules are copied from Inyon's
// crates/inyon-core/src/features/pages_core.rs (view_headers, view_csp, decide_view); fixtures/pages/responses.json
// in the Inyon repo pins them, and tests/serve.test.mjs replays those cases against this server. Change
// them together. The one difference: there is no token here, so the site's root is the folder.
const TYPES = {
  html: 'text/html;charset=utf-8', htm: 'text/html;charset=utf-8', js: 'text/javascript;charset=utf-8', mjs: 'text/javascript;charset=utf-8',
  css: 'text/css;charset=utf-8', json: 'application/json;charset=utf-8', map: 'application/json;charset=utf-8', webmanifest: 'application/manifest+json',
  svg: 'image/svg+xml', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', avif: 'image/avif', ico: 'image/x-icon',
  woff2: 'font/woff2', woff: 'font/woff', ttf: 'font/ttf', otf: 'font/otf', txt: 'text/plain;charset=utf-8', md: 'text/plain;charset=utf-8',
  xml: 'application/xml', wasm: 'application/wasm', mp3: 'audio/mpeg', mp4: 'video/mp4', webm: 'video/webm', pdf: 'application/pdf',
};
export const contentType = (name) => { const i = name.lastIndexOf('.'); return TYPES[i < 0 ? '' : name.slice(i + 1).toLowerCase()] ?? 'application/octet-stream'; };
const SANDBOX = 'sandbox allow-scripts allow-forms allow-popups allow-downloads';
// The origin a request reached us on, from a Host header that is a plain host[:port]; null otherwise.
export const originOf = (host) => (host && /^([A-Za-z0-9.-]+|\[[0-9A-Fa-f:.]+\])(:\d{1,5})?$/.test(host) ? `http://${host.toLowerCase()}` : null);
// With an origin the page may load only from <origin>/ (in Inyon: <origin>/v/<token>/); without one, nothing but inline styles.
// There is no frame-ancestors, here or in Inyon: the sandbox gives every page an opaque origin, and a
// browser matches no frame-ancestors source (even *) against an opaque parent, which would break the
// viewer framing screens.
export function siteCsp(origin) {
  const tail = "object-src 'none'; base-uri 'none'; form-action 'none'";
  if (!origin) return `${SANDBOX}; default-src 'none'; style-src 'unsafe-inline'; ${tail}`;
  const b = `${origin}/`;
  return [SANDBOX, "default-src 'none'", `script-src ${b} 'unsafe-inline'`, `style-src ${b} 'unsafe-inline'`, `img-src ${b} data: blob:`,
    `font-src ${b} data:`, `media-src ${b}`, `connect-src ${b}`, `frame-src ${b}`, tail].join('; ');
}
// Every header an answer carries, lowercase and in this order, for every status.
export function siteHeaders({ origin, cache, type, etag, location, allow }) {
  const h = [['content-security-policy', siteCsp(origin)], ['access-control-allow-origin', '*'], ['referrer-policy', 'no-referrer'], ['x-content-type-options', 'nosniff'], ['x-dns-prefetch-control', 'off'], ['cache-control', cache]];
  if (type) h.push(['content-type', type]);
  if (etag) h.push(['etag', etag]);
  if (location) h.push(['location', location]);
  if (allow) h.push(['allow', allow]);
  return h;
}
const etagOf = (size, mtimeMs) => `"${size.toString(16)}-${Math.floor(mtimeMs).toString(16)}"`;
function etagMatches(inm, etag) {
  if (!inm) return false;
  if (inm.trim() === '*') return true;
  return inm.split(',').some((t) => t.trim().replace(/^W\//, '') === etag);
}
const keptQuery = (q) => (q && /^[\x21\x24-\x26\x28-\x3b\x3d\x3f-\x7e]+$/.test(q) ? `?${q}` : '');
const ONE_DOT = /^(\.|%2e)$/i, TWO_DOTS = /^(\.|%2e)(\.|%2e)$/i;
// Dot segments resolved the way a browser's URL parser does (`\\` counts as `/`); the query split off unchanged.
export function normalizeTarget(target) {
  const q = target.indexOf('?');
  const raw = (q < 0 ? target : target.slice(0, q)).replace(/\\/g, '/');
  const segs = raw.split('/').slice(1), out = [];
  segs.forEach((seg, i) => {
    const last = i === segs.length - 1;
    if (TWO_DOTS.test(seg)) { out.pop(); if (last) out.push(''); } else if (ONE_DOT.test(seg)) { if (last) out.push(''); } else out.push(seg);
  });
  return { path: `/${out.join('/')}`, query: q < 0 ? null : target.slice(q + 1) };
}
// The decoded names in a path, refusing empty names, names starting with a dot, and encoded slashes or NULs.
function segmentsOf(path) {
  const body = path.slice(1), dir = body === '' || body.endsWith('/');
  const parts = body === '' ? [] : (dir ? body.slice(0, -1) : body).split('/');
  const segments = [];
  for (const part of parts) {
    let name; try { name = decodeURIComponent(part); } catch { return null; }
    if (name === '' || name.startsWith('.') || /[/\\\0]/.test(name)) return null;
    segments.push(name);
  }
  return { segments, dir };
}
function within(root, real) {
  const rel = relative(root, real);
  if (rel === '') return true;
  if (isAbsolute(rel)) return false;
  return rel.split(/[\\/]/).every((s) => s !== '..' && !s.startsWith('.'));
}
const ERRORS = { 404: ['Not found', 'This site has no file at that address.'], 405: ['Read only', 'This site can be read, nothing else.'], 421: ['Wrong address', "This server doesn't answer to that address."] };
const errorPage = (status) => { const [t, x] = ERRORS[status]; return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${t}</title><style>body{font:15px/1.5 system-ui,sans-serif;margin:0;display:grid;place-items:center;min-height:100vh;background:#f4f3f0;color:#1d1c1a}@media(prefers-color-scheme:dark){body{background:#1b1a18;color:#e9e8e4}}main{max-width:28em;padding:24px}h1{font-size:20px;margin:0 0 6px}p{margin:0;opacity:.75}</style></head><body><main><h1>${t}</h1><p>${x}</p></main></body></html>`; };
const fileStat = async (p) => { try { return await stat(p); } catch { return null; } };
const real = async (p) => { try { return await realpath(p); } catch { return null; } };

// What to answer: { status, headers, file?, body? }. Exported for the tests.
export async function decide(rootDir, { method, url, host, ifNoneMatch }) {
  const origin = originOf(host);
  const fail = (status, extra = {}) => ({ status, headers: siteHeaders({ origin: status === 421 ? null : origin, cache: 'no-store', type: 'text/html;charset=utf-8', ...extra }), body: errorPage(status) });
  if (!origin) return fail(421);
  if (method !== 'GET' && method !== 'HEAD') return fail(405, { allow: 'GET, HEAD' });
  if (!url.startsWith('/')) return fail(404);
  const { path, query } = normalizeTarget(url);
  const redirect = (segments) => ({ status: 301, headers: siteHeaders({ origin, cache: 'no-store', location: `/${segments.map((s) => `${encodeURIComponent(s)}/`).join('')}${keptQuery(query)}` }), body: '' });
  const plan = segmentsOf(path);
  if (!plan) return fail(404);
  const root = await real(rootDir);
  if (root === null) return fail(404);
  let f = await real(join(root, ...plan.segments));
  if (f === null || !within(root, f)) return fail(404);
  let st = await fileStat(f);
  let name = plan.segments.at(-1) ?? 'index.html';
  if (st?.isDirectory()) {
    if (!plan.dir) return redirect(plan.segments);
    f = await real(join(f, 'index.html'));
    if (f === null || !within(root, f)) return fail(404);
    st = await fileStat(f);
    name = 'index.html';
  } else if (plan.dir) return fail(404);
  if (!st?.isFile()) return fail(404);
  const type = contentType(name);
  if (name === 'progress.json') return { status: 200, headers: siteHeaders({ origin, cache: 'no-store', type }), file: f, size: st.size };
  const etag = etagOf(st.size, st.mtimeMs);
  if (etagMatches(ifNoneMatch, etag)) return { status: 304, headers: siteHeaders({ origin, cache: 'no-cache', etag }), body: '' };
  return { status: 200, headers: siteHeaders({ origin, cache: 'no-cache', type, etag }), file: f, size: st.size };
}

// A small static server for a site folder, answering like Inyon does (see above).
export function serve(root, { host = '127.0.0.1', port = 0 } = {}) {
  root = resolve(root);
  const srv = createServer(async (rq, rs) => {
    try {
      const d = await decide(root, { method: rq.method, url: rq.url, host: rq.headers.host, ifNoneMatch: rq.headers['if-none-match'] });
      const headers = Object.fromEntries(d.headers);
      if (d.file) {
        rs.writeHead(d.status, { ...headers, 'content-length': d.size });
        if (rq.method === 'HEAD') return rs.end();
        createReadStream(d.file).on('error', () => rs.destroy()).pipe(rs);
      } else {
        const body = Buffer.from(d.body);
        rs.writeHead(d.status, d.status === 304 ? headers : { ...headers, 'content-length': body.length });
        rs.end(rq.method === 'HEAD' ? undefined : body);
      }
    } catch { if (!rs.headersSent) rs.writeHead(500); rs.end(); }
  });
  return new Promise((ok) => srv.listen(port, host, () => ok({ url: `http://${host.includes(':') ? `[${host}]` : host}:${srv.address().port}/`, close: () => srv.close() })));
}
