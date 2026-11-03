#!/bin/sh
# Adds this machine to an Inyon hub: gets the inyon command from the hub and runs its connection
# to the hub in the background, as the current user.
# Usage: INYON_TOKEN=<token> INYON_SUMS_SHA256=<sha256> sh install.sh <hub-url>
# (the hub's "Add a machine" command does this; `sh -s -- <hub-url> <token>` still works)
# The token comes from the environment so it doesn't show in the process list. With
# INYON_SUMS_SHA256 set, the hub's list of checksums must match it, and the program its line.
# INYON_HUB_CERT is the sha256 of a certificate the hub made itself (setup's LAN choice): the
# downloads can't check it and don't need to, being checked against INYON_SUMS_SHA256, and
# `inyon join` trusts only that certificate for the token.
set -e
HUB="${1%/}"; TOKEN="${2:-${INYON_TOKEN:-$TETHER_TOKEN}}"
[ -n "$HUB" ] && [ -n "$TOKEN" ] || { echo "usage: INYON_TOKEN=<token> install.sh <hub-url>" >&2; exit 2; }

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) echo "This is for Linux and macOS. On Windows, use the PowerShell command from the hub." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *) echo "There's no inyon build for $(uname -m) yet." >&2; exit 1 ;;
esac
name="inyon-$os-$arch"
sum() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -c1-64; }
k=
if [ -n "$INYON_HUB_CERT" ]; then
  [ -n "$INYON_SUMS_SHA256" ] || { echo "INYON_HUB_CERT needs INYON_SUMS_SHA256: get a new command from the hub." >&2; exit 2; }
  k=-k
fi

DIR="$HOME/.inyon/bin"
mkdir -p "$DIR"
tmp="$DIR/inyon.download"
trap 'rm -f "$tmp" "$tmp.sums"' EXIT
curl -fsSL $k "$HUB/node/SHA256SUMS" -o "$tmp.sums"
if [ -n "$INYON_SUMS_SHA256" ] && [ "$(sum "$tmp.sums")" != "$INYON_SUMS_SHA256" ]; then
  echo "The hub's checksums did not match. Get a new command from the hub and run it again." >&2
  exit 1
fi
want="$(awk -v f="$name" '$2 == f || $2 == "*" f { print $1 }' "$tmp.sums")"
echo "Downloading inyon from the hub…"
curl -fL $k --progress-bar "$HUB/node/inyon?os=$os&arch=$arch" -o "$tmp"
if [ -z "$want" ] || [ "$(sum "$tmp")" != "$want" ]; then
  echo "inyon did not match its checksum. Get a new command from the hub and run it again." >&2
  exit 1
fi
chmod +x "$tmp"
mv -f "$tmp" "$DIR/inyon"
# Apple Silicon only runs signed programs; a local signature is enough
[ "$os" = darwin ] && command -v codesign >/dev/null 2>&1 && codesign --force --sign - "$DIR/inyon" >/dev/null 2>&1 || true

# saves the hub and token (readable only by you) and starts the connection in the background
INYON_TOKEN="$TOKEN" "$DIR/inyon" join "$HUB"
