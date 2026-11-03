// bun test skills/blueprint/tests
// publish.mjs and new-site.mjs, run with node the way an agent runs them, against a made-up home folder.
import { afterAll, beforeEach, describe, expect, test } from 'bun:test';
import { spawn, spawnSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync, truncateSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BAD_SLUG, GIT_TIMEOUT_MS, LIST_MAX, NO_INDEX, PRIVATE_FOLDER, TOO_WIDE, agentName, agentSession, hubHere, isSlug, joinedHere, mainTree, removeJudged, sitePathProblem, slugify, uniqueSlug, withLock } from '../scripts/pages.mjs';

const scripts = fileURLToPath(new URL('../scripts/', import.meta.url));
const tmp = mkdtempSync(join(tmpdir(), 'bp-publish-'));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

let home, n = 0;
const env = (extra = {}) => ({ PATH: process.env.PATH, HOME: home, ...extra });
const run = (script, args, extra) => spawnSync('node', [join(scripts, script), ...args], { env: env(extra), encoding: 'utf8' });
const pub = (args, extra) => run('publish.mjs', args, extra);
const list = () => JSON.parse(readFileSync(join(home, '.inyon/pages.json'), 'utf8'));
const marker = (dir) => JSON.parse(readFileSync(join(dir, '.inyon-page.json'), 'utf8'));
function site(rel, { index = true } = {}) {
  const dir = join(home, rel);
  mkdirSync(dir, { recursive: true });
  if (index) writeFileSync(join(dir, 'index.html'), '<!doctype html><title>x</title>');
  return dir;
}
// A FIFO at `p`, and a writer that opens it after `secs` (2 s) and closes it at once: a script that
// opened it as a plain file waits that long, instead of hanging the test for good. Kill the writer
// at the end.
function fifo(p, secs = 2) {
  rmSync(p, { force: true });
  expect(spawnSync('mkfifo', [p]).status).toBe(0);
  return spawn('sh', ['-c', `sleep ${secs}; : > "$1"`, 'sh', p], { stdio: 'ignore' });
}
const git = (dir, ...args) => spawnSync('git', ['-C', dir, ...args], { encoding: 'utf8', env: { ...env(), GIT_CONFIG_GLOBAL: '/dev/null' } });

beforeEach(() => {
  home = join(tmp, `home${++n}`);
  mkdirSync(join(home, '.inyon'), { recursive: true });
  // a hub runs on this made-up machine unless a test says otherwise
  writeFileSync(join(home, '.inyon/hub.env'), '');
});

describe('publish.mjs', () => {
  test('adds a valid entry and a marker', () => {
    const repo = join(home, 'proj');
    mkdirSync(repo);
    git(repo, 'init', '-q');
    const dir = site('proj/private/plans/inyon-plan');
    const before = Date.now();
    const r = pub([dir, '--name', 'Inyon plan', '--by', 'codex']);
    expect(r.status).toBe(0);
    expect(r.stdout).toContain('"Inyon plan" is in Inyon: Pages > ');
    expect(r.stdout).toContain('> proj');
    expect(r.stdout).toContain('/pages/local/inyon-plan');
    expect(r.stdout).not.toContain('/v/');
    const d = list();
    expect(d.v).toBe(1);
    expect(d.pages).toHaveLength(1);
    const p = d.pages[0];
    expect(Object.keys(p).sort()).toEqual(['added', 'by', 'name', 'path', 'project', 'slug']);
    expect(p).toMatchObject({ slug: 'inyon-plan', name: 'Inyon plan', path: dir, project: repo, by: 'codex' });
    expect(Number.isSafeInteger(p.added) && p.added >= before && p.added <= Date.now()).toBe(true);
    const m = marker(dir);
    expect(m).toEqual({ v: 1, slug: 'inyon-plan', name: 'Inyon plan', kind: 'blueprint', created: m.created });
    expect(Number.isSafeInteger(m.created)).toBe(true);
    expect(statSync(join(home, '.inyon/pages.json')).mode & 0o777).toBe(0o600);
    expect(existsSync(join(home, '.inyon/pages.json.lock'))).toBe(false);
  });

  test('a site in a linked worktree belongs to the main checkout, not the worktree', () => {
    const repo = join(home, 'main');
    mkdirSync(repo);
    git(repo, 'init', '-q');
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'x');
    const wt = join(repo, '.claude/worktrees/feature');
    expect(git(repo, 'worktree', 'add', '-q', wt).status).toBe(0);
    const dir = site('main/.claude/worktrees/feature/private/plans/plan');
    const r = pub([dir, '--name', 'Plan']);
    expect(r.status).toBe(0);
    expect(r.stdout).toContain(' > main\n');
    expect(list().pages[0]).toMatchObject({ path: dir, project: repo });
  });

  test('outside a git repo, the project is the folder itself', () => {
    const dir = site('handy-redesign');
    expect(pub([dir]).status).toBe(0);
    expect(list().pages[0]).toMatchObject({ slug: 'handy-redesign', name: 'handy-redesign', project: dir });
  });

  test('publishing again changes nothing', () => {
    const dir = site('a');
    expect(pub([dir, '--name', 'A plan']).status).toBe(0);
    const file = readFileSync(join(home, '.inyon/pages.json'), 'utf8');
    const m = readFileSync(join(dir, '.inyon-page.json'), 'utf8');
    const again = pub([dir]);
    expect(again.status).toBe(0);
    expect(again.stdout).toContain('already listed');
    expect(readFileSync(join(home, '.inyon/pages.json'), 'utf8')).toBe(file);
    expect(readFileSync(join(dir, '.inyon-page.json'), 'utf8')).toBe(m);
  });

  test('a new name updates the entry and keeps its slug and date', () => {
    const dir = site('a');
    pub([dir, '--name', 'First']);
    const first = list().pages[0];
    expect(pub([dir, '--name', 'Second']).status).toBe(0);
    const d = list();
    expect(d.pages).toHaveLength(1);
    expect(d.pages[0]).toEqual({ ...first, name: 'Second' });
    expect(marker(dir)).toMatchObject({ slug: first.slug, name: 'Second' });
  });

  test('a slug is made unique, and an explicit one may not be taken', () => {
    const a = site('one/plan'), b = site('two/plan'), c = site('three/plan');
    pub([a, '--name', 'Plan']);
    pub([b, '--name', 'Plan']);
    expect(list().pages.map((p) => p.slug)).toEqual(['plan', 'plan-2']);
    const r = pub([c, '--slug', 'plan']);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(`taken by ${a}`);
    expect(list().pages).toHaveLength(2);
    expect(pub([c, '--slug', 'plan-three']).status).toBe(0);
    expect(list().pages[2].slug).toBe('plan-three');
  });

  test("the marker's slug is used for a site that isn't listed yet", () => {
    const dir = site('m');
    writeFileSync(join(dir, '.inyon-page.json'), JSON.stringify({ v: 1, slug: 'from-marker', name: 'Marker name', kind: 'blueprint', created: 5 }));
    expect(pub([dir]).status).toBe(0);
    expect(list().pages[0]).toMatchObject({ slug: 'from-marker', name: 'Marker name' });
    expect(marker(dir).created).toBe(5);
  });

  test('a FIFO as the marker or as the list is skipped at once, not read until something writes to it', () => {
    const dir = site('piped');
    let writer = fifo(join(dir, '.inyon-page.json'));
    try {
      const at = Date.now();
      expect(pub([dir]).status).toBe(0);
      expect(Date.now() - at).toBeLessThan(1500);
      expect(list().pages[0].slug).toBe('piped');
    } finally { writer.kill(); }
    writer = fifo(join(home, '.inyon/pages.json'));
    try {
      const at = Date.now();
      const r = pub([site('other')]);
      expect(Date.now() - at).toBeLessThan(1500);
      expect([r.status, r.stderr.includes("isn't a plain file of at most 4 MB")]).toEqual([1, true]);
    } finally { writer.kill(); }
  });

  test('a list bigger than the cap is a broken list at once, a sparse one claiming terabytes too', () => {
    const file = join(home, '.inyon/pages.json');
    for (const size of [4 * 1024 ** 4, LIST_MAX + 1]) {
      writeFileSync(file, '{"v":1,"pages":[]}');
      truncateSync(file, size);
      const at = Date.now();
      const r = pub([site(`big-${size}`)]);
      expect(Date.now() - at).toBeLessThan(1500);
      expect([size, r.status, r.stderr.includes("isn't a plain file of at most 4 MB")]).toEqual([size, 1, true]);
      expect(statSync(file).size).toBe(size);
    }
    // one at the cap is still read
    const text = '{"v":1,"pages":[]}';
    writeFileSync(file, text + ' '.repeat(LIST_MAX - text.length));
    expect(pub([site('at-cap')]).status).toBe(0);
    expect(list().pages.map((p) => p.slug)).toEqual(['at-cap']);
  });

  test('a symlinked list (a dotfile setup) is read and written through: the link stays, its file changes', () => {
    const dotfiles = join(home, 'dotfiles');
    mkdirSync(dotfiles);
    const file = join(home, '.inyon/pages.json');
    // a relative link to a file not made yet, as a dotfile manager can leave it
    symlinkSync('../dotfiles/pages.json', file);
    const a = site('a'), b = site('b');
    expect(pub([a]).status).toBe(0);
    expect(pub([b]).status).toBe(0);
    expect(lstatSync(file).isSymbolicLink()).toBe(true);
    expect(JSON.parse(readFileSync(join(dotfiles, 'pages.json'), 'utf8')).pages.map((p) => p.path)).toEqual([a, b]);
    expect(readdirSync(dotfiles)).toEqual(['pages.json']);
  });

  test('git that never answers (a named pipe as .git/HEAD) is stopped after 3 s, and the folder is the project', () => {
    const dir = site('stuck');
    mkdirSync(join(dir, '.git/objects'), { recursive: true });
    mkdirSync(join(dir, '.git/refs'));
    // a writer late enough that a script waiting on git fails the test instead of hanging it
    const writer = fifo(join(dir, '.git/HEAD'), 8);
    try {
      const at = Date.now();
      const r = pub([dir]);
      const ms = Date.now() - at;
      expect(r.status).toBe(0);
      expect(ms).toBeGreaterThan(GIT_TIMEOUT_MS - 200);
      expect(ms).toBeLessThan(GIT_TIMEOUT_MS + 1500);
      expect(list().pages[0]).toMatchObject({ path: dir, project: dir });
    } finally { writer.kill(); }
  }, 15_000);

  test('bad slugs and folders are refused, and the list is left alone', () => {
    const ok = site('fine');
    pub([ok]);
    const before = readFileSync(join(home, '.inyon/pages.json'), 'utf8');
    const cases = [
      [[ok, '--slug', 'Bad Slug'], BAD_SLUG],
      [[ok, '--slug', '-x'], BAD_SLUG],
      [[site('noindex', { index: false })], NO_INDEX],
      [[home], TOO_WIDE],
      [['/'], TOO_WIDE],
      [[site('.ssh/site')], PRIVATE_FOLDER],
      [[site('.inyon/site')], PRIVATE_FOLDER],
      [[join(home, 'missing')], "doesn't exist"],
    ];
    mkdirSync(join(home, '.ssh/keys'), { recursive: true });
    writeFileSync(join(home, '.ssh/keys/index.html'), 'x');
    symlinkSync(join(home, '.ssh/keys'), join(home, 'sneaky'));
    cases.push([[join(home, 'sneaky')], PRIVATE_FOLDER]);
    for (const [args, msg] of cases) {
      const r = pub(args);
      expect([args[0], r.status]).toEqual([args[0], 1]);
      expect(r.stderr).toContain(msg);
    }
    expect(readFileSync(join(home, '.inyon/pages.json'), 'utf8')).toBe(before);
  });

  test('a relative folder is taken from the current folder', () => {
    const dir = site('rel/site');
    const r = spawnSync('node', [join(scripts, 'publish.mjs'), 'site'], { cwd: join(home, 'rel'), env: env(), encoding: 'utf8' });
    expect(r.status).toBe(0);
    expect(list().pages[0].path).toBe(dir);
  });

  test('without ~/.inyon it says to serve the site instead, and writes nothing', () => {
    rmSync(join(home, '.inyon'), { recursive: true });
    const r = pub([site('s')]);
    expect(r.status).toBe(3);
    expect(r.stderr).toContain('serve.mjs');
    expect(existsSync(join(home, '.inyon'))).toBe(false);
  });

  test('the list stays in ~/.inyon with INYON_DATA_DIR set, where the hub and the CLI read it', () => {
    const data = join(home, 'data');
    mkdirSync(data);
    writeFileSync(join(data, 'hub.env'), '');
    expect(pub([site('s')], { INYON_DATA_DIR: data }).status).toBe(0);
    expect(list().pages).toHaveLength(1);
    expect(existsSync(join(data, 'pages.json'))).toBe(false);
  });

  test.skipIf(process.getuid?.() === 0)('when ~/.inyon is read-only it prints the command to run', () => {
    const dir = site('sandboxed');
    chmodSync(join(home, '.inyon'), 0o500);
    try {
      const r = pub([dir, '--name', "Bob's plan"]);
      expect(r.status).toBe(4);
      // without the program, this script with node, for the user's own shell outside the sandbox
      expect(r.stderr).toContain(`\n  node ${join(scripts, 'publish.mjs')} ${dir} --name 'Bob'\\''s plan' --by agent\n`);
      expect(r.stderr).not.toContain('inyon pages add');
    } finally { chmodSync(join(home, '.inyon'), 0o700); }
    // with the program installed, by its full path: it works where inyon isn't on the PATH
    mkdirSync(join(home, '.inyon/bin'));
    writeFileSync(join(home, '.inyon/bin/inyon'), '');
    chmodSync(join(home, '.inyon'), 0o500);
    try {
      const r = pub([dir]);
      expect(r.status).toBe(4);
      expect(r.stderr).toContain(`\n  ${home}/.inyon/bin/inyon pages add ${dir}\n`);
    } finally { chmodSync(join(home, '.inyon'), 0o700); }
  });

  test('the printed commands name the scripts by their file path, spaces and all', () => {
    // a copy of the skill in a folder with a space: a file: URL's pathname would say my%20skills
    const copy = join(home, 'my skills', 'blueprint');
    cpSync(join(scripts, '..'), copy, { recursive: true, filter: (f) => !f.includes('node_modules') });
    const at = (f) => `'${join(copy, 'scripts', f)}'`;
    const p = (args) => spawnSync('node', [join(copy, 'scripts/publish.mjs'), ...args], { env: env({ CLAUDECODE: '1' }), encoding: 'utf8' });
    const dir = site('spaced');
    rmSync(join(home, '.inyon/hub.env'));
    const r6 = p([dir]);
    expect(r6.status).toBe(6);
    expect(r6.stdout).toContain(`node ${at('serve.mjs')} ${dir} --host <private-ip>`);
    if (process.getuid?.() !== 0) {
      chmodSync(join(home, '.inyon'), 0o500);
      try {
        const r4 = p([site('spaced2')]);
        expect(r4.status).toBe(4);
        expect(r4.stderr).toContain(`\n  node ${at('publish.mjs')} ${join(home, 'spaced2')} --by claude\n`);
      } finally { chmodSync(join(home, '.inyon'), 0o700); }
    }
    rmSync(join(home, '.inyon'), { recursive: true });
    const r3 = p([dir]);
    expect(r3.status).toBe(3);
    expect(r3.stderr).toContain(`node ${at('serve.mjs')} ${dir} --host <private-ip>`);
    expect(r3.stdout + r3.stderr + r6.stdout + r6.stderr).not.toContain('%20');
    // new-site.mjs copies the template from there too, and prints the same paths
    const made = spawnSync('node', [join(copy, 'scripts/new-site.mjs'), join(home, 'fresh'), '--name', 'Fresh'], { env: env(), encoding: 'utf8' });
    expect(made.status).toBe(0);
    expect(existsSync(join(home, 'fresh/index.html'))).toBe(true);
    expect(made.stdout).toContain(`node ${join(copy, 'scripts/publish.mjs')} `);
    expect(made.stdout).not.toContain('%20');
  });

  test('with no hub and none joined it lists the page, says nothing can show it, and exits 6', () => {
    rmSync(join(home, '.inyon/hub.env'));
    const dir = site('alone');
    const r = pub([dir, '--name', 'Paired only']);
    expect(r.status).toBe(6);
    expect(r.stdout).toContain(`"Paired only" is listed on `);
    expect(r.stdout).toContain("isn't joined to one, so the app can't open this page");
    expect(r.stdout).not.toContain('joined to shows');
    expect(r.stdout).toContain(`serve.mjs ${dir} --host <private-ip>`);
  });

  test('on a machine joined to a hub it lists the page, says that hub shows it, and exits 5', () => {
    rmSync(join(home, '.inyon/hub.env'));
    mkdirSync(join(home, '.inyon/node'), { recursive: true });
    writeFileSync(join(home, '.inyon/node/env'), 'INYON_HUB=https://hub.example.ts.net\nINYON_TOKEN=t\n');
    const dir = site('nohub');
    const r = pub([dir, '--name', 'Laptop plan']);
    rmSync(join(home, '.inyon/node/env'));
    expect(r.status).toBe(5);
    expect(r.stdout).toContain(`"Laptop plan" is in Inyon: Pages > `);
    expect(r.stdout).toContain("This machine doesn't run a hub. The hub it's joined to shows its pages (hub 0.22 or later).");
    expect(r.stdout).toContain(`serve.mjs ${dir} --host <private-ip>`);
    expect(r.stdout).not.toContain('/pages/local/');
    expect(list().pages[0]).toMatchObject({ slug: 'nohub', name: 'Laptop plan' });
  });

  test('a machine counts as joined by INYON_HUB in its environment or its node env file, old name too', () => {
    const h = join(tmp, `joined${++n}`);
    expect(joinedHere(h, {})).toBe(false);
    expect(joinedHere(h, { INYON_HUB: 'https://hub.example' })).toBe(true);
    mkdirSync(join(h, '.tether-node'), { recursive: true });
    writeFileSync(join(h, '.tether-node/env'), 'TETHER_HUB=\n');
    expect(joinedHere(h, {})).toBe(false);
    writeFileSync(join(h, '.tether-node/env'), 'TETHER_HUB="http://100.64.0.1:4747"\n');
    expect(joinedHere(h, {})).toBe(true);
  });

  test('a hub is found by its settings, its node record or its service, as the inyon command finds it', () => {
    const h = join(tmp, `hubs${++n}`);
    expect(hubHere(h, {})).toBe(false);
    for (const f of ['.inyon/node/hub.json', '.config/systemd/user/inyon-hub.service', 'Library/LaunchAgents/tech.abhashchakraborty.inyon.hub.plist', 'data/hub.env']) {
      const one = join(h, f.replace(/\//g, '-'));
      mkdirSync(join(one, f, '..'), { recursive: true });
      writeFileSync(join(one, f), '');
      expect([f, hubHere(one, f === 'data/hub.env' ? { INYON_DATA_DIR: join(one, 'data') } : {})]).toEqual([f, true]);
    }
  });

  test('a folder listed under another spelling (a symlink on the way) is the same page', () => {
    const real = site('work/plan');
    symlinkSync(join(home, 'work'), join(home, 'w'));
    expect(pub([real, '--name', 'Plan']).status).toBe(0);
    const r = pub([join(home, 'w/plan'), '--name', 'Plan']);
    expect(r.status).toBe(0);
    expect(r.stdout).toContain('(already listed)');
    expect(list().pages.map((p) => [p.slug, p.path])).toEqual([['plan', real]]);
  });

  test('a broken list is never overwritten', () => {
    writeFileSync(join(home, '.inyon/pages.json'), '{"v":1,"pages":[');
    const r = pub([site('s')]);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("isn't valid JSON");
    expect(readFileSync(join(home, '.inyon/pages.json'), 'utf8')).toBe('{"v":1,"pages":[');
    writeFileSync(join(home, '.inyon/pages.json'), '{"v":2,"pages":[]}');
    expect(pub([site('s')]).status).toBe(1);
  });

  test("other writers' entries and fields are kept", () => {
    const other = { slug: 'ghostex', name: 'Inyon plan', path: '/somewhere/else', project: '/somewhere', by: 'cli', added: 1, extra: { kept: true } };
    writeFileSync(join(home, '.inyon/pages.json'), JSON.stringify({ v: 1, pages: [other], note: 'kept' }));
    expect(pub([site('ghostex')]).status).toBe(0);
    const d = list();
    expect(d.note).toBe('kept');
    expect(d.pages[0]).toEqual(other);
    expect(d.pages[1].slug).toBe('ghostex-2');
  });

  test('the agent is read from its environment, and --by wins', () => {
    expect(agentName({ CLAUDECODE: '1' })).toBe('claude');
    expect(agentName({ GEMINI_CLI: '1' })).toBe('gemini');
    expect(agentName({})).toBe('agent');
    expect(pub([site('a')], { CLAUDECODE: '1' }).status).toBe(0);
    expect(pub([site('b')], { CLAUDECODE: '1', GEMINI_CLI: '1' }).status).toBe(0);
    expect(pub([site('c'), '--by', 'opencode'], { CLAUDECODE: '1' }).status).toBe(0);
    expect(list().pages.map((p) => p.by)).toEqual(['claude', 'claude', 'opencode']);
  });

  test("the agent's conversation is kept, and the latest one to add the page wins", () => {
    expect(agentSession({ CLAUDE_CODE_SESSION_ID: 'f941de55-2d8f' })).toBe('f941de55-2d8f');
    expect(agentSession({ CODEX_THREAD_ID: '019a-thread' })).toBe('019a-thread');
    expect(agentSession({ CLAUDE_CODE_SESSION_ID: 'not one!' })).toBeUndefined();
    const dir = site('s');
    expect(pub([dir], { CLAUDECODE: '1', CLAUDE_CODE_SESSION_ID: 'chat-1' }).status).toBe(0);
    expect(list().pages[0].session).toBe('chat-1');
    expect(pub([dir]).status).toBe(0);
    expect(list().pages[0].session).toBe('chat-1');
    expect(pub([dir], { CODEX_THREAD_ID: 'chat-2' }).status).toBe(0);
    expect(list().pages[0].session).toBe('chat-2');
  });

  test('usage errors exit 2', () => {
    expect(pub([]).status).toBe(2);
    expect(pub([site('a'), '--name']).status).toBe(2);
    expect(pub([site('a'), 'extra']).status).toBe(2);
  });

  test('a stale lock is taken over; a fresh one is waited for', async () => {
    const lock = join(home, '.inyon/pages.json.lock');
    writeFileSync(lock, '');
    const old = (Date.now() - 20_000) / 1000;
    utimesSync(lock, old, old);
    expect(pub([site('a')]).status).toBe(0);
    expect(existsSync(lock)).toBe(false);

    writeFileSync(lock, '');
    const child = spawn('node', [join(scripts, 'publish.mjs'), site('b')], { env: env() });
    const done = new Promise((ok) => child.on('exit', ok));
    await Bun.sleep(400);
    expect(list().pages).toHaveLength(1);
    rmSync(lock);
    expect(await done).toBe(0);
    expect(list().pages).toHaveLength(2);

    writeFileSync(lock, '');
    expect(() => withLock(join(home, '.inyon/pages.json'), () => 1, { waitMs: 100 })).toThrow('held by another writer');
    rmSync(lock);
  });

  test('a lock its tag could not be written to (a full disk) is taken away again, not left for 10 seconds', () => {
    const file = join(home, '.inyon/pages.json');
    const full = () => { throw Object.assign(new Error('ENOSPC: no space left on device, write'), { code: 'ENOSPC' }); };
    let ran = false;
    expect(() => withLock(file, () => (ran = true), { waitMs: 100, write: full })).toThrow('ENOSPC');
    expect([ran, existsSync(`${file}.lock`)]).toEqual([false, false]);
    expect(withLock(file, () => 'in', { waitMs: 100 })).toBe('in');
  });

  test('a named pipe put in place of the lock is not waited on when it is released', () => {
    const file = join(home, '.inyon/pages.json');
    let writer;
    try {
      const at = Date.now();
      withLock(file, () => { writer = fifo(`${file}.lock`); });
      expect(Date.now() - at).toBeLessThan(1000);
      // it isn't this writer's lock, so it stays
      expect(statSync(`${file}.lock`).isFIFO()).toBe(true);
    } finally { writer?.kill(); }
  });

  test('the lock is removed only while it is still this writer\'s', () => {
    const file = join(home, '.inyon/pages.json');
    // another writer took it over (it thought this one had died) and holds it now
    withLock(file, () => writeFileSync(`${file}.lock`, '999 0123456789abcdef\n'));
    expect(readFileSync(`${file}.lock`, 'utf8')).toBe('999 0123456789abcdef\n');
  });

  test('a stale lock two writers judged leaves the fresh one made in between in its place', () => {
    const lock = join(home, '.inyon/pages.json.lock');
    writeFileSync(lock, '1 stale\n');
    const old = (Date.now() - 20_000) / 1000;
    utimesSync(lock, old, old);
    const judged = statSync(lock);
    rmSync(lock);
    writeFileSync(lock, '2 fresh\n');
    removeJudged(lock, judged);
    expect(readFileSync(lock, 'utf8')).toBe('2 fresh\n');
  });

  test.skipIf(process.getuid?.() === 0)('a stale lock that cannot be removed (a sandbox) is exit 4 at once, not a hang', () => {
    const lock = join(home, '.inyon/pages.json.lock');
    writeFileSync(lock, '1\n');
    const old = (Date.now() - 20_000) / 1000;
    utimesSync(lock, old, old);
    chmodSync(join(home, '.inyon'), 0o500);
    try {
      const r = spawnSync('node', [join(scripts, 'publish.mjs'), site('locked')], { env: env(), encoding: 'utf8', timeout: 10_000 });
      expect(r.status).toBe(4);
      expect(r.stderr).toContain(`node ${join(scripts, 'publish.mjs')} ${join(home, 'locked')}`);
    } finally { chmodSync(join(home, '.inyon'), 0o700); }
  });

  test('eight publishers at once all land, with unique slugs', async () => {
    const dirs = Array.from({ length: 8 }, (_, i) => site(`p${i}/plan`));
    const codes = await Promise.all(dirs.map((d) => new Promise((ok) => spawn('node', [join(scripts, 'publish.mjs'), d, '--name', 'Plan'], { env: env() }).on('exit', ok))));
    expect(codes).toEqual(Array(8).fill(0));
    const d = list();
    expect(d.pages.map((p) => p.path).sort()).toEqual([...dirs].sort());
    expect(new Set(d.pages.map((p) => p.slug)).size).toBe(8);
    expect(d.pages.every((p) => isSlug(p.slug) && /^plan(-\d)?$/.test(p.slug))).toBe(true);
    expect(existsSync(join(home, '.inyon/pages.json.lock'))).toBe(false);
  });
});

describe('new-site.mjs', () => {
  test('writes the marker and ships kit/icon.svg', () => {
    const dir = join(home, 'sites', 'grocer');
    const r = run('new-site.mjs', [dir, '--name', 'Grocer App']);
    expect(r.status).toBe(0);
    expect(marker(dir)).toMatchObject({ v: 1, slug: 'grocer-app', name: 'Grocer App', kind: 'blueprint' });
    expect(readFileSync(join(dir, 'kit/icon.svg'), 'utf8')).toStartWith('<svg');
    expect(readFileSync(join(dir, 'kit/data.js'), 'utf8')).toContain("name: 'Grocer App'");
    expect(r.stdout).toContain('inyon pages add');
    expect(r.stdout).toContain('publish.mjs');
    expect(pub([dir]).status).toBe(0);
    expect(list().pages[0]).toMatchObject({ slug: 'grocer-app', name: 'Grocer App' });
  });

  test('in a git repo it defaults to private/plans/<slug> and says when git would commit it', () => {
    const repo = join(home, 'repo');
    mkdirSync(repo);
    git(repo, 'init', '-q');
    const r = spawnSync('node', [join(scripts, 'new-site.mjs'), '--name', 'Big Plan', '--slug', 'big'], { cwd: repo, env: env(), encoding: 'utf8' });
    expect(r.status).toBe(0);
    expect(marker(join(repo, 'private/plans/big'))).toMatchObject({ slug: 'big', name: 'Big Plan' });
    expect(r.stdout).toContain('git would commit private/plans/big');

    writeFileSync(join(repo, '.git/info/exclude'), '/private/\n');
    const other = join(repo, 'docs/plan');
    const r2 = run('new-site.mjs', [other, '--name', 'Docs']);
    expect(r2.status).toBe(0);
    expect(r2.stdout).toContain(`Plans usually live in ${join(repo, 'private/plans/docs')}`);
    expect(r2.stdout).toContain('git would commit docs/plan');
    const r3 = run('new-site.mjs', [join(repo, 'private/plans/third'), '--name', 'Third']);
    expect(r3.stdout).not.toContain('git would commit');
    expect(r3.stdout).not.toContain('usually live');
  });

  test('in a linked worktree the plan goes to the main checkout, which outlives the worktree', () => {
    const repo = join(home, 'main');
    mkdirSync(repo);
    git(repo, 'init', '-q');
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'x');
    const wt = join(repo, '.claude/worktrees/feature');
    expect(git(repo, 'worktree', 'add', '-q', wt).status).toBe(0);
    const r = spawnSync('node', [join(scripts, 'new-site.mjs'), '--name', 'Feature plan'], { cwd: wt, env: env(), encoding: 'utf8' });
    expect(r.status).toBe(0);
    expect(marker(join(repo, 'private/plans/feature-plan'))).toMatchObject({ slug: 'feature-plan' });
    expect(existsSync(join(wt, 'private'))).toBe(false);
    // a folder named inside the worktree is used, with a warning
    const inside = join(wt, 'private/plans/here');
    const r2 = run('new-site.mjs', [inside, '--name', 'Here']);
    expect(r2.status).toBe(0);
    expect(r2.stdout).toContain(`is inside the git worktree ${wt}`);
    expect(r2.stdout).toContain(`Plans usually live in ${join(repo, 'private/plans/here')}`);
  });

  test('refuses a bad slug, a full folder, and no folder outside a repo', () => {
    expect(run('new-site.mjs', [join(home, 'x'), '--slug', 'No']).status).toBe(2);
    const dir = site('full');
    expect(run('new-site.mjs', [dir, '--name', 'x']).status).toBe(1);
    const r = spawnSync('node', [join(scripts, 'new-site.mjs'), '--name', 'X'], { cwd: home, env: env(), encoding: 'utf8' });
    expect(r.status).toBe(2);
  });

  test('--force keeps the first created time', () => {
    const dir = join(home, 's');
    run('new-site.mjs', [dir, '--name', 'S']);
    const first = marker(dir).created;
    expect(run('new-site.mjs', [dir, '--name', 'S', '--force']).status).toBe(0);
    expect(marker(dir).created).toBe(first);
  });
});

// Git before 2.31 prints --path-format=absolute back and exits 0; before 2.5, --git-common-dir too.
// fixtures/old-git has a git that does that, for when this runs inside the Inyon repo.
const oldGit = fileURLToPath(new URL('../../../fixtures/old-git/', import.meta.url));
describe('the project a folder belongs to', () => {
  function repoWithWorktree() {
    const repo = join(home, 'main');
    mkdirSync(join(repo, 'sub'), { recursive: true });
    git(repo, 'init', '-q');
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'x');
    const wt = join(repo, '.claude/worktrees/feature');
    expect(git(repo, 'worktree', 'add', '-q', wt).status).toBe(0);
    mkdirSync(join(wt, 'deep'));
    return { repo, wt };
  }

  test('with this git: the main checkout, from a linked worktree and from a subfolder', () => {
    const { repo, wt } = repoWithWorktree();
    expect(mainTree(repo)).toBe(repo);
    expect(mainTree(join(repo, 'sub'))).toBe(repo);
    expect(mainTree(wt)).toBe(repo);
    expect(mainTree(join(wt, 'deep'))).toBe(repo);
    expect(mainTree(home)).toBe(null);
    // a bare repo's worktree keeps its own top folder
    const bare = join(home, 'b.git');
    expect(spawnSync('git', ['clone', '-q', '--bare', repo, bare]).status).toBe(0);
    const bw = join(home, 'bw');
    expect(git(bare, 'worktree', 'add', '-q', bw).status).toBe(0);
    expect(mainTree(bw)).toBe(bw);
  });

  describe.skipIf(!existsSync(join(oldGit, 'git')))('with an old git that prints unknown options back', () => {
    test('git 2.30: the main checkout, never a folder named --path-format=absolute', () => {
      const { repo, wt } = repoWithWorktree();
      const g = join(oldGit, 'git');
      expect(mainTree(repo, g)).toBe(repo);
      expect(mainTree(join(wt, 'deep'), g)).toBe(repo);
      expect(mainTree(home, g)).toBe(null);
      const extra = { PATH: `${oldGit}:${process.env.PATH}` };
      const dir = site('main/.claude/worktrees/feature/private/plans/plan');
      expect(pub([dir], extra).status).toBe(0);
      expect(list().pages[0]).toMatchObject({ path: dir, project: repo });
      const r = spawnSync('node', [join(scripts, 'new-site.mjs'), '--name', 'Old git'], { cwd: wt, env: env(extra), encoding: 'utf8' });
      expect(r.status).toBe(0);
      expect(marker(join(repo, 'private/plans/old-git'))).toMatchObject({ slug: 'old-git' });
      expect(existsSync(join(wt, '--path-format=absolute'))).toBe(false);
      expect(r.stdout).not.toContain('--path-format');
    });

    test('git 2.4: no common dir, so the top folder', () => {
      const { repo, wt } = repoWithWorktree();
      const g = join(oldGit, 'git-2.4');
      expect(mainTree(join(repo, 'sub'), g)).toBe(repo);
      expect(mainTree(wt, g)).toBe(wt);
    });
  });
});

// The same rules as Inyon's hub, checked against its fixtures when this runs inside the Inyon repo.
const rulesFile = fileURLToPath(new URL('../../../fixtures/pages/rules.json', import.meta.url));
describe.skipIf(!existsSync(rulesFile))('rules match fixtures/pages/rules.json', () => {
  const rules = existsSync(rulesFile) ? JSON.parse(readFileSync(rulesFile, 'utf8')) : {};
  test('listMax', () => expect(LIST_MAX).toBe(rules.listMax));
  test('slugify', () => { for (const c of rules.slugify) expect([c.text, slugify(c.text)]).toEqual([c.text, c.slug]); });
  test('isSlug', () => { for (const c of rules.isSlug) expect([c.s, isSlug(c.s)]).toEqual([c.s, c.ok]); });
  test('uniqueSlug', () => { for (const c of rules.uniqueSlug) expect(uniqueSlug(c.base, (s) => c.taken.includes(s))).toBe(c.slug); });
  test('sitePathProblem', () => { for (const c of rules.sitePath) expect([c.path, sitePathProblem(c.path, c.home)]).toEqual([c.path, c.problem ?? null]); });
});
