#!/usr/bin/env bash
# Builds the wasm link for the browser test and prints its sizes.
#   needs: rustup target wasm32-unknown-unknown, wasm-bindgen-cli matching Cargo.lock,
#          wasm-opt (binaryen) on PATH or in WASM_OPT for the -Oz step (skipped without it)
set -euo pipefail
cd "$(dirname "$0")"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
cargo build --release --target wasm32-unknown-unknown
raw=target/wasm32-unknown-unknown/release/s4_wasm.wasm
wasm-bindgen --target web --out-dir web/pkg --no-typescript "$raw"
bg=web/pkg/s4_wasm_bg.wasm
size() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; }
gz() { gzip -9 -c "$1" | wc -c | tr -d ' '; }
echo "cargo --release (opt-level z, lto):  $(size "$raw") bytes"
echo "after wasm-bindgen:                  $(size "$bg") bytes, gzip -9 $(gz "$bg")"
opt="${WASM_OPT:-$(command -v wasm-opt || true)}"
if [ -n "$opt" ]; then
  "$opt" -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals "$bg" -o "$bg.opt"
  mv "$bg.opt" "$bg"
  echo "after wasm-opt -Oz:                  $(size "$bg") bytes, gzip -9 $(gz "$bg")"
fi
# brotli through Bun's zlib, so no extra tool is needed
br=$(bun -e 'const z = require("node:zlib"); console.log(z.brotliCompressSync(require("node:fs").readFileSync(process.argv[1]), { params: { [z.constants.BROTLI_PARAM_QUALITY]: 11 } }).length)' "$bg" 2>/dev/null || true)
[ -n "$br" ] && echo "brotli q11:                          $br"
echo "JS glue (s4_wasm.js):                $(size web/pkg/s4_wasm.js) bytes, gzip -9 $(gz web/pkg/s4_wasm.js)"

# the TypeScript phone link bundled for the browser, for comparison
bun build web/ts-phone.ts --target browser --minify --outdir web/pkg >/dev/null
echo "TS link bundle (ts-phone.js, minified): $(size web/pkg/ts-phone.js) bytes, gzip -9 $(gz web/pkg/ts-phone.js)"
