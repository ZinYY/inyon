#!/bin/sh
# Installs inyon in fresh Linux containers from a mock of this release, sets up a hub, and
# upgrades real 0.12.0 and 0.17.6 installs to it. The release's inyon-engine-* files are copies of
# inyon (scripts/engine-copies.sh), which those old versions fetch as they update. Needs docker and
# minisign (apt-get install minisign / brew install minisign); nothing else is installed on this
# machine.
#
#   scripts/container-tests.sh                 builds the command (cargo, static musl) for this
#                                              machine's processor first
#   INYON_DIST=<dir> scripts/container-tests.sh  takes inyon-linux-<arch>[.gz] from <dir> (CI)
#   INYON_TEST_SYSTEMS="alpine upgrade" …       runs only those (ubuntu debian alpine fedora hub
#                                              upgrade upgrade-0.17)
#
# The release is served from a container on a network with no way out, so a test that reached
# the website or GitHub would fail. Fresh installs read it the way the website serves it (/download,
# with latest.json); the 0.12.0 program in the upgrade test reads it the way GitHub does, as
# installs from before the website still do. The downloads from GitHub are those old programs,
# kept in ~/.cache/inyon-tests. The mock's SHA256SUMS is signed with apps/cli/tests/keys/test.key,
# and the containers get that key's public half as INYON_SIGNING_KEY, which get.sh and the
# command take instead of the release key when downloads come from somewhere else.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *) echo "no inyon build for $(uname -m)" >&2; exit 1 ;;
esac
command -v minisign >/dev/null 2>&1 || { echo "minisign is needed to sign the mock release: apt-get install minisign, or brew install minisign" >&2; exit 1; }
cli="inyon-linux-$arch"
engine="inyon-engine-linux-$arch"
engine_musl="inyon-engine-linux-$arch-musl"
keys="$root/apps/cli/tests/keys"
pubkey=$(sed -n 2p "$keys/test.pub")
version=${INYON_TEST_VERSION:-0.24.0}
old=0.12.0
old17=0.17.6
work=$(mktemp -d "${TMPDIR:-/tmp}/inyon-containers.XXXXXX")
cache=${INYON_TEST_CACHE:-$HOME/.cache/inyon-tests}
id=$$
net="inyon-test-$id"
failed=""

say() { printf '\n== %s\n' "$*"; }
cleanup() {
  docker rm -f "release-$id" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

# ---------- the release ----------
dist="$work/dist"
mkdir -p "$dist"
if [ -n "${INYON_DIST:-}" ]; then
  if [ -f "$INYON_DIST/$cli" ]; then cp "$INYON_DIST/$cli" "$dist/"
  elif [ -f "$INYON_DIST/$cli.gz" ]; then gzip -dc "$INYON_DIST/$cli.gz" > "$dist/$cli"
  else echo "$INYON_DIST has no $cli" >&2; exit 1
  fi
else
  say "building the command for linux-$arch"
  target="$(uname -m)-unknown-linux-musl"
  rustup target add "$target" >/dev/null
  # the TLS library has C in it; without a musl cross compiler the host one does, for the same processor
  cc_var="CC_$(printf '%s' "$target" | tr '-' '_')"
  if command -v musl-gcc >/dev/null 2>&1; then cc=musl-gcc; else cc=cc; fi
  # the hub in the command carries the web app, so it's built first
  [ -f "$root/apps/web/dist/index.html" ] || (cd "$root" && VITE_INYON_PHONE_CORE=none bun run build >/dev/null)
  (cd "$root/apps/cli" && env "$cc_var=$cc" cargo build --release --target "$target" --quiet)
  cp "$root/target/$target/release/inyon" "$dist/$cli"
fi
chmod +x "$dist/$cli"
# the engine files, as the release makes them
sh "$root/scripts/engine-copies.sh" "$dist" "linux-$arch"
cp "$root/installers/get.sh" "$dist/"
# a release lists its APK, whose name carries the version: `inyon update` checks the signed list
# against the version latest.json names that way, and get.sh finds the release's tag by it
apk="inyon-$version.apk"
printf 'not an app\n' > "$dist/$apk"
(cd "$dist" && gzip -9 -k "$cli" && sha256sum "$cli" "$cli.gz" "$engine" "$engine.gz" "$engine_musl" "$engine_musl.gz" "$apk" > SHA256SUMS)
# signed as a release is, with the test key (no password; minisign still reads one from stdin)
minisign -S -s "$keys/test.key" -m "$dist/SHA256SUMS" -x "$dist/SHA256SUMS.minisig" < /dev/null
minisign -V -q -m "$dist/SHA256SUMS" -x "$dist/SHA256SUMS.minisig" -P "$pubkey"
# the website serves get.sh with the sha256 of the list it checked filled in (installers/installers.ts),
# so the containers, which have no minisign, install as they would from the website
list_sha=$(sha256sum "$dist/SHA256SUMS" | cut -d' ' -f1)
sed -i "s/^LIST_SHA256=\$/LIST_SHA256=$list_sha/" "$dist/get.sh"
grep -q "^LIST_SHA256=$list_sha\$" "$dist/get.sh" || { echo "couldn't fill in the list's sha256 in get.sh" >&2; exit 1; }
say "the mock release $version"
cat "$dist/SHA256SUMS"

docker network create --internal "$net" >/dev/null
docker run -d --rm --name "release-$id" --network "$net" --network-alias release \
  -v "$dist:/dist:ro" -v "$root/scripts/mock-release.py:/srv/mock-release.py:ro" \
  python:3.12-alpine python3 -I /srv/mock-release.py --dir /dist --version "$version" --port 8000 >/dev/null
site="http://release:8000/download"
github="http://release:8000/releases/latest/download"

# ---------- the images: each base with curl, which get.sh needs ----------
image() {
  name="inyon-test-$1"
  case "$1" in
    ubuntu) from=ubuntu:24.04; add='RUN apt-get update -q && apt-get install -y -q --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*' ;;
    debian) from=debian:bookworm-slim; add='RUN apt-get update -q && apt-get install -y -q --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*' ;;
    # the inyon command is static: no libstdc++ or libgcc, which a fresh Alpine doesn't have
    alpine) from=alpine:3.20; add='RUN apk add --no-cache curl' ;;
    fedora) from=fedora:40; add='RUN dnf install -y -q curl && dnf clean all' ;;
  esac
  mkdir -p "$work/ctx"
  printf 'FROM %s\n%s\n' "$from" "$add" | docker build -q -t "$name" -f - "$work/ctx" >/dev/null
  echo "$name"
}

# ---------- a fresh install on each system (each container reaches the mock release and nothing else) ----------
fresh='set -e
curl -fsSL http://release:8000/get.sh | sh
echo "--- after setup"
ls -l ~/.inyon/bin
# a computer runs on inyon alone
test ! -e ~/.inyon/bin/inyon-engine || { echo "the engine was downloaded"; exit 1; }
~/.inyon/bin/inyon version
~/.inyon/bin/inyon status --plain
~/.inyon/bin/inyon status --plain | grep -q "^link: running" || { echo "the link is not running"; cat ~/.inyon/node/link.log 2>/dev/null; exit 1; }
~/.inyon/bin/inyon engine version
echo "--- the newest version, from latest.json"
~/.inyon/bin/inyon update | tee /tmp/update.out
grep -q "is up to date" /tmp/update.out || { echo "inyon update did not find itself up to date"; exit 1; }
echo "--- a new shell finds inyon by name"
# the startup file of the shell in $SHELL: ~/.bashrc on Fedora, whose image sets it, ~/.profile without one
rc=$(grep -l "added by Inyon" ~/.profile ~/.bashrc ~/.zshrc 2>/dev/null | head -n 1)
[ -n "$rc" ] || { echo "no startup file has the PATH line"; exit 1; }
echo "$rc"
"${SHELL:-sh}" -c ". $rc; command -v inyon" | grep -q "/.inyon/bin/inyon$" || { echo "inyon is not on the PATH of a new shell"; exit 1; }
echo "--- uninstall --purge leaves nothing behind"
~/.inyon/bin/inyon uninstall --purge
if grep -q "added by Inyon" "$rc"; then echo "the PATH line is still in $rc"; exit 1; fi
test ! -e ~/.inyon || { echo "~/.inyon is still there"; ls -la ~/.inyon; exit 1; }'
systems=${INYON_TEST_SYSTEMS:-ubuntu debian alpine fedora hub upgrade upgrade-0.17}
has() { case " $systems " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
for sys in ubuntu debian alpine fedora; do
  has "$sys" || continue
  say "fresh install on $sys"
  img=$(image "$sys")
  if timeout 300 docker run --rm --network "$net" -e "INYON_DOWNLOAD=$site" -e "INYON_SIGNING_KEY=$pubkey" "$img" sh -c "$fresh"; then
    echo "== $sys: ok"
  else
    echo "== $sys: FAILED"
    failed="$failed $sys"
  fi
done

# ---------- a hub: on the inyon command, and the Bun hub's switch no longer read ----------
hub='set -e
curl -fsSL http://release:8000/get.sh | sh -s -- --hub
echo "--- after setup"
ls -l ~/.inyon/bin
test ! -e ~/.inyon/bin/inyon-engine || { echo "the engine was downloaded for a hub"; exit 1; }
~/.inyon/bin/inyon status --plain
~/.inyon/bin/inyon status --plain | grep -q "^hub: running" || { echo "the hub is not running"; cat ~/.inyon/node/hub.log 2>/dev/null; exit 1; }
curl -fsS http://127.0.0.1:4777/ | grep -qi "<html" || { echo "the hub does not serve the app"; exit 1; }
~/.inyon/bin/inyon doctor | grep -q "engine *built in" || { echo "doctor does not say the engine is built in"; ~/.inyon/bin/inyon doctor; exit 1; }
echo "--- the switch back to the Bun hub, removed in 0.23, is said and ignored"
echo INYON_HUB_IMPL=bun >> ~/.inyon/hub.env
~/.inyon/bin/inyon hub restart 2>&1 | tee /tmp/restart.out
grep -q "INYON_HUB_IMPL=bun is no longer read" /tmp/restart.out || { echo "hub restart did not say the switch is gone"; exit 1; }
test ! -e ~/.inyon/bin/inyon-engine || { echo "the restart fetched an engine"; exit 1; }
for i in 1 2 3 4 5 6 7 8 9 10; do curl -fsS http://127.0.0.1:4777/ >/dev/null 2>&1 && break; sleep 1; done
curl -fsS http://127.0.0.1:4777/ >/dev/null || { echo "the hub does not answer after the restart"; cat ~/.inyon/node/hub.log 2>/dev/null; exit 1; }
~/.inyon/bin/inyon doctor | grep -q "INYON_HUB_IMPL=bun is no longer read" || { echo "doctor does not list the switch"; exit 1; }
echo "--- an update fetches no engine"
~/.inyon/bin/inyon update | tee /tmp/update.out
grep -q "is up to date" /tmp/update.out
test ! -e ~/.inyon/bin/inyon-engine'
if has hub; then
  say "a hub on ubuntu"
  if timeout 300 docker run --rm --network "$net" -e "INYON_DOWNLOAD=$site" -e "INYON_SIGNING_KEY=$pubkey" "$(image ubuntu)" sh -c "$hub"; then
    echo "== hub: ok"
  else
    echo "== hub: FAILED"
    failed="$failed hub"
  fi
fi

# ---------- an upgrade from a real 0.12.0 install ----------
if has upgrade; then
say "upgrade from $old on ubuntu"
mkdir -p "$cache"
if [ ! -f "$cache/inyon-$old-linux-$arch" ]; then
  echo "downloading inyon $old (once; kept in $cache)"
  curl -fsSL "https://github.com/Abhash-Chakraborty/inyon/releases/download/v$old/SHA256SUMS" -o "$cache/SHA256SUMS-$old"
  curl -fL --progress-bar "https://github.com/Abhash-Chakraborty/inyon/releases/download/v$old/$cli.gz" -o "$cache/old.gz"
  want=$(awk -v f="$cli.gz" '$2 == f { print $1 }' "$cache/SHA256SUMS-$old")
  [ "$(sha256sum "$cache/old.gz" | cut -d' ' -f1)" = "$want" ] || { echo "the $old download didn't match its checksum" >&2; exit 1; }
  gzip -dc "$cache/old.gz" > "$cache/inyon-$old-linux-$arch"
  rm -f "$cache/old.gz"
fi
mkdir -p "$work/old"
cp "$cache/inyon-$old-linux-$arch" "$work/old/inyon"
chmod +x "$work/old/inyon"
upgrade='set -e
mkdir -p ~/.inyon/bin ~/.inyon/node
cp /old/inyon ~/.inyon/bin/inyon
# what a paired machine has: its identity and one phone
printf "%s" "{\"seed\":\"3f1a9c2e4b7d6058a1c3e5f7092b4d6e8f1a3c5e7092b4d6e8f1a3c5e7092b4d\",\"id\":\"k7m2p9x4w1q8z3v6\"}" > ~/.inyon/node/identity.json
printf "%s" "{\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2\":{\"name\":\"Pixel 9\",\"pairedAt\":1759300000000,\"lastSeen\":1760300000000}}" > ~/.inyon/node/phones.json
before=$(sha256sum ~/.inyon/node/phones.json)
~/.inyon/bin/inyon version
echo "--- the old program updates itself"
~/.inyon/bin/inyon update
echo "--- after the update"
ls -l ~/.inyon/bin
test "$before" = "$(sha256sum ~/.inyon/node/phones.json)" || { echo "phones.json changed"; exit 1; }
~/.inyon/bin/inyon version | grep -q "^inyon '"$version"'" || { echo "~/.inyon/bin/inyon is not the new command"; ~/.inyon/bin/inyon version; exit 1; }
~/.inyon/bin/inyon engine version
~/.inyon/bin/inyon phones | grep "Pixel 9"
~/.inyon/bin/inyon status --plain | grep -q "machine k7m2p9x4w1q8z3v6"
# the engine 0.12.0 fetched with it is the new inyon, answering as the engine
~/.inyon/bin/inyon-engine version | grep -qx "'"$version"'" || { echo "inyon-engine is not the new version"; ~/.inyon/bin/inyon-engine version; exit 1; }
# the old program keeps its copy aside (inyon.old-<time>) while nothing restarted on the new
# one; the next start removes it. No download may be left behind
test -z "$(ls ~/.inyon/bin | grep download)" || { echo "a download was left in bin"; exit 1; }
echo "--- the link starts on inyon alone and drops the engine the old program fetched"
timeout 10 ~/.inyon/bin/inyon link --no-tunnel --no-vpn > /tmp/link.out 2>&1 || true
grep -q "link service for" /tmp/link.out || { echo "the link did not start"; cat /tmp/link.out; exit 1; }
test ! -e ~/.inyon/bin/inyon-engine || { echo "the engine is still there"; exit 1; }'
if timeout 300 docker run --rm --network "$net" -e "INYON_DOWNLOAD=$github" -e "INYON_SIGNING_KEY=$pubkey" -v "$work/old:/old:ro" "$(image ubuntu)" sh -c "$upgrade"; then
  echo "== upgrade: ok"
else
  echo "== upgrade: FAILED"
  failed="$failed upgrade"
fi
fi

# ---------- an upgrade from a real 0.17.6 install, on the Bun engine ----------
# 0.13 to 0.18 refuse a release without inyon-engine, take it with the command and run the jobs
# on it; from 0.23 it's a copy of the new inyon
if has upgrade-0.17; then
say "upgrade from $old17 on ubuntu"
mkdir -p "$cache"
for f in "$cli" "$engine"; do
  [ -f "$cache/$f-$old17" ] && continue
  echo "downloading $f $old17 (once; kept in $cache)"
  curl -fsSL "https://github.com/Abhash-Chakraborty/inyon/releases/download/v$old17/SHA256SUMS" -o "$cache/SHA256SUMS-$old17"
  curl -fL --progress-bar "https://github.com/Abhash-Chakraborty/inyon/releases/download/v$old17/$f.gz" -o "$cache/old.gz"
  want=$(awk -v f="$f.gz" '$2 == f { print $1 }' "$cache/SHA256SUMS-$old17")
  [ "$(sha256sum "$cache/old.gz" | cut -d' ' -f1)" = "$want" ] || { echo "the $old17 $f download didn't match its checksum" >&2; exit 1; }
  gzip -dc "$cache/old.gz" > "$cache/$f-$old17"
  rm -f "$cache/old.gz"
done
mkdir -p "$work/old17"
cp "$cache/$cli-$old17" "$work/old17/inyon"
cp "$cache/$engine-$old17" "$work/old17/inyon-engine"
chmod +x "$work/old17/inyon" "$work/old17/inyon-engine"
upgrade17='set -e
mkdir -p ~/.inyon/bin ~/.inyon/node
cp /old/inyon /old/inyon-engine ~/.inyon/bin/
printf "%s" "{\"seed\":\"3f1a9c2e4b7d6058a1c3e5f7092b4d6e8f1a3c5e7092b4d6e8f1a3c5e7092b4d\",\"id\":\"k7m2p9x4w1q8z3v6\"}" > ~/.inyon/node/identity.json
printf "%s" "{\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2\":{\"name\":\"Pixel 9\",\"pairedAt\":1759300000000,\"lastSeen\":1760300000000}}" > ~/.inyon/node/phones.json
before=$(sha256sum ~/.inyon/node/phones.json)
~/.inyon/bin/inyon version | grep -q "'"$old17"'" || { echo "not the old command"; exit 1; }
~/.inyon/bin/inyon-engine version | grep -q "'"$old17"'" || { echo "not the old Bun engine"; exit 1; }
echo "--- the old program updates itself and its engine"
~/.inyon/bin/inyon update
echo "--- after the update"
ls -l ~/.inyon/bin
test "$before" = "$(sha256sum ~/.inyon/node/phones.json)" || { echo "phones.json changed"; exit 1; }
~/.inyon/bin/inyon version | grep -q "^inyon '"$version"'" || { echo "~/.inyon/bin/inyon is not the new command"; ~/.inyon/bin/inyon version; exit 1; }
cmp ~/.inyon/bin/inyon ~/.inyon/bin/inyon-engine || { echo "the engine is not a copy of the new inyon"; exit 1; }
~/.inyon/bin/inyon-engine version | grep -qx "'"$version"'" || { echo "inyon-engine does not answer as the new version"; exit 1; }
~/.inyon/bin/inyon phones | grep "Pixel 9"
test -z "$(ls ~/.inyon/bin | grep download)" || { echo "a download was left in bin"; exit 1; }
echo "--- the link starts on inyon alone and drops the copy"
timeout 10 ~/.inyon/bin/inyon link --no-tunnel --no-vpn > /tmp/link.out 2>&1 || true
grep -q "link service for" /tmp/link.out || { echo "the link did not start"; cat /tmp/link.out; exit 1; }
test ! -e ~/.inyon/bin/inyon-engine || { echo "the engine is still there"; exit 1; }'
if timeout 300 docker run --rm --network "$net" -e "INYON_DOWNLOAD=$site" -e "INYON_SIGNING_KEY=$pubkey" -v "$work/old17:/old:ro" "$(image ubuntu)" sh -c "$upgrade17"; then
  echo "== upgrade-0.17: ok"
else
  echo "== upgrade-0.17: FAILED"
  failed="$failed upgrade-0.17"
fi
fi

if [ -n "$failed" ]; then
  say "failed:$failed"
  exit 1
fi
say "all container tests passed"
