# Contributing

Thanks for helping. A few notes before you open a pull request.

## How changes get in

- For anything bigger than a small fix, open an issue first, so we can agree on the shape before
  you spend time on it.
- Fork the repo, branch from `main` and open a pull request against it. One change per pull
  request is easiest to review.
- `main` only takes pull requests. Each one needs CI green and a review from the maintainer, and
  it's squash-merged, so the commits inside it can be messy.
- CI doesn't run on an outside contributor's pull request until the maintainer approves the run,
  and it never sees the repo's secrets.
- Security problems go through private reporting, not issues or pull requests. See
  [SECURITY.md](SECURITY.md).
- By sending a pull request you agree to license your change under the [MIT licence](LICENSE).
  Everyone here follows the [code of conduct](CODE_OF_CONDUCT.md).

## Setup

```sh
bun install
bun dev:hub      # the hub on :4777, a development build (cargo run -p inyon-cli -- engine hub)
bun dev:web      # Vite on :5173, proxies to the hub
```

Requires [Rust](https://rustup.rs), [Bun](https://bun.sh) 1.3 or later and tmux. The hub, the
`inyon` command and the phone core are Rust; Bun builds and tests the web app and is the package
manager too: there's no npm or pnpm step. Building the web app also builds the phone core as wasm, so it needs Rust with
the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version `crates/inyon-phone` pins:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version "$(sed -n 's/^wasm-bindgen = "=\(.*\)"/\1/p' crates/inyon-phone/Cargo.toml)"
```

## Checks

```sh
bun run typecheck
(cd apps/web && bun test)
(cd packages/protocol && bun test)
bun test ./installers ./scripts ./site   # get.sh and get.ps1 against a mock release, the release pipeline, the site
(cd apps/web && bun run build && bun run e2e)   # layout at phone widths, needs Playwright's Chromium
cargo test --workspace            # the inyon command and the Rust crates (or bun run cli:test)
cargo build -p inyon-cli && bun conformance/run.ts   # the hub against the recorded scenarios
bun site/build.ts --offline       # the website and docs; fails on a broken link
```

The screen parser (`crates/inyon-core/src/claude/screen.rs`) is tested against real captures from
the agents' terminal apps in `fixtures/screens`. If you change it, add a capture that shows the
case; `INYON_BLESS=1 cargo test -p inyon-core --test fixtures` writes what the parser makes of a
new one to `fixtures/expected`, to look over and commit with it.

The Inyon window on Windows (`apps/cli/src/window/`) places every control from the display scale
and the work area alone, in `layout.rs`, and `model.rs` works out what each place says.
`INYON_WINDOW_SVG=<dir> cargo test -p inyon-cli window::svg` draws every state as an SVG, so the
layout can be checked without Windows. On Windows, `INYON_WINDOW_DEMO=<file> inyon window link`
opens a separate window that reads lines such as `state=code dpi=144 theme=dark` from the file and
logs its size after each one to `window.log`, the only file it writes.

## Where things live

```
apps/cli           the inyon command (Rust): setup, status, pairing, services, updates, the Windows window
apps/web           React + Vite app, also served by the hub
installers/        get.sh and get.ps1, the one-line installers (the hub's install.sh and install.ps1
                   are in crates/inyon-hub/assets)
apps/mobile        Capacitor shell for Android (and an unshipped iOS project); scripts/icons.ts renders every icon
crates/            the Rust side: inyon-core (readers, machines), inyon-api (the routes and app
                   sockets, no server), inyon-hub (the hub), inyon-engine and inyon-link (the
                   phone link), inyon-backends, inyon-protocol, inyon-phone (the phone core as
                   wasm, for the Android app and the browser copy)
packages/protocol  types and the agent registry shared by hub and app
conformance/       recorded scenarios the hub must keep answering the same way
site/              the website and docs at inyon.tech
npm/               the npm launcher and per-platform packages
skills/            agent skills shipped with each release (Blueprint)
```

The phone core is Rust (`crates/inyon-phone`), built as wasm by `bun scripts/phone-core.ts` in
`apps/web`, which `bun run build` runs first: the core routes and sockets, SSH into servers through
the native plugin, the link to paired computers, and push through those computers. What the app
hands it and gets back is typed in `apps/web/src/lib/phone-types.ts`. `bun test crates/inyon-phone/js`
checks the module as the app calls it, and with `INYON_BIN` pointing at an `inyon` build it also
pairs with that build's `inyon link`. `VITE_INYON_PHONE_CORE=none` leaves it out, for the copy a
hub serves.

The pictures in the README are the real app fed a made-up set of machines: `bun run build` in
`apps/web`, then `bun scripts/readme/shots.ts` makes them again.

## Adding an agent

Most of what Inyon knows about an agent lives in one place: `packages/protocol/src/agents.ts`
(how to detect it, its colours, commands, models and config files). A chat view also needs a
reader that turns the agent's session files or API into chat items; see
`crates/inyon-core/src/agents/` for Codex, Gemini CLI, OpenCode, Copilot and Antigravity.

An agent without a reader starts as a terminal-first one. `terminalAgent()` in the same file fills in
the empty model, mode and command lists; you give its kind, name, vendor, install, sign-in and
version commands, auth files, process names and argv patterns, launch command and skin
(`update` and `latest` are optional). Add it to the `AgentKind` enum in
`crates/inyon-protocol/src/agents.rs`, run `bun packages/protocol/scripts/catalog.ts`, put its mark
in `apps/web/public/agents/<kind>.svg`, and add a `detect_agent` case from a real `ps` line to
`crates/inyon-protocol/tests/catalog.rs`. Apps before 0.23 crash on a kind they don't know, so the
hub shows them only the first six (`crates/inyon-protocol/src/compat.rs`); the app itself looks
kinds up with `agentDef()`, never `AGENTS[kind]`.

## Look and feel

The app is ink on warm silver, with a graphite dark mode and no accent colour. The colours live as
CSS variables in `apps/web/src/styles.css`; an agent session swaps them for that agent's skin from
`packages/protocol/src/agents.ts`. A few rules hold everywhere, and `design.test.ts` and
`skins.test.ts` check the ones code can show:

- No gradients and no blur. Depth comes from a hairline border and a faint top highlight.
- Nothing smaller than 12px. Mono is for data: paths, models, costs, times.
- Status chips stay neutral and the dot carries the state. Colour means something needs you
  (amber) or went wrong (red); green only marks added lines.
- Every agent skin keeps muted text, links and button labels at 4.5:1 or better.

Every icon, splash screen and favicon is drawn from `apps/web/public/icon.svg`. Change that file,
then run `bun --filter @inyon/mobile icons`.

## Style

Match the code around you. Keep comments for the why, not the what.
