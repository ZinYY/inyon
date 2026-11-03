// node publish.mjs <site-dir> [--name "Plan name"] [--slug plan-name] [--by claude|codex|gemini|opencode]
// Adds a Blueprint site to Inyon's list of pages on this machine (~/.inyon/pages.json), so it opens in
// the Inyon app under Pages. Use it when the `inyon` command isn't installed; `inyon pages add` does the
// same. Publishing a folder again updates its entry and keeps its slug. Works with no hub running: the
// hub reads the file again when it changes.
//
// Exit codes: 0 listed, 1 refused (the message says why), 2 usage, 3 Inyon isn't set up here (serve
// the site with serve.mjs instead), 4 ~/.inyon can't be written from here (run the printed command),
// 5 listed on a machine joined to a hub: that hub shows it from 0.22 (with an older hub, serve it with
// serve.mjs too), 6 listed, but this machine has no hub and isn't joined to one (serve it with serve.mjs).
import { existsSync } from 'node:fs';
import { hostname } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LOCAL_HOST, agentName, hubHere, inyonDir, joinedHere, publish } from './pages.mjs';

const a = process.argv.slice(2);
const opt = (k) => { const i = a.indexOf(k); if (i < 0) return undefined; const v = a[i + 1]; a.splice(i, 2); return v === undefined || v.startsWith('--') ? null : v; };
const name = opt('--name'), slug = opt('--slug'), by = opt('--by');
if (a.includes('--help') || a.includes('-h') || a.length !== 1 || [name, slug, by].includes(null)) {
  console.error('usage: node publish.mjs <site-dir> [--name "Plan name"] [--slug plan-name] [--by claude|codex|gemini|opencode]');
  process.exit(2);
}
const dir = resolve(a[0]);
const win = process.platform === 'win32';
// quoted for the user's own shell: PowerShell on Windows, a POSIX shell elsewhere
const q = (s) => (win
  ? (/^[\w+=:./\\-]+$/.test(s) ? s : `'${s.replace(/'/g, "''")}'`)
  : (/^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`));
// What to run later from the user's own shell, outside the sandbox: the installed program by its full
// path (so it works where inyon isn't on the PATH), or else this script with node.
const bin = join(inyonDir(), 'bin', win ? 'inyon.exe' : 'inyon');
const here = (f) => q(fileURLToPath(new URL(f, import.meta.url)));
const flags = [...(name ? ['--name', q(name)] : []), ...(slug ? ['--slug', q(slug)] : [])];
const manual = () => (existsSync(bin)
  ? [win ? `& ${q(bin)}` : q(bin), 'pages', 'add', q(dir), ...flags]
  : ['node', here('publish.mjs'), q(dir), ...flags, '--by', q(by || agentName())]).join(' ');
const serve = () => `node ${here('serve.mjs')} ${q(dir)} --host <private-ip>`;

try {
  const { page, changed } = publish(dir, { name, slug, by });
  const again = changed ? '' : ' (already listed)';
  // the same lines `inyon pages add` prints; without a hub here the page opens through the hub this
  // machine is joined to, so there's no app path to hand over
  if (!hubHere() && joinedHere()) {
    console.log(`"${page.name}" is in Inyon: Pages > ${hostname()} > ${basename(page.project)}${again}`);
    console.log("This machine doesn't run a hub. The hub it's joined to shows its pages (hub 0.22 or later).");
    console.log(`With an older hub, serve the site to open it: ${serve()}`);
    process.exit(5);
  }
  if (!hubHere()) {
    console.log(`"${page.name}" is listed on ${hostname()}, under ${basename(page.project)}${again}.`);
    console.log("This machine doesn't run a hub and isn't joined to one, so the app can't open this page. Add it to a hub (Machines > Add a machine) to show its pages there.");
    console.log(`Serve the site to open it now: ${serve()}`);
    process.exit(6);
  }
  console.log(`"${page.name}" is in Inyon: Pages > ${hostname()} > ${basename(page.project)}${again}`);
  console.log(`Open it in the app at /pages/${LOCAL_HOST}/${page.slug}`);
} catch (e) {
  if (e.code === 'ENOINYON') {
    console.error(`${e.message}. Serve the site instead: ${serve()}`);
    process.exit(3);
  }
  if (['EACCES', 'EPERM', 'EROFS'].includes(e.code)) {
    console.error(`Can't write to ${inyonDir()} from here (${e.code}). Ask the user to run this once:\n  ${manual()}`);
    process.exit(4);
  }
  console.error(e.message);
  process.exit(1);
}
