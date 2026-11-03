# Test fixtures

Captured agent screens and records that the parser tests read. Until 0.23 the Bun hub's tests
and the Rust tests read the same files, so each parser ported to Rust was checked against exactly
what the Bun one made of them. The Bun hub is gone now, and these files hold the Rust side to
those answers.

## Layout

```
fixtures/
  screens/<agent>/<name>.txt         one terminal capture each
  transcripts/<agent>/<name>.jsonl   an agent's own records, one JSON value per line
  protocol/<Type>/<name>.json        messages the hub and the app send each other
  link/<part>.json                   the phone link's bytes, as the TypeScript link wrote them
  pages/                             a small site and what a hub answers for it under /v/
  expected/                          what the readers make of the captures
  old-git/                           a git that prints rev-parse options it doesn't know back
  upgrade/export-v1.json             what the Capacitor app seals for the app that installs over it
```

`<agent>` is the agent's id from `packages/protocol/src/agents.ts`: `claude`, `codex`, `opencode`,
`copilot`, `gemini` or `antigravity`. The screen and transcript parsers are tested from here.

`protocol/` holds wire samples, made with the Bun hub's parsers and readers. `<Type>` is the name
of a type in `packages/protocol` (`ServerMsg`, `ClientMsg`, `AgentCatalog`…), and a file holds one
message or an array of them. Each must read into the Rust type and write back as the same JSON.

`link/` holds the phone link's test vectors (identity, Noise, channel, codes, pairing links, pkarr,
relay), written by the TypeScript link (`packages/link`, removed in 0.23). `crates/inyon-link`
checks its output against them. They don't change: phones and computers on older versions still
send those bytes.

`expected/` holds what the readers make of the shared captures:
`expected/screens/<agent>/<name>.json` for each screen (read at its own width, at 80 and at 120
columns), `expected/chat-screen.json`, which ties each `chat.screen` sample in `protocol/` to the
capture it came from, and `expected/scripts.json`, the shell the hub sends to machines. The Bun
hub's readers wrote them; `crates/inyon-core` checks the Rust readers against them.
`INYON_BLESS=1 cargo test -p inyon-core --test fixtures` writes the file for a capture added since.
When a reader changes on purpose, change its file in the same commit.

`pages/` is the contract for Pages, the agent-made sites the hub serves under `/v/<token>/`:

- `site/` is a small site: an index, a sub folder, an image, a stylesheet, notes in Markdown, a
  `progress.json`, the `.inyon-page.json` marker, and a dot file and dot folder that are never
  served. Its bytes matter (ETags count them), so it is checked in as binary like everything here.
- `tokens.json` holds the token key made from a fixed session secret, tokens minted at a fixed
  time, and tokens to verify with the verdict each must get: valid, expired, revoked, a phone that
  was unpaired, a changed payload or mac, and a list of malformed ones.
- `rules.json` holds the rules the hub and the `inyon` command share: slugs made from names, which
  folders may be pages, and how a `progress.json` is counted for the list.
- `responses.json` lists requests against `site/` (traversal, encoded dot segments, dot files, a
  folder without its slash, symlinks out of the site, HEAD, If-None-Match, bad tokens, other
  methods, a wrong Host) and the exact status, headers and body each one gets. Its `setup` says
  how to lay the site out first: copied to a temp folder, with symlinks added and every mtime set.

The Bun hub wrote the three JSON files from its pages rules. The Rust hub sends each request in
`responses.json` to a real server and compares, and `crates/inyon-core` checks the tokens and
rules, so a change to the rules means changing the files and the code in the same commit.

`old-git/git` stands in for git before 2.31, which prints an option it doesn't know
(`--path-format=absolute`) back on stdout and still exits 0; `old-git/git-2.4` doesn't know
`--git-common-dir` either. Everything else goes to the real git. The tests for the project a page
belongs to (the hub, the `inyon` command and the Blueprint skill) run against both and the real one.

There are no transcript captures yet. The reader tests build their records in code, one at a time,
next to what each one should produce.

## Screens

A screen file holds the rows exactly as the test feeds them to the parser, joined with `\n`. The
tests read it with `.split('\n')`, so:

- Keep the bytes as captured: colour codes, trailing spaces and wide characters stay.
- Don't add a final newline the capture didn't have (it would become an extra empty row). Files
  saved straight from tmux end with one, and the tests expect that row.
- The pane width and the agent aren't stored in the file. Each test passes them, so name a capture
  after its width when that matters (`permission-wrapped` is the 80-column one).

To add one, capture the pane and save it under the agent's folder:

```sh
tmux capture-pane -p -t %12 > fixtures/screens/codex/new-menu.txt      # text only
tmux capture-pane -p -e -t %12 > fixtures/screens/opencode/new-tab.txt # with colours, when the parser needs them
```

Shorten home paths and anything private before you commit it.

## Reading them

- Rust tests: `screen_fixture("codex", "queued")` from `crates/inyon-core/src/test_support.rs`.
- The layout: `cargo test -p inyon-protocol --test fixtures` (`crates/inyon-protocol/tests/fixtures.rs`)
  loads every file and fails on one that is out of place or doesn't parse.

`upgrade/export-v1.json` is the Android upgrade file's contents (from 1.0): what
`apps/web/src/lib/upgradeExport.ts` collects, plus the `secrets` the Kotlin side adds before it
seals the file. The web app's test checks it writes this, and `apps/app/src/upgrade.rs` checks it
reads it back, so the two ends of the upgrade can't drift apart. The secrets are placeholders.
