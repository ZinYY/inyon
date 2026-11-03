---
name: blueprint
description: Build a plan site for a product or feature, a static HTML page you open on your phone or laptop instead of a chat artifact. It holds the written plan, clickable phone and desktop mockups in a viewer, a feature matrix, figures and a live progress board that updates while the work runs. Use it whenever someone asks for a plan page, an HTML plan, a roadmap site, mockups of a redesign, "show me what it will look like", or a page to follow progress on, even if they don't name this skill, and when a plan is too big to read in a terminal.
---

# Blueprint

A blueprint is a folder of plain HTML, CSS and JS with no build step. Publish it to Inyon, or serve it on
a private address, and open it on a phone. It has four parts that link to each other:

| Page | What it is |
|---|---|
| `index.html` | The plan: a short lede, the live progress board, a contents box, numbered sections, a screen gallery, releases you can open, risks and decisions |
| `mockups/index.html` | The viewer: every screen framed at 390×844 or 1440×900, light or dark, with design notes |
| `mockups/screens/*.html` | One clickable screen per page, built on the kit (menus, right-click and long-press, sheets, toasts, tooltips) |
| `matrix.html` | One row per item (a feature, a part, a task): what's wanted, what exists today, its status and its release, with search and filters. Statuses, labels and columns come from `matrix-data.js`, so it also fits a parts list or a task list |

`progress.json` drives the board on the plan page. The page polls it every 20 seconds, so updating it with
`scripts/progress.py` while you work is how the reader follows along. `.inyon-page.json` in the site's
root names the site for Inyon. It's a dot file, so no server shows it.

## Workflow

1. **Research first.** Read the code, the product and anything it's compared with. Write notes into
   `research/` inside the site (features, how things work today, open bugs). The matrix and the screens
   are built from these notes, so every claim needs a source.
2. **Start the site.** Run `node <skill>/scripts/new-site.mjs <dir> --name "Product"`. Inside a git repo
   you can leave out `<dir>`: the site goes to `<repo>/private/plans/<slug>`, which is where plans belong.
   In a linked worktree (`.claude/worktrees/<name>`, say) that's the main checkout's folder, so the plan
   isn't deleted with the worktree.
   Keep `private/` out of git (the script tells you when git would commit the folder). It copies
   `assets/site/`, a working sample for a made-up grocery app called Pantry, so you can see each piece in
   action, and writes `.inyon-page.json`. Add `--no-mockups` when there's no UI to show (a parts list, a
   migration): it leaves out `mockups/`, `kit/shell.js`, `kit/menus.js` and every link to them. Such a
   site has no screens, so skip what's marked *(screens only)* below. Make the site yours:
   - In `kit/data.js`, set `BRAND` (name, optional SVG logo) and replace the sample nav, groups and people.
   - In `kit/blueprint.css`, swap the `:root` and dark tokens for the product's palette and fonts, if it
     has its own. Components read only tokens.
   - *(screens only)* Rebuild `kit/shell.js` to match the product's real layout. Screens only call
     `K.shell.mount`.
   - *(screens only)* Put shared menus in `kit/menus.js`. Each object gets one menu, used by right-click,
     long-press and its ⋯ button.
3. **Write the plan** in `index.html`. Read `references/plan.md` for the section order, the release format
   and the writing rules. Number the h2s to match the contents box.
4. **Build the screens** *(screens only)*. List them in `mockups/screens.js`, copy `mockups/screens/_template.html` for
   each, and follow `references/kit.md`. Every visible control has to do something. For more than about
   six screens, split them across parallel workers using `references/builders.md`.
5. **Fill the matrix** in `matrix-data.js` from the research notes, one row per item, then set
   `MATRIX_STATUSES`, `MATRIX_RELEASES`, `MATRIX_AREAS` and `MATRIX_LABELS`. A row can carry a `url`
   (a store page, an issue). For a plan that isn't software, rename things in `MATRIX_LABELS` (a release
   can be a store or a phase) and set a label to `''` to drop its column, such as `today`.
   `references/plan.md` lists the keys.
6. **Figures.** Use a still WebP, or an animated SVG with the still inside it, in `figures/`. Take labels
   from the real system. Use Mermaid only if nothing better is available.
7. **Check it.** *(screens only)* `node <skill>/scripts/stills.mjs <dir> --audit` writes gallery stills to
   `shots/` and flags console errors and dead controls on every screen at both sizes. Open the plan, the
   viewer *(screens only)* and the matrix with `scripts/shoot.mjs` at 390×844 and 1440×900 in light and dark, and look at every picture
   (`FULL=1` takes the whole page; lazy images load first). Fix what you see, then check once more.
8. **Publish it** where the reader can open it. Take the first of these that works:
   - **Inyon's command.** If `command -v inyon` finds it, run `inyon pages add <dir> --name "Product plan"`.
     It prints where the page sits in the app (`Pages > <machine> > <project>`; from Inyon 0.22.5 the app
     files it under your session's name instead) and its app link
     (`/pages/<host>/<slug>`). Hand over both.
   - **The script.** Without the command, run `node <skill>/scripts/publish.mjs <dir> --name "Product plan"
     --by <claude|codex|gemini|opencode>`. It adds the site to `~/.inyon/pages.json` itself, and the hub
     picks it up with no restart. It prints the same two lines.
   - **A server.** When publish.mjs says Inyon isn't set up here (exit 3), serve the folder:
     `node <skill>/scripts/serve.mjs <dir> --host <private-ip> --port 8790`. Prefer a VPN or LAN address
     over a public one, since plans are usually private. Hand over the URL.

   **No hub on this machine.** When either command says "This machine doesn't run a hub":
   - *"The hub it's joined to shows its pages"* (publish.mjs exits 5): a hub on 0.22 or later shows the
     page under this machine's name. Hand over the `Pages > <machine> > <project>` line; there's no app
     link. With an older hub the app can't open it, so also serve the folder with serve.mjs as above (the
     script prints that line) and hand over the URL.
   - *"...and isn't joined to one"* (publish.mjs exits 6): nothing can show the page in the app. Serve the
     folder with serve.mjs and hand over the URL.

   Publishing a folder again keeps its slug, so do it whenever you like. Never set up a systemd unit,
   launchd job or any other service to keep a site up: Inyon serves pages for as long as its hub runs.
   Never hand over a `/v/...` link either, since those expire after 12 hours.

   **In a sandbox.** If `~/.inyon` can't be written from where you run (publish.mjs exits 4), don't look
   for a way around it. Show the reader the line the script prints and ask them to run it once from
   their own shell. It's `inyon pages add ...` with the installed program's full path, so it works where
   `inyon` isn't on the PATH, or `node .../publish.mjs ...` when the command isn't installed. Everything
   else works as before.
9. **Keep the board live** while you carry out the plan:
   `python3 <skill>/scripts/progress.py --file <dir>/progress.json task <id> doing "note"`, then `done`,
   plus `now "<what you're on>"`, `log "<line>"` and `link <id> <PR url>`. Parallel workers can all call it:
   it takes a lock and swaps the file in whole, so the page never reads half of it.

## Rules that make it good

- **True or nothing.** Numbers, names and statuses come from the code or the research notes. Mark
  estimates as estimates and say how they'll be measured.
- **Phone first.** Every page works at 390 px with no sideways scroll. Menus turn into bottom sheets,
  long-press does what right-click does, and touch targets are at least 44 px.
- **Nothing dead.** A control that can't really work in a mockup shows a toast saying what it would do.
  Destructive actions get an Undo toast.
- **Quiet design.** Hairlines, tokens, no gradients and no decorative colour. Status colours are only for
  states that need attention. Use Lucide icons, never sparkles. Nothing smaller than 12 px.
- **Plain copy.** Short sentences, sentence case, real labels, no filler words. Show the trade-off.
- **Everything local, every URL relative.** Pages open inside Inyon in a sandbox that blocks other
  sites, so fonts, icons, scripts and pictures live in the folder and links say `kit/kit.js`, never
  `/kit/kit.js` or `https://...`. Task links on the board may point anywhere over http(s).
- **It's a page, not a chat artifact.** Ship it as files the reader opens in Inyon or on a server they
  control.

## Files in this skill

- `assets/site/`: the template site (kit, viewer, sample screens, matrix, progress board)
- `scripts/new-site.mjs`, `publish.mjs`, `serve.mjs`, `shoot.mjs`, `stills.mjs`, `progress.py`, plus
  `lib.mjs` and `pages.mjs`, which they share
- `tests/`: checks for the scripts. From this folder, run `bun test ./tests` and
  `python3 tests/test_progress.py`
- `references/kit.md`: the `window.K` API, CSS classes and data shapes
- `references/plan.md`: plan page structure, releases, matrix rows, progress.json
- `references/builders.md`: a brief to hand parallel screen builders

The scripts need Node 18+ (or Bun) and Playwright with Chromium: `bun add -d playwright` (or npm) in the
scripts folder or the project, then `bunx playwright install chromium`. Without that download they use
Edge or Chrome where it usually lives on Windows, macOS and Linux. Set `CHROME_PATH` to use another
browser, or `BLUEPRINT_PLAYWRIGHT` to the folder whose `node_modules` holds Playwright. If sharp
is installed, stills are WebP; without it they're PNG.
