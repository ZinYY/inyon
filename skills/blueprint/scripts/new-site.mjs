// node new-site.mjs [dir] --name "Product" [--slug product] [--no-mockups] [--force]
// Copies the Blueprint site template into <dir>, sets the product name and writes .inyon-page.json, the
// marker Inyon reads. Then replace the sample "Pantry" content: kit/data.js, mockups/screens.js, the
// screens, matrix-data.js, progress.json, index.html.
// In a git repo, <dir> defaults to <repo>/private/plans/<slug>, the usual home for a plan. In a linked
// worktree that is the repo's main checkout, so the plan isn't deleted with the worktree.
// --no-mockups leaves out the screens (mockups/, kit/shell.js, kit/menus.js) and every link to them,
// for a plan with no UI to show, such as a parts list or a migration.
import { appendFileSync, cpSync, existsSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BAD_SLUG, GIT_ENV, GIT_TIMEOUT_MS, gitTop, isSlug, mainTree, revParse, slugify, writeMarker } from './pages.mjs';

const a = process.argv.slice(2);
const opt = (k) => { const i = a.indexOf(k); return i < 0 ? null : a.splice(i, 2)[1]; };
const name = opt('--name'), slugOpt = opt('--slug');
const force = a.includes('--force'), mockups = !a.includes('--no-mockups');
const dirArg = a.find((x) => !x.startsWith('--'));
const usage = 'usage: node new-site.mjs [dir] --name "Product" [--slug product] [--no-mockups] [--force]';
if (slugOpt !== null && !isSlug(slugOpt)) { console.error(BAD_SLUG); process.exit(2); }

const git = (...args) => { try { return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: GIT_TIMEOUT_MS, killSignal: 'SIGKILL', env: { ...process.env, ...GIT_ENV } }).trim(); } catch { return null; } };
// the repo the new folder will be in: ask git from its nearest folder that exists already
let probe = dirArg ? resolve(dirArg) : process.cwd();
while (!existsSync(probe) && probe !== dirname(probe)) probe = dirname(probe);
const top = gitTop(probe);
// a linked worktree's plans go to the main checkout
const repo = top ? (mainTree(probe) ?? top) : null;
const slug = slugOpt || slugify(name || (dirArg ? basename(resolve(dirArg)) : ''));
if (!dirArg && (!repo || !name)) { console.error(`${usage}\nOutside a git repo, name the folder; inside one, --name is enough.`); process.exit(2); }
const out = dirArg ? resolve(dirArg) : join(repo, 'private', 'plans', slug);

if (existsSync(out) && readdirSync(out).length && !force) { console.error(`${out} is not empty; pass --force to copy over it`); process.exit(1); }
const here = (f) => fileURLToPath(new URL(f, import.meta.url));
cpSync(here('../assets/site'), out, { recursive: true });
const edit = (f, fn) => { const p = join(out, f); writeFileSync(p, fn(readFileSync(p, 'utf8'))); };
if (name) edit('kit/data.js', (s) => s.replace("name: 'Pantry'", `name: ${JSON.stringify(name).replace(/^"|"$/g, "'")}`));
if (!mockups) {
  for (const f of ['mockups', 'kit/shell.js', 'kit/menus.js']) rmSync(join(out, f), { recursive: true, force: true });
  edit('index.html', (s) => {
    // Drop everything between the mockups markers, make the matrix the main button, renumber the sections.
    s = s.replace(/^[ \t]*<!-- mockups -->[\s\S]*?<!-- \/mockups -->[ \t]*\n/gm, '')
      .replace(/<!-- mockups -->[\s\S]*?<!-- \/mockups -->/g, '')
      .replace(/^[ \t]*\/\* mockups \*\/[\s\S]*?\/\* \/mockups \*\/[ \t]*\n/gm, '')
      .replace(/\n[ \t]*The mockups markers wrap[^\n]*(?= -->)/, '')
      .replace('<a class="btn" href="matrix.html">', '<a class="btn primary" href="matrix.html">');
    let t = 0, h = 0;
    s = s.replace(/(<nav class="toc"[\s\S]*?<\/nav>)/, (nav) => nav.replace(/<span>\d+<\/span>/g, () => `<span>${String(++t).padStart(2, '0')}</span>`));
    return s.replace(/<span class="n">\d+<\/span>/g, () => `<span class="n">${String(++h).padStart(2, '0')}</span>`);
  });
  appendFileSync(join(out, 'matrix-data.js'), '// Made with --no-mockups: the site has no mockups folder, so rows link to no screens.\nwindow.MATRIX_MOCKUPS = false;\n');
}
writeMarker(out, { slug, name: name || basename(out) });

const notes = [];
if (repo) {
  const plans = join(repo, 'private', 'plans');
  if (relative(plans, out).startsWith('..')) notes.push(`Plans usually live in ${join(plans, slug)}, next to the code they describe.`);
  if (repo !== top && !relative(top, out).startsWith('..')) notes.push(`${out} is inside the git worktree ${top}: removing the worktree deletes the plan with it.`);
  if (git('-C', repo, 'check-ignore', '-q', out) === null) notes.push(`git would commit ${relative(repo, out)}. Plans often describe unreleased work: add ${relative(repo, out).split('/')[0]}/ to ${resolve(repo, revParse(repo, ['--git-path', 'info/exclude']) || '.git/info/exclude')} unless the repo should have it.`);
}
console.log(`Blueprint site in ${out}${name ? ` for ${name}` : ''} (slug ${slug})${mockups ? '' : ', without mockups'}.`);
for (const n of notes) console.log(n);
console.log(`When it's ready, publish it: inyon pages add ${out}${name ? ` --name ${JSON.stringify(name)}` : ''}\n  (without the inyon command: node ${here('publish.mjs')} ${out}; without Inyon: node ${here('serve.mjs')} ${out} --host <private-ip>)`);
