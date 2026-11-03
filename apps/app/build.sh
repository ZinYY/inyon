#!/usr/bin/env bash
# Builds the browser app into apps/app/dist, which the hub carries and serves at /next
# (crates/inyon-hub/src/static_files.rs). Needs dx at the version below (the Dioxus CLI, a
# prebuilt binary from github.com/DioxusLabs/dioxus/releases), the wasm32-unknown-unknown target, and
# Bun with the workspace installed (bun install) for the terminal's xterm.js.
set -euo pipefail
cd "$(dirname "$0")"
want=0.7.10
have=$(dx --version 2>/dev/null | awk '{print $2}' || true)
if [ "$have" != "$want" ]; then
  echo "dx $want is needed, found ${have:-none}: download dx-<target>.tar.gz from https://github.com/DioxusLabs/dioxus/releases/tag/v$want" >&2
  exit 1
fi
# dx keeps every earlier build's hashed files in its own folder and copies them all out
target=$(cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)
rm -rf "$target/dx/inyon-app/release/web"
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
# --package works around dx resolving the workspace's default-members from the wrong folder;
# no debug symbols, so wasm-opt strips the DWARF it would otherwise keep
dx bundle --platform web --release --package inyon-app --debug-symbols false --out-dir "$out"
rm -rf dist
cp -r "$out/public" dist
# the terminal's drawing (xterm.js, term/), loaded the first time a terminal opens; the page finds
# its hashed name in a meta tag. Needs the workspace's packages (bun install).
term=$(bun term/build.ts dist/assets)
sed -i.bak "s|</head>|<meta name=\"inyon-term\" content=\"/next/assets/$term\"></head>|" dist/index.html
rm dist/index.html.bak
grep -q "assets/$term" dist/index.html
ls -l dist dist/assets
