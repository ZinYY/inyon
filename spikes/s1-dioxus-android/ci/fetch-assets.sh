#!/usr/bin/env bash
# Puts xterm.js, its WebGL addon and the Geist fonts in assets/vendor, from the npm registry at the
# versions apps/web uses, checked against the sha512 in the repo's bun.lock.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
lock="$here/../../bun.lock"
out="$here/assets/vendor"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out"

fetch() { # name version file-in-package dest
  local name=$1 version=$2 file=$3 dest=$4
  local base=${name##*/}
  local url="https://registry.npmjs.org/$name/-/$base-$version.tgz"
  local want
  want=$(grep -o "\"$name@$version\", \"\", {[^}]*}, \"sha512-[^\"]*\"" "$lock" | grep -o 'sha512-[^"]*' | head -1)
  [ -n "$want" ] || { echo "no $name@$version in bun.lock" >&2; exit 1; }
  curl -fsSL "$url" -o "$tmp/$base.tgz"
  local got="sha512-$(openssl dgst -sha512 -binary "$tmp/$base.tgz" | base64 -w0)"
  [ "$got" = "$want" ] || { echo "$name@$version: checksum mismatch" >&2; exit 1; }
  mkdir -p "$tmp/$base"
  tar xzf "$tmp/$base.tgz" -C "$tmp/$base"
  cp "$tmp/$base/package/$file" "$out/$dest"
}

fetch @xterm/xterm 6.0.0 lib/xterm.js xterm.js
fetch @xterm/xterm 6.0.0 css/xterm.css xterm.css
fetch @xterm/addon-webgl 0.19.0 lib/addon-webgl.js addon-webgl.js
fetch @fontsource-variable/geist 5.3.0 files/geist-latin-wght-normal.woff2 geist.woff2
fetch @fontsource-variable/geist-mono 5.3.0 files/geist-mono-latin-wght-normal.woff2 geist-mono.woff2
ls -l "$out"
