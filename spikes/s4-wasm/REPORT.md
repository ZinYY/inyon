# S4: the phone's link as wasm in a browser

**Verdict: go** for running `inyon-core` as wasm inside the React app in 0.23, with two things to
watch: the bundle is several times larger than the TypeScript link it replaces, and nothing here
ran in the Android WebView yet.

## What was built

A small crate (`spikes/s4-wasm`, its own workspace) with the phone side of `packages/link` in
Rust, built for `wasm32-unknown-unknown` with wasm-bindgen:

- the identity from `identity.ts`: ed25519 seed, z-base-32 id, the X25519 key derived from it
  (byte-identical to noble's, checked against a vector printed by Bun);
- Noise_IK_25519_ChaChaPoly_SHA256 through `snow`, with the `tether-link 1` prologue, the welcome
  check and the 60000-byte chunking with the flag byte, as in `channel.ts`;
- a `connect(url, machineId, seed, hello)` export that owns a browser `WebSocket` (web-sys) and
  returns a `Link` with `send`, `setOnMessage`, `binding` and `signBinding`.

The crates and features are the ones `crates/inyon-link` uses (snow 0.10 without default
features, ed25519-dalek 2, curve25519-dalek 4, getrandom 0.3 with `wasm_js`), so the sizes are
the real ones. It does not depend on that crate.

The other side is `web/server.ts`: Bun serving the page and answering on `/link` with
`accept()` from `packages/link`, unchanged. `web/run.ts` opens the page in headless Chromium
(Playwright, chrome-headless-shell 1243 on the ARM box) twice and prints what the page measured.

One difference from noble turned up: ed25519-dalek's `to_scalar_bytes()` gives the X25519 secret
unclamped, noble's `toMontgomerySecret` clamped. X25519 clamps either way, so the keys agree, but
the Rust side clamps too so stored bytes match.

## What the browser test proves

All nine checks pass on every run (`bun web/run.ts`, after `./build.sh`):

| Check | Result |
|---|---|
| Handshake from Rust/wasm to the TS machine, welcome read | ok |
| Phone's ed25519 signature over the handshake hash verifies in TS with the phone's id | ok |
| Handshake hash identical on both ends | ok |
| 450 KB of multi-byte text echoed through the chunking both ways | ok |
| 100,000 × "é" from TS, so a 60000-byte piece ends inside a character, joined in Rust | ok |
| Wrong machine key: TS can't read message 1 and closes with 4401 | ok |
| A Noise key that isn't the phone named in the hello: TS refuses, Rust reads the encrypted reason | ok |
| 50 in-memory handshakes + 64 KB each way, in wasm and in TS | ok |

Native tests (`cargo test`): 7 pass, covering the TS identity vector, sign/verify, z-base-32,
chunking, a wrong key, a refusal and a tampered message.

## Sizes

| Build | Bytes | gzip -9 | brotli 11 |
|---|---|---|---|
| `cargo build --release` (opt z, LTO, panic abort) | 588,426 | | |
| after wasm-bindgen | 217,930 | 105,663 | |
| after `wasm-opt -Oz` (binaryen 133) | 198,166 | 104,574 | 93,356 |
| JS glue `s4_wasm.js` | 26,557 | 5,505 | |
| For comparison: the TS link bundled and minified by Bun | 57,366 | 22,391 | |

The link alone costs about 110 KB gzipped as wasm plus glue, against 22 KB for the TypeScript
link. Most of the wasm is curve25519, ed25519, ChaCha20-Poly1305 and serde_json; `inyon-core` on
top adds its own code but shares these. The app loads it locally (from the APK or the hub), so
this is disk and parse time, not a download on every open.

## Startup and speed

Chromium headless on the 2-core ARM box, shared with three other builds, so the network numbers
are noisy. Ranges over four runs of two page loads each:

| | wasm (Rust) | TypeScript |
|---|---|---|
| Fetch the module (second fetch; the first paid ~210 ms for the page's first request) | 9–27 ms | |
| Compile and instantiate | 2.7–12 ms | |
| Load the bundled TS link | | 15–34 ms |
| Handshake over the WebSocket to Bun | 28–104 ms | 61–157 ms |
| One handshake + 64 KB each way, in memory | 5.7–11.6 ms | 8.9–12.5 ms |
| Echo round trip, small message | 0.75–2.3 ms | |

Startup is not a problem: compiling the module takes less time than loading the JS link. The
crypto is a little faster in wasm. In each page the wasm handshake ran before the TS one, so that
row is not a fair race, only proof that both land in the same range.

## What this does not cover

- The Android WebView. The phone is slower than this box; the APK should be measured on a real
  phone before 0.23 commits to it.
- `inyon-core` itself: hosts, tmux, agents and the `Store`/`HostBackend`/runtime traits don't
  exist yet. This spike shows the toolchain, the async bridge (wasm-bindgen-futures around a JS
  Promise) and JS callbacks from Rust work; it says nothing about the core's own size.
- The relay and the tunnel. Both carry the same ciphertext over a WebSocket, so nothing in them
  should differ, but only the direct connection was run.

## How to run it

```sh
cd spikes/s4-wasm
cargo test
WASM_OPT=/path/to/wasm-opt ./build.sh     # needs wasm-bindgen-cli 0.2.129 on PATH
bun web/run.ts                            # after bun install at the repo root
```
