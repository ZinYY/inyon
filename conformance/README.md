# Hub conformance

Golden transcripts of what the hub answers, and a harness that checks a hub against them. The Bun
hub wrote the first goldens; the hub in Rust (`crates/inyon-hub`) had to produce the same
transcripts, line for line, before it replaced it, and the Bun hub was removed in 0.23. Now the
goldens hold the Rust hub to what it answers.

```sh
bun run --cwd apps/web build && cargo build -p inyon-cli   # the hub serves the app built here
bun conformance/run.ts                          # the hub cargo built in this checkout
bun conformance/run.ts --hub '<command>'        # any other hub
bun conformance/run.ts --update                 # rewrite the goldens from this run
bun conformance/run.ts --only phones,push       # some scenarios
bun conformance/run.ts --keep --verbose         # keep each temp folder, print the hub's log
bun test conformance/test                       # the harness's own tests
```

A full run takes about 2.5 minutes on two ARM cores and needs no network, tmux or agents.

## Why it lives at the repo root

It tests a contract between programs, not one package. It starts the hub as a separate process by
its command line, so any hub can be checked against the same goldens (it held the Bun hub and the
Rust one to the same answers while both existed). It is a Bun workspace (`@inyon/conformance`) so
`bun --filter '*' typecheck` covers it.

## How a run works

Each scenario gets a fresh temp folder, a fresh hub and, when it needs one, a fake machine:

- **The hub under test** runs with `INYON_DATA_DIR`, `HOME` and `PATH` inside the temp folder, on a
  free port on 127.0.0.1, with `NODE_ENV=test`. Its `PATH` holds stubs: no tmux server, no agents,
  `git` that says "not a git repository", `curl` that can't connect. `hosts.json` is seeded with
  one paired machine, `box`, whose token the harness knows.
- **The fake machine** (`lib/node.ts`, `lib/machine.ts`) connects to `/node` like `inyon` does and
  answers `exec`, `stream` and `pty`. Shell snippets run with `/bin/sh` under the temp `HOME`, with
  stub `tmux`, `ps`, `pgrep` and `sqlite3` that ask the harness. The harness keeps the panes in
  memory: their screens come from `fixtures/screens/`, transcripts are written as the agents would
  write them, and every key the hub sends is recorded. Control mode (`tmux -C`) gets a `%output`
  line whenever a scenario redraws a pane.
- **The driver** (`lib/context.ts`) signs in as the web app does (setup code, then the cookie),
  calls REST routes and opens `/ws`. Everything it sends and gets back goes into the transcript.

A scenario is a list of steps. A step ends when nothing has arrived for 900 ms (at most 12 s); then
the messages pushed during it are written down, folded:

- The overview, `chat.info`, `chat.screen`, `chat.live`, `chat.draft` and `chat.progress` are
  snapshots: only the last one in a step counts. One equal (after normalization) to the snapshot
  written before it is left out: the hub sends a new overview when a value changes that the
  normalizer hides (a clock reading, say), and whether a step catches one is timing.
- `chat.append` messages in a row are one; `term.data` chunks of one terminal are one string.
- Notices, `chat.reset`, `term.exit` and anything unknown are kept in order.
- What the fake machine was told to do is written per pane, in order.

Waits are explicit (`until` an overview shows the pane working, say). A wait that runs out is
written as `{"timeout": …}` instead of failing the run, so a hub that never sends something shows
up as a diff. Nothing is retried.

## The normalizer

`lib/normalize.ts` holds the only differences two runs may have. A value no rule matches is kept
exactly. Each rule says why it exists:

| Rule | Replaces |
|---|---|
| learned | values made up for this run that the harness was handed: tokens, codes, cookies, ids, the temp folder, the port, the checkout's path. The computer's name only where it is the whole string |
| uuid | random UUIDs, numbered by first appearance so identity is kept |
| sha256 | checksums of build outputs and of the install script inside pairing commands (not the harness's own `contentSha256` of a long body) |
| view-name | the random name of a terminal's view session |
| iso-time | ISO timestamps from this run (fixtures are dated January 2026 and stay) |
| epoch | clock readings in ms or s from two days before the run to 40 days after |
| key-order | the order of an object's fields (sorted by name); arrays keep theirs. The Bun hub wrote fields in the order its code added them, a Rust struct in the order it declares them, and every client reads them with `JSON.parse`. Keys that are ids not seen before (a map keyed by session ids) are numbered in the order of what they hold, so two such maps alike but for order still match |
| latency | `latencyMs` |

`test/normalize.test.ts` has a test for each rule and fails when a rule has none.
`test/diff.test.ts` runs the push scenario against the real hub behind a proxy that drops one
question mark, and checks the diff points at it.

## Goldens

`golden/<scenario>.jsonl`, one JSON value per line: `{"step"}`, `{"rest", "status", "headers",
"body"}` (only the headers in `HEADERS` in `lib/context.ts`, a `Set-Cookie` split into its fields
with its signature checked), `{"raw", …}` the same for a request written byte for byte on a socket
(a target with `..` or `%2e%2e` that `fetch` would rewrite, or another Host), `{"send"}`,
`{"ws", "got"}`, `{"ws", "upgrade", "status", "body"}` for a refused WebSocket, `{"machine"}`,
`{"node"}`, `{"file"}` for a file the hub wrote, and `{"timeout"}`. Long text bodies are written as
their size and checksum, downloads too.

When a change to the hub is meant to change an answer, run with `--update` and commit the goldens
with it. The diff of the golden file is the review.

## Testing another hub

`--hub '<command>'` runs the command with `/bin/sh -c` in the temp folder, with this environment:
`PORT`, `BIND=127.0.0.1`, `INYON_DATA_DIR`, `HOME`, `PATH`, `NODE_ENV=test`,
`INYON_HUB_NAME="Conformance hub"`, `INYON_CLI_DIST`, `TMUX_TMPDIR`, `LANG=C.UTF-8` and `TZ=UTC`
(and `CARGO_TARGET_DIR` when it's set). The hub must read the same data files (`hosts.json`,
`hub.json`, `setup-code`, `local-token`) and answer on `/api/session` within 30 s.
`INYON_CLI_DIST` holds a small placeholder `inyon` for each system (the Windows ones with just
enough of a PE header to be made windowless), standing in for what cargo builds into
`apps/cli/dist`, so the `node-dist` scenario builds and fetches nothing.

The default hub is `$CARGO_TARGET_DIR/debug/inyon engine hub` (or `target/debug` without it).

## What is covered

| Scenario | What it checks |
|---|---|
| setup-signin | first run: setup code and password, the cookie's fields, the WebSocket with and without it, sign-out, sign-in, a password change |
| setup-limits | wrong setup codes, slowed and capped at eight a minute |
| login-limits | wrong passwords, capped per client (and per `X-Forwarded-For` client behind a proxy), an overlong password |
| overview | a machine connecting, its sessions, a Claude pane and a shell, refresh, settings, ping |
| claude-chat | a Claude Code chat: screen, transcript, older items, commands, models, past conversations, subagents, closing it |
| claude-send | the draft mirror, a closed-loop send with its delivery receipt, retries, a paste of several lines, bad sends |
| claude-approval | a permission and a question card answered from the app, a stale answer, a send while it waits |
| claude-model | model and effort changes from the app's controls, with Claude's confirmation |
| codex | a Codex session: overview, chat from the rollout, a send and its reply |
| opencode | an OpenCode session read from its database: overview, chat, models, commands |
| terminal | attach, input, resize, close, a missing pane, the machine ending it |
| notifications | waiting and done notices to every open app |
| push | Web Push registration routes and a test push that can't be delivered |
| phones | phone pairing: code, wrong digits, claim, bearer token, tickets, CORS, owner-only routes, removal |
| machine-pairing | pairing commands, a new machine connecting, pairing again, removal and the 410 after it |
| hub-update | the update check and a refused start |
| node-dist | install scripts, `SHA256SUMS`, programs and checksums, bad systems |
| local-cli | `/api/local/nodes` with and without the local token |
| errors | unknown machine and pane, bad JSON and input, settings ranges, sign-in, origin and Host checks, 413, an offline machine |
| pages | adding a site (refused folders, its marker's slug), the list with progress, a token link and every kind of `/v/` answer (redirects, ETags, dot files, symlinks and dot segments out of the site, bad tokens, a wrong Host), framed and cross-site API calls refused, revoking, a page removed and added again by another program, removing |

## What is left out

- The web app's static files, and the page a browser gets for a client route.
- Offline and back-online notices: they come after a machine has been gone for 60 s.
- Delivering a Web Push message (the push service needs TLS); only the routes are covered.
- SSH machines: adding one starts a real `ssh`.
- Downloads that come from GitHub: busybox, release checksums.
- Uploads, git routes, file editing, rewind, fork, launch and restart, provider sign-in, the
  integration hooks, Antigravity, Gemini and Copilot sessions. They need more of a machine than
  the fake one models, and are next.
- The exact shell text of `exec` and `stream` requests: the machine runs them, and only what it was
  asked to do to its panes is written down.
- Sending to OpenCode: the fake TUI doesn't write sent messages to its database yet.
