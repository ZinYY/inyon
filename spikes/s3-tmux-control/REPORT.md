# S3: tmux control mode in Rust

**Verdict: go.** One control-mode connection per session can carry everything the hub does with
tmux today, on tmux 3.0a through 3.6a, and runs each command 20 to 200 times faster than starting a
tmux process for it. psmux works for listing, typing, output and capture, but not for pastes, so
Windows keeps a non-control path for those.

## What was built

`spikes/s3-tmux-control`, its own Cargo workspace, std only (about 1,050 lines of library):

- `proto.rs`, `escape.rs`: a sans-io parser. Lines in, events out: `%begin`/`%end`/`%error`
  blocks matched by time and number, `%output` and `%extended-output` decoded to raw bytes
  (octal escapes, and psmux's `\\`), the structure notifications `live.ts` listens for,
  `%pause`/`%continue`, `%subscription-changed`, `%exit`. Argument quoting for tmux's parser so
  any text fits on one command line.
- `client.rs`: a connection over any reader/writer pair, so the same code runs over a local
  process, an ssh channel or a node-link stream. One reader thread; replies matched to commands
  in order; notifications on an unbounded channel so tmux never sees a slow client because of
  us; `wait_ready` for the attach.
- `tmux.rs`: the hub's commands over that connection: `list-panes -a` in the hub's PANE_FORMAT,
  `capture-pane -p -e` (one or many in one write), `send-keys` with the hub's key list,
  `send-keys -l` typing, bracketed paste with `set-buffer` chunks and `paste-buffer -d -p` (no
  temp file, no shell), `resize-window`, `resize-pane`, version-aware attach flags.

## Tested

16 unit tests and 21 tests against a real tmux, each on a private server
(`tmux -L inyon-s3-<test>`, killed at the end). Locally on tmux 3.6, and in CI
(`.github/workflows/spike-s3.yml`, copy in `ci.yml`) against 3.0a, 3.2a, 3.3a, 3.4, 3.5a and 3.6a
built from the release tarballs: all green in run 39123119079. The tests cover listing (a UTF-8
session name, three windows, a split), styled capture, `%output` with backslashes, control bytes,
UTF-8 and escapes, typing quotes, `$HOME`, `;`, `#{pane_id}` and `--` literally, bracketed paste
(small, 113 KB, and to a program that didn't ask for brackets), resizing, structure events,
`%exit` on session and server death, detaching, 2,000 pipelined commands, a reader that stops for
3 s, `pause-after`, subscriptions and the hub's own attach arguments.

## Numbers (CI runners, per version)

| tmux | 2,000 pipelined commands | one tmux process per command | 1.9 MB of pane output | 113 KB paste |
|---|---|---|---|---|
| 3.0a | 147 ms (73 µs each) | 1.8 ms | 0.76 s, nothing lost | 31 ms |
| 3.2a | 15 ms (7.6 µs) | 1.6 ms | 0.59 s, nothing lost | 30 ms |
| 3.3a | 21 ms (11 µs) | 2.0 ms | 0.74 s, nothing lost | 33 ms |
| 3.4 | 29 ms (15 µs) | 2.4 ms | 0.78 s, nothing lost | 34 ms |
| 3.5a | 25 ms (13 µs) | 2.2 ms | 0.72 s, nothing lost | 34 ms |
| 3.6a | 16 ms (8 µs) | 1.7 ms | 0.60 s, nothing lost | 31 ms |

The process column is the floor for today's `host.exec` per command; over ssh or a node link
each one is also a network round trip, where a control connection pays that once. tmux itself
caps pane output at 2.5–3.4 MB/s here, and the escaped stream on the wire is about twice the
pane bytes. A reader that stopped for 3 s while 200,000 lines went by lost nothing: tmux holds the
backlog.

## What the tests found

1. **On tmux 3.0 and 3.1 the hub's live client never attaches.** `live.ts` always passes
   `-f read-only,ignore-size`, and `attach-session -f` only exists from 3.2, so the attach fails
   and the hub quietly falls back to polling. Ubuntu 20.04 ships 3.0a and Debian 11 ships 3.1c.
   Fix: pass `-r` and no `-f` before 3.2 (`attach_args`), which also costs nothing, since those
   versions don't size windows from a control client (checked on 3.0a).
2. **`read-only` doesn't stop commands.** A read-only control client could `send-keys` on every
   version. It only blocks the client's own keyboard, so it is no safety line for Inyon.
3. **Wait for the attach before sending commands.** A command written right after starting
   `tmux -C` can run before the client is attached: 3.3a answered "no current client" in one
   run. tmux answers the attach in a block with flags 0; `wait_ready` waits for it.
4. **`%continue` arrives inside the reply block** of the `refresh-client -A` that caused it,
   although the manual says notifications never do. The parser takes exactly `%pause %N` and
   `%continue %N` out of blocks.
5. **A killed window is reported as `%unlinked-window-close`,** not `%window-close`. `live.ts`
   already listens for both.
6. **tmux 3.0 sends no `%exit`** when the session or the server dies; the stream just ends. The
   client treats EOF as the exit.
7. **tmux 3.2 sizes new windows from the client that made them,** control or one-off command,
   so without a terminal attached they come out 80x24 (`new-session -d -x 120 -y 40` gave 80x23
   too). 3.3 fixed it. This affects the hub on Ubuntu 22.04 (3.2a) today:
   `features/signin.ts` asks for a 220x50 sign-in session and likely gets 80 columns there;
   worth checking on a 3.2 host. The test
   server now resizes its windows the way `terminal/mirror.ts` does. The hub's own attach left
   window sizes alone on every version, five out of five times.
8. **`pause-after` works from 3.2:** `%pause`, `%extended-output` with the age, and
   `refresh-client -A '%N:continue'`. In two CI runs (3.6a once, 3.5a once) a 3 s stall didn't
   trigger a pause in time, so the test now stalls 5 s and only records a miss; the last run
   paused on every version from 3.2. For Inyon, which re-reads the screen
   on any output, `pause-after` is the safe choice for a slow link: it never disconnects.
9. **Subscriptions** (`refresh-client -B`, 3.2+) report `pane_current_command` changes, but in
   our runs only after the pane wrote something. They can't replace the process probe.

## psmux (Windows)

Probed on `windows-latest` with psmux 3.4.0 from its release zip (`tests/psmux.rs`), using the
same client and the hub's `-C attach-session -t <session>`:

| Works | Missing or different |
|---|---|
| Attach, `%begin`/`%end` framing, replies in order | No attach block, so `wait_ready` must not be used; `-C` echoes each command back (ignored as stray lines) |
| `list-panes -a -F` in the hub's format, `list-sessions`, `#{version}` | Pane ids are per session (`%1` everywhere), so the hub's `winLine` rewrite stays |
| `send-keys -l`, named keys, `%output` for what was typed | `send-keys -H` hangs until psmux answers "command timed out" |
| `capture-pane -p -e` with styles | Trailing blank rows are dropped (2 rows for a 30-row pane) |
| `resize-window`, `resize-pane`, `refresh-client -C/-f/-B` accepted | `set-buffer`, `load-buffer`, `paste-buffer`: "unknown command", though `list-commands` lists them |
| | `kill-session` ends the client with `%exit server exited unexpectedly` |

So on Windows a paste can't go over control mode. Keep the hub's current path (`load-buffer` from
a file through the CLI, `pasteCommand` in `tmux.ts`) or psmux's own `send-paste` CLI. psmux's
docs also say a client that stalls 5 s without `pause-after` is dropped, unlike tmux, so the
Windows client should always set `pause-after`. psmux runs one server per session, which matches
the one-client-per-session design.

## Recommendation for inyon-core

- One read-write control client per session (with `ignore-size`, not `read-only`, given finding
  2), carrying both events and the commands that go to that session; `host.exec` stays for the
  probe script, process listing and file reads.
- Version-gated attach flags (finding 1); it's also worth fixing in `live.ts` now.
- On Windows, paste through the CLI path; everything else can share the control client.
- Keep the parser sans-io as here, so it runs over the SSH, node-link and local backends and in
  wasm.
