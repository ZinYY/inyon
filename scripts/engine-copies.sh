#!/bin/sh
# engine-copies.sh <dist> [system…]: the release's inyon-engine-* files, made from the inyon-* ones
# in <dist>, for every system (linux-x64 linux-arm64 darwin-x64 darwin-arm64 windows-x64) or the
# ones named.
#
# From 0.23 inyon is the engine too, but inyon 0.12 to 0.22 still fetch inyon-engine-<system> as
# they update, and refuse a release without it (0.13 to 0.18), so each is a copy of the new inyon,
# which answers the engine's commands under that name. Linux's inyon is static, so it stands in for
# the musl engine as well. Windows on ARM ran the x64 engine, and the Windows copy is marked as a
# program without a console (the PE header's subsystem set to 2, as `inyon` makes its own copy),
# since logon tasks from 0.13 to 0.18 start it directly. Each file is gzipped too.
set -eu
dist=${1:?usage: engine-copies.sh <dist> [system…]}
shift
[ $# -gt 0 ] || set -- linux-x64 linux-arm64 darwin-x64 darwin-arm64 windows-x64
cd "$dist"

# the byte at <offset> in <file>, as a number
byte() { od -An -tu1 -j"$2" -N1 "$1" | tr -d ' '; }

windowless() {
  pe=$(( $(byte "$1" 60) + $(byte "$1" 61) * 256 + $(byte "$1" 62) * 65536 + $(byte "$1" 63) * 16777216 ))
  [ "$(dd if="$1" bs=1 skip="$pe" count=4 2>/dev/null | od -An -c | tr -d ' ')" = 'PE\0\0' ] || { echo "engine-copies: $1 isn't a Windows program" >&2; exit 1; }
  printf '\002\000' | dd of="$1" bs=1 seek=$(( pe + 4 + 20 + 68 )) conv=notrunc 2>/dev/null
}

made=""
copy() {
  [ -f "$1" ] || { echo "engine-copies: $1 is missing" >&2; exit 1; }
  cp "$1" "$2"
  chmod +x "$2"
  made="$made $2"
}
for system in "$@"; do
  case "$system" in
    linux-x64 | linux-arm64)
      copy "inyon-$system" "inyon-engine-$system"
      copy "inyon-$system" "inyon-engine-$system-musl" ;;
    darwin-x64 | darwin-arm64) copy "inyon-$system" "inyon-engine-$system" ;;
    windows-x64)
      copy inyon-windows-x64.exe inyon-engine-windows-x64.exe
      windowless inyon-engine-windows-x64.exe ;;
    *) echo "engine-copies: no engine for $system" >&2; exit 1 ;;
  esac
done

for f in $made; do gzip -9 -n -k -f "$f"; done
