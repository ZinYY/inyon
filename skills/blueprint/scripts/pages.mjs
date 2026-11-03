// Inyon's list of pages, shared by new-site.mjs and publish.mjs. A site becomes a page in the Inyon app
// once its folder is in ~/.inyon/pages.json; the hub reads that file and serves the folder from disk.
//
// The rules here (slugs, which folders may be pages, the file formats) copy Inyon's own in
// crates/inyon-core/src/features/pages_core.rs. This skill ships on its own, so it imports nothing from Inyon;
// tests/publish.test.mjs checks them against fixtures/pages/rules.json when run inside the Inyon repo.
//
//   ~/.inyon/pages.json     {"v":1,"pages":[{"slug","name","path","project","by","added","session"?}]}
//   <site>/.inyon-page.json {"v":1,"slug","name","kind":"blueprint","created"}
//
// `added` and `created` are ms since 1970. pages.json is written to a temp file and renamed into place,
// while holding pages.json.lock (created with O_EXCL, taken over once it is 10 s old). It may be a
// symlink (a dotfile setup): it is read through the link, and written where the link leads.
import { closeSync, constants, existsSync, fstatSync, fsyncSync, linkSync, openSync, readFileSync, readlinkSync, readSync, realpathSync, renameSync, statSync, unlinkSync, writeSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { homedir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';

// The host id of the machine the hub runs on; app links are /pages/<host>/<slug>.
export const LOCAL_HOST = 'local';
export const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,62}$/;
export const isSlug = (s) => typeof s === 'string' && SLUG_RE.test(s);
export const BAD_SLUG = 'A slug is lowercase letters, digits and dashes, up to 63 characters, starting with a letter or digit';
export const NOT_ABSOLUTE = 'The folder must be an absolute path';
export const TOO_WIDE = "Pick the site's own folder, not / or your home folder";
export const PRIVATE_FOLDER = "Folders under ~/.ssh and ~/.inyon can't be pages";
export const NO_INDEX = 'That folder has no index.html';
export const LOCK_STALE_MS = 10_000;
// The most of pages.json this reads, as Inyon's hubs and CLI: a bigger file (a sparse one claiming
// terabytes too) is a list this can't read.
export const LIST_MAX = 4 * 1024 * 1024;
// A lock holds a process id and a tag; anything longer isn't one of ours.
const LOCK_MAX = 256;
// How long the git calls that find a folder's project may take, together, before git is killed. A
// named pipe in a folder's .git would otherwise keep it waiting for good.
export const GIT_TIMEOUT_MS = 3000;
// Set for those calls: no machine-wide git config, and no lock files taken just to look.
export const GIT_ENV = { GIT_CONFIG_NOSYSTEM: '1', GIT_OPTIONAL_LOCKS: '0' };

// ASCII letters lowercased, every other run one dash, no dash at either end, at most 63 characters.
export function slugify(text) {
  const s = String(text).replace(/[A-Z]/g, (c) => c.toLowerCase()).replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 63).replace(/-+$/, '');
  return s || 'page';
}

// base, then base-2, base-3 and so on (cut so the whole stays 63 characters), whichever is free first.
export function uniqueSlug(base, taken) {
  if (!taken(base)) return base;
  for (let i = 2; ; i++) {
    const suffix = `-${i}`;
    const s = base.slice(0, 63 - suffix.length).replace(/-+$/, '') + suffix;
    if (!taken(s)) return s;
  }
}

// What is wrong with a folder as a page, before the disk is asked. Null when it may be one.
export function sitePathProblem(p, home) {
  const win = /^[A-Za-z]:[\\/]/.test(p);
  if (!(p.startsWith('/') || win) || /\0/.test(p)) return NOT_ABSOLUTE;
  const parts = (s) => s.split(win ? /[\\/]+/ : /\/+/).filter(Boolean);
  const segs = parts(p);
  if (segs.some((s) => s === '.' || s === '..')) return NOT_ABSOLUTE;
  const homeSegs = parts(home);
  const same = (a, b) => a.length === b.length && a.every((s, i) => s === b[i]);
  if (segs.length === 0 || (win && segs.length === 1) || (homeSegs.length > 0 && same(segs, homeSegs))) return TOO_WIDE;
  const under = (dir) => { const d = [...homeSegs, dir]; return segs.length >= d.length && same(segs.slice(0, d.length), d); };
  if (homeSegs.length > 0 && (under('.ssh') || under('.inyon'))) return PRIVATE_FOLDER;
  return null;
}

// Where the list lives: always ~/.inyon, even when INYON_DATA_DIR moves the hub's other records, so
// the hub, the inyon command and this script all read and write the same file.
export const inyonDir = () => join(homedir(), '.inyon');

// Whether this machine runs an Inyon hub, judged as `inyon pages add` judges it: the hub's settings
// (hub.env in INYON_DATA_DIR or ~/.inyon), a node that runs one (hub.json), or a hub service. Without
// one, the page shows through the hub this machine joined, and there's no app link to print here.
export function hubHere(home = homedir(), env = process.env) {
  const data = env.INYON_DATA_DIR || join(home, '.inyon');
  return [
    join(data, 'hub.env'),
    join(home, '.inyon', 'node', 'hub.json'),
    join(home, '.tether-node', 'hub.json'),
    join(home, '.config', 'systemd', 'user', 'inyon-hub.service'),
    join(home, '.config', 'systemd', 'user', 'tether-hub.service'),
    join(home, 'Library', 'LaunchAgents', 'tech.abhashchakraborty.inyon.hub.plist'),
  ].some((f) => existsSync(f));
}

// Whether this machine is joined to a hub, judged as `inyon pages add` judges it: INYON_HUB in the
// environment or in the node's env file (~/.inyon/node/env, or ~/.tether-node from before the rename).
// A hub from 0.22 shows a joined machine's pages; with neither, no app can open them.
export function joinedHere(home = homedir(), env = process.env) {
  if ((env.INYON_HUB || env.TETHER_HUB || '').trim()) return true;
  for (const dir of [join(home, '.inyon', 'node'), join(home, '.tether-node')]) {
    for (const name of ['.env', 'env']) {
      let text = '';
      try { text = readFileSync(join(dir, name), 'utf8'); } catch { continue; }
      if (/^\s*(?:export\s+)?(?:INYON|TETHER)_HUB\s*=\s*['"]?[^\s'"]/m.test(text)) return true;
    }
  }
  return false;
}

// The line git prints for `git -C <dir> rev-parse <option...>`, or null. A line starting with '-' is
// refused: a git too old to know the option prints it back and still exits 0 (git before 2.31 does
// that with --path-format, before 2.5 with --git-common-dir). Git is killed after `timeout` ms.
export function revParse(dir, option, git = 'git', timeout = GIT_TIMEOUT_MS) {
  if (timeout <= 0) return null;
  try {
    const line = execFileSync(git, ['-C', dir, 'rev-parse', ...[option].flat()], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout, killSignal: 'SIGKILL', env: { ...process.env, ...GIT_ENV } }).split(/\r?\n/)[0].trim();
    return line && !line.startsWith('-') ? line : null;
  } catch { return null; }
}

// The top folder of the git work tree a folder is in, or null outside one.
export function gitTop(dir, git = 'git', timeout = GIT_TIMEOUT_MS) {
  const top = revParse(dir, '--show-toplevel', git, timeout);
  return top ? resolve(top) : null;
}

// The main working tree of the git repo a folder is in, when it is in one. In a linked worktree (an
// agent's .claude/worktrees/<name>, removed once its branch merges) that is the repo's own checkout,
// so a plan made there outlives the worktree. The common dir is then the main tree's .git; a
// submodule's (.git/modules/<name>) or a bare repo's (<name>.git) means the top folder stays.
// Two plain calls work on any git; the top folder git prints is kept when the common dir's parent is
// the same folder. Both calls together get GIT_TIMEOUT_MS.
export function mainTree(dir, git = 'git') {
  const until = Date.now() + GIT_TIMEOUT_MS;
  const top = gitTop(dir, git);
  if (!top) return null;
  const c = revParse(dir, '--git-common-dir', git, until - Date.now());
  const common = c ? resolve(dir, c) : null;
  if (!common || basename(common) !== '.git' || !isDir(common)) return top;
  const parent = dirname(common);
  try { if (realpathSync(parent) === realpathSync(top)) return top; } catch { return top; }
  return parent;
}

const isDir = (p) => { try { return statSync(p).isDirectory(); } catch { return false; } };

// The project a folder belongs to: its repo's main working tree (see mainTree), or the folder itself.
export function projectOf(dir) {
  return mainTree(dir) ?? dir;
}

// The agent running this script, from the variables each one sets for its shell.
export function agentName(env = process.env) {
  if (env.CLAUDECODE) return 'claude';
  if (env.CODEX_SANDBOX || env.CODEX_SANDBOX_NETWORK_DISABLED || env.CODEX_THREAD_ID) return 'codex';
  if (env.GEMINI_CLI) return 'gemini';
  if (env.OPENCODE) return 'opencode';
  return 'agent';
}

// The conversation of the agent running this script, so Inyon can show the page under that session's
// name: letters, digits and `._:-`, at most 128 of them, or nothing.
export function agentSession(env = process.env) {
  return [env.CLAUDE_CODE_SESSION_ID, env.CODEX_THREAD_ID].find((v) => typeof v === 'string' && /^[A-Za-z0-9._:-]{1,128}$/.test(v));
}

// A file's text, read without ever waiting on it: opened with O_NONBLOCK, and read only when the open
// file is a plain file of at most `max` bytes, into a buffer that grows as it reads (never sized from
// the file, which can claim terabytes it doesn't have). A FIFO left in a site's folder (or as the
// list) would otherwise hold the read until something wrote to it. A symlink at the end of the path
// is followed only when `follow` says so: the list may be one, a file in a site's folder may not.
// Throws the open's error, or a plain Error for anything else.
export function readPlain(file, max, follow = false) {
  const fd = openSync(file, constants.O_RDONLY | (follow ? 0 : (constants.O_NOFOLLOW ?? 0)) | (constants.O_NONBLOCK ?? 0));
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.size > max) throw new Error(`${file} isn't a plain file of a size this reads`);
    const parts = [];
    let got = 0;
    for (;;) {
      const buf = Buffer.alloc(Math.min(64 * 1024, max + 1 - got));
      const n = readSync(fd, buf, 0, buf.length, null);
      if (n === 0) break;
      parts.push(buf.subarray(0, n));
      got += n;
      if (got > max) throw new Error(`${file} is too big`);
    }
    return Buffer.concat(parts).toString('utf8');
  } finally { closeSync(fd); }
}

const readJson = (f, max) => JSON.parse(readPlain(f, max));

// The path a chain of symlinks ends at, read link by link so a link to a file not made yet still
// leads somewhere; the path itself when it isn't a link. Gives up after 40 links (a loop).
export function linkTarget(file) {
  let p = file;
  for (let i = 0; i < 40; i++) {
    let to;
    try { to = readlinkSync(p); } catch { return p; }
    p = resolve(dirname(p), to);
  }
  return p;
}
// A bigger `.inyon-page.json` isn't read for a page's name and slug.
const MARKER_MAX = 64 * 1024;

export function readMarker(site) {
  try {
    const m = readJson(join(site, '.inyon-page.json'), MARKER_MAX);
    return m && typeof m === 'object' && !Array.isArray(m) ? m : null;
  } catch { return null; }
}

// Writes <site>/.inyon-page.json, keeping its first `created`. Returns the marker.
export function writeMarker(site, { slug, name }, now = Date.now()) {
  const old = readMarker(site);
  const created = Number.isSafeInteger(old?.created) ? old.created : now;
  const m = { v: 1, slug, name, kind: 'blueprint', created };
  if (old && old.v === 1 && old.slug === slug && old.name === name && old.kind === 'blueprint' && old.created === created) return m;
  atomicWrite(join(site, '.inyon-page.json'), JSON.stringify(m) + '\n');
  return m;
}

// Write to a temp file in the same folder, flush it, then rename it over the target.
export function atomicWrite(file, text, mode = 0o644) {
  const tmp = `${file}.${process.pid}.${Math.random().toString(36).slice(2, 8)}.tmp`;
  const fd = openSync(tmp, 'wx', mode);
  try { writeSync(fd, text); fsyncSync(fd); } finally { closeSync(fd); }
  try { renameSync(tmp, file); } catch (e) { try { unlinkSync(tmp); } catch {} throw e; }
}

const pause = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// Runs fn while holding <file>.lock. A lock older than LOCK_STALE_MS was left by a writer that died
// and is taken over. The lock holds this process's id and a random tag, and is removed afterwards
// only while it still holds them. A failure other than "someone else has it" (a folder an agent's
// sandbox keeps read-only: EACCES, EPERM, EROFS) is thrown at once, never waited on.
export function withLock(file, fn, { waitMs = 15_000, write = writeSync } = {}) {
  const lock = `${file}.lock`;
  const until = Date.now() + waitMs;
  const mine = `${process.pid} ${randomBytes(8).toString('hex')}\n`;
  for (;;) {
    try {
      const fd = openSync(lock, 'wx', 0o600);
      try {
        if (write(fd, mine) !== Buffer.byteLength(mine)) throw new Error(`short write to ${lock}`);
      } catch (e) {
        // a lock without this writer's tag (a full disk) could never be released: take it away again
        dropMade(lock, fd);
        throw e;
      } finally { closeSync(fd); }
      break;
    } catch (e) {
      if (e.code !== 'EEXIST') throw e;
    }
    if (takeOverStale(lock)) continue;
    if (Date.now() > until) throw Object.assign(new Error(`${lock} is held by another writer; try again, or delete it if nothing is running`), { code: 'ELOCKED' });
    pause(25 + Math.random() * 50);
  }
  // read as the list is, so a FIFO put in its place isn't waited on
  try { return fn(); } finally { try { if (readPlain(lock, LOCK_MAX) === mine) unlinkSync(lock); } catch {} }
}

// Removes the lock this writer made as `fd`, while the path still leads to that file.
function dropMade(lock, fd) {
  try {
    const made = fstatSync(fd), now = statSync(lock);
    if (now.dev === made.dev && now.ino === made.ino) unlinkSync(lock);
  } catch {}
}

// Removes a lock left by a writer that died; true when the caller should try to take it again at once.
function takeOverStale(lock) {
  let old;
  try { old = statSync(lock); } catch (e) { if (e.code === 'ENOENT') return true; throw e; }
  if (Date.now() - old.mtimeMs <= LOCK_STALE_MS) return false;
  removeJudged(lock, old);
  return true;
}

// Removes the lock that was judged stale as `old`, and only that one: renamed aside, then put back if
// it turns out to be a new lock another writer made since (two writers can judge one lock stale at
// once, and the first makes its own before the second acts).
export function removeJudged(lock, old) {
  const aside = `${lock}.${process.pid}.${randomBytes(4).toString('hex')}.stale`;
  try { renameSync(lock, aside); } catch (e) { if (e.code === 'ENOENT') return; throw e; }
  const got = statSync(aside);
  if (got.ino !== old.ino || got.mtimeMs !== old.mtimeMs) {
    // another writer's new lock: back where it was, unless a third has made one since
    try { linkSync(aside, lock); } catch {}
  }
  try { unlinkSync(aside); } catch {}
}

export function readPages(file) {
  if (!existsSync(file)) return { v: 1, pages: [] };
  let text, d;
  try { text = readPlain(file, LIST_MAX, true); } catch { throw Object.assign(new Error(`${file} isn't a plain file of at most 4 MB; fix it or move it away, then publish again`), { code: 'EBADLIST' }); }
  try { d = JSON.parse(text); } catch { throw Object.assign(new Error(`${file} isn't valid JSON; fix it or move it away, then publish again`), { code: 'EBADLIST' }); }
  if (!d || typeof d !== 'object' || d.v !== 1 || !Array.isArray(d.pages)) throw Object.assign(new Error(`${file} isn't a version 1 list of pages; update this skill or Inyon`), { code: 'EBADLIST' });
  return d;
}

// Adds a site to the machine's list, or updates its entry when the folder is already there, so
// publishing again is safe. Returns { page, marker, file, changed }; marker is null when the site's
// folder couldn't be written.
//   opts: { name?, slug?, by?, now?, home? }
export function publish(dir, opts = {}) {
  const home = opts.home ?? homedir();
  const now = opts.now ?? Date.now();
  const path = resolve(dir);
  const fail = (msg, code = 'EINVAL') => { throw Object.assign(new Error(msg), { code }); };
  const problem = sitePathProblem(path, home);
  if (problem) fail(problem);
  let real;
  try { real = realpathSync(path); } catch { fail(`${path} doesn't exist`, 'ENOENT'); }
  const realProblem = sitePathProblem(real, (() => { try { return realpathSync(home); } catch { return home; } })());
  if (realProblem) fail(realProblem);
  const data = inyonDir();
  const inside = (p, d) => p === d || p.startsWith(d + '/');
  if (inside(path, data) || inside(real, data) || (existsSync(data) && inside(real, realpathSync(data)))) fail(PRIVATE_FOLDER);
  let index = null;
  try { index = statSync(join(real, 'index.html')); } catch {}
  if (!index?.isFile()) fail(NO_INDEX);
  if (opts.slug !== undefined && !isSlug(opts.slug)) fail(BAD_SLUG);
  if (!existsSync(data)) fail(`Inyon isn't set up on this machine (no ${data})`, 'ENOINYON');

  const found = readMarker(path);
  const file = join(data, 'pages.json');
  return withLock(file, () => {
    const list = readPages(file);
    const pages = list.pages;
    // the folder as listed by any writer, under this spelling or another that leads to it (a symlink)
    const realOf = (p) => { try { return realpathSync(p); } catch { return null; } };
    const at = pages.findIndex((p) => p && typeof p.path === 'string' && (p.path === path || realOf(p.path) === real));
    const old = at >= 0 ? pages[at] : null;
    const name = (opts.name ?? '').trim() || old?.name || (typeof found?.name === 'string' && found.name.trim()) || basename(path);
    const takenBy = (slug) => pages.find((p, i) => i !== at && p && p.slug === slug);
    let slug;
    if (opts.slug !== undefined) {
      const other = takenBy(opts.slug);
      if (other) fail(`The slug "${opts.slug}" is taken by ${other.path}; pick another with --slug`, 'ETAKEN');
      slug = opts.slug;
    } else if (old && isSlug(old.slug)) slug = old.slug;
    else {
      const base = isSlug(found?.slug) ? found.slug : slugify(basename(path));
      slug = uniqueSlug(base, (s) => !!takenBy(s));
    }
    const page = {
      ...(old || {}),
      slug,
      name,
      // an entry found under another spelling keeps the one it was listed with, as the inyon command does
      path: typeof old?.path === 'string' ? old.path : path,
      project: old?.project || projectOf(path),
      by: old?.by || opts.by || agentName(),
      added: Number.isSafeInteger(old?.added) ? old.added : now,
    };
    // the latest conversation to add it, as the inyon command does
    const session = agentSession();
    if (session) page.session = session;
    const changed = !old || JSON.stringify(old) !== JSON.stringify(page);
    if (changed) {
      if (at >= 0) pages[at] = page; else pages.push(page);
      // through a symlinked list to its file, so the link stays
      atomicWrite(linkTarget(file), JSON.stringify(list, null, 2) + '\n', 0o600);
    }
    // The marker helps Inyon find the site again later; a folder it can't write to still gets listed.
    let marker = null;
    try { marker = writeMarker(path, page, now); } catch {}
    return { page, marker, file, changed };
  });
}
