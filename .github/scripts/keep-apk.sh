#!/bin/sh
# Keeps the APK's line when node.yml's publish job runs again on a tag after android.yml listed the
# APK, so the new SHA256SUMS still names it (the website mirrors a release only when it does):
#   keep-apk.sh <folder with the release's SHA256SUMS, .minisig and APK> <the new SHA256SUMS>
# with the public key in PUBKEY. A line is kept only from a list that carries a good signature from
# PUBKEY, and only for the APK the release serves under that same hash, so nothing on the release
# that wasn't signed already ends up in the new list. Nothing to keep is not an error.
set -eu
[ $# -eq 2 ] || { echo "usage: keep-apk.sh <folder> <new SHA256SUMS>" >&2; exit 2; }
dir=$1 new=$2
[ -n "${PUBKEY:-}" ] || { echo "::error::PUBKEY must be set" >&2; exit 1; }
[ -f "$dir/SHA256SUMS" ] && [ -f "$dir/SHA256SUMS.minisig" ] || exit 0
if ! minisign -V -q -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" -P "$PUBKEY" >/dev/null 2>&1; then
  echo "::warning::the release's SHA256SUMS isn't signed by the release key, so no APK line is kept from it"
  exit 0
fi
for apk in "$dir"/inyon-*.apk; do
  [ -f "$apk" ] || continue
  name=${apk##*/}
  sum=$(awk -v f="$name" '$2 == f { print $1; exit }' "$dir/SHA256SUMS")
  [ -n "$sum" ] || continue
  if [ "$(sha256sum "$apk" | cut -d' ' -f1)" != "$sum" ]; then
    echo "::warning::$name on the release doesn't match its signed line, so it isn't kept"
    continue
  fi
  awk -v f="$name" '$2 == f { found = 1 } END { exit !found }' "$new" && continue
  printf '%s  %s\n' "$sum" "$name" >> "$new"
  echo "Kept $name in SHA256SUMS"
done
