#!/bin/sh
# Lists a release's APK in its SHA256SUMS and signs the list again (android.yml's publish job, on a
# tag, once node.yml has published and signed the rest of the release):
#   list-apk.sh <folder with the release's SHA256SUMS and .minisig> <the APK built here> <the APK as the release serves it>
# with the minisign secret key (no password) in KEY and the public key it belongs to in PUBKEY.
# Nothing is signed unless the list as it stands carries a good signature from PUBKEY, and the
# APK on the release is the one built here: the new list is the signed one plus that one line, so a
# list or a file someone else put on the release can't come out of this job signed.
set -eu
[ $# -eq 3 ] || { echo "usage: list-apk.sh <folder> <apk> <apk from the release>" >&2; exit 2; }
dir=$1 apk=$2 served=$3
name=${apk##*/}
fail() { echo "::error::$1" >&2; exit 1; }
[ -n "${KEY:-}" ] && [ -n "${PUBKEY:-}" ] || fail "KEY and PUBKEY must be set"
case "$name" in inyon-*.apk) ;; *) fail "$name isn't named like a release's APK (inyon-<version>.apk)" ;; esac
[ -f "$dir/SHA256SUMS.minisig" ] || fail "the release's SHA256SUMS has no signature, so it isn't signed again"
minisign -V -q -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" -P "$PUBKEY" >/dev/null 2>&1 ||
  fail "the release's SHA256SUMS isn't signed by the release key, so it isn't signed again"
sum=$(sha256sum "$apk" | cut -d' ' -f1)
[ -f "$served" ] && [ "$(sha256sum "$served" | cut -d' ' -f1)" = "$sum" ] ||
  fail "$name on the release isn't the one this job built, so it isn't listed"

# the signed list less any line for this APK (from an earlier run of this job), plus the APK
awk -v f="$name" '$2 != f && $2 != "*" f' "$dir/SHA256SUMS" > "$dir/SHA256SUMS.new"
printf '%s  %s\n' "$sum" "$name" >> "$dir/SHA256SUMS.new"
mv "$dir/SHA256SUMS.new" "$dir/SHA256SUMS"

# minisign wants the key file whole, a comment line above the key; it is gone again when this exits
keyfile=$(mktemp)
trap 'rm -f "$keyfile"' EXIT
(umask 077 && { case "$KEY" in "untrusted comment:"*) ;; *) echo "untrusted comment: minisign secret key" ;; esac; printf '%s\n' "$KEY"; } > "$keyfile")
minisign -S -s "$keyfile" -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" < /dev/null >/dev/null
minisign -V -q -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" -P "$PUBKEY" >/dev/null || fail "the new signature doesn't check out against PUBKEY"
echo "Listed $name in SHA256SUMS and signed it again"
