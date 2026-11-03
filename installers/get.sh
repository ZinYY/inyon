#!/bin/sh
# Gets inyon for this computer and sets it up for the Inyon phone app:
#   curl -fsSL https://inyon.tech/get.sh | sh
# It downloads the inyon command for this system into ~/.inyon/bin from the website's copy of the
# newest release, checks it against the release's signed checksums, and runs its setup, which
# starts it in the background at login and shows a code to scan with the app. With --hub it sets
# up an Inyon hub instead (the server the app signs in to), which from 0.21 runs on the inyon
# command too:
#   curl -fsSL …/get.sh | sh -s -- --hub
set -e
# The sha256 of the checksum list this copy was published with. The website fills it in when it
# publishes a release, after checking the list's signature against the release key, so the copy
# it serves needs no other tool to trust that list. Empty here: anywhere else, minisign checks it.
LIST_SHA256=
BASE="${INYON_DOWNLOAD:-${TETHER_DOWNLOAD:-https://inyon.tech/download}}"
# the minisign key a release's SHA256SUMS is signed with (the inyon command carries the same one)
KEY=RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6
SITE=https://inyon.tech
# the website's earlier address, which redirects to the same path here, so it counts as the website
OLD_SITE=https://inyon.abhashchakraborty.tech
GITHUB=https://github.com/Abhash-Chakraborty/inyon/releases
# a mock release for tests signs with a key of its own; it only counts with downloads pointed
# somewhere other than the website or GitHub
case "$BASE" in
  "$SITE" | "$SITE"/* | http://inyon.tech | http://inyon.tech/* | "$OLD_SITE" | "$OLD_SITE"/* | http://inyon.abhashchakraborty.tech | http://inyon.abhashchakraborty.tech/* | "$GITHUB"/*) ;;
  *) if [ -n "${INYON_DOWNLOAD:-}${TETHER_DOWNLOAD:-}" ] && [ -n "${INYON_SIGNING_KEY:-}" ]; then KEY="$INYON_SIGNING_KEY"; fi ;;
esac
ROLE=setup
# --no-signature-check (or INYON_NO_SIGNATURE_CHECK=1) goes on without the signature when nothing
# here can check it; a list with a bad signature is refused all the same
UNCHECKED="${INYON_NO_SIGNATURE_CHECK:-}"
for a in "$@"; do
  case "$a" in
    --hub) ROLE=hub ;;
    --no-signature-check) UNCHECKED=1 ;;
    *) echo "unknown option $a (the options are --hub and --no-signature-check)" >&2; exit 2 ;;
  esac
done

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) echo "This is for macOS and Linux. On Windows, run this in PowerShell instead:" >&2
     # the short address unless downloads come from somewhere else
     [ -n "${INYON_DOWNLOAD:-}${TETHER_DOWNLOAD:-}" ] && ps1="$BASE/get.ps1" || ps1="$SITE/get.ps1"
     echo "  irm $ps1 | iex" >&2
     exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *) echo "There's no inyon build for $(uname -m) yet." >&2; exit 1 ;;
esac
# the command is static and the same everywhere; releases name their engine for musl systems apart
# (Alpine and others), which tells a release from before 0.13 below
libc=""
if [ -f /etc/alpine-release ] || ldd --version 2>&1 | grep -qi musl; then libc=-musl; fi

name="inyon-$os-$arch"
engine="inyon-engine-$os-$arch$libc"
dir="$HOME/.inyon/bin"
mkdir -p "$dir"
tmp="$dir/inyon.download"
list="$dir/SHA256SUMS.download"
sum_of() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }
sum_for() { printf '%s\n' "$sums" | awk -v f="$1" '$2 == f || $2 == "*" f { print $1 }'; }
refuse() {
  rm -f "$list" "$list.minisig"
  echo "$1" >&2
  exit 1
}
# The version this install asks for: the tag a GitHub address names, the one GitHub calls latest,
# or the one latest.json names on the website or a mirror. Empty for a tag that isn't a version
# (a build like node-latest), which asks for that build and nothing older.
asked_version() {
  b="${BASE%/}"
  case "$b" in
    */releases/download/*) v="${b##*/releases/download/}"; case "$v" in v[0-9]*) ;; *) return 0 ;; esac ;;
    */releases/latest/download) v="$(curl -fsS -o /dev/null -w '%{redirect_url}' "${b%/download}")" && v="${v##*/tag/}" || v="" ;;
    *) v="$(curl -fsSL "$b/latest.json" | awk -F'"' '{ for (i = 1; i < NF - 1; i++) if ($i == "version") { print $(i + 2); exit } }')" || v="" ;;
  esac
  v="${v#v}"
  case "$v" in
    [0-9]*.[0-9]*.[0-9]*) case "$v" in *[!0-9A-Za-z.+-]*) v="" ;; esac ;;
    *) v="" ;;
  esac
  [ -n "$v" ] || refuse "Couldn't tell which version $BASE serves, so nothing was installed."
  echo "$v"
}
curl -fsSL "$BASE/SHA256SUMS" -o "$list" || { echo "Couldn't get $BASE/SHA256SUMS. Check the connection and run the command again." >&2; exit 1; }
sums="$(cat "$list")"
redirected="${INYON_DOWNLOAD:-}${TETHER_DOWNLOAD:-}"
if [ -n "$LIST_SHA256" ] && [ "$(sum_of "$list")" = "$LIST_SHA256" ]; then
  # the list the website checked against the release key when it published this copy
  :
elif command -v minisign >/dev/null 2>&1; then
  if ! { curl -fsSL "$BASE/SHA256SUMS.minisig" -o "$list.minisig" && minisign -V -q -m "$list" -x "$list.minisig" -P "$KEY" >/dev/null 2>&1; }; then
    refuse "The release's checksum list isn't signed by Inyon's release key, so nothing was installed."
  fi
  # a good signature isn't enough on its own: an older release's list carries one too. Each
  # release lists its APK as inyon-<version>.apk, so the list has to name the version asked for
  version="$(asked_version)" || exit 1
  if [ -n "$version" ] && [ -z "$(sum_for "inyon-$version.apk")" ]; then
    refuse "The release's signed checksum list isn't the one for Inyon $version (it doesn't list inyon-$version.apk), so nothing was installed. Try again in a few minutes."
  fi
elif [ -n "$UNCHECKED" ]; then
  echo "(the checksum list's signature wasn't checked, as asked)"
elif [ -n "$LIST_SHA256" ] && [ -z "$redirected" ]; then
  refuse "The website's checksum list isn't the one this installer was published with: it's probably changing over to a new release. Run the command again in a few minutes."
else
  refuse "Couldn't check the signature of the release's checksum list, because minisign isn't installed, so nothing was installed. Install minisign (brew install minisign, apt install minisign) and run this again, or add --no-signature-check to trust the list without it:
  curl -fsSL $SITE/get.sh | sh -s -- --no-signature-check"
fi
rm -f "$list" "$list.minisig"
mismatch() {
  rm -f "$tmp" "$tmp.gz"
  echo "The download didn't match the release's checksum. Run the command again." >&2
  exit 1
}
want="$(sum_for "$name")"
want_gz="$(sum_for "$name.gz")"
# the gzipped copy is under half the size; releases before 0.11 have none
if [ -n "$want_gz" ] && command -v gzip >/dev/null 2>&1; then
  echo "Downloading $name.gz…"
  curl -fL --progress-bar "$BASE/$name.gz" -o "$tmp.gz"
  [ "$(sum_of "$tmp.gz")" = "$want_gz" ] || mismatch
  gzip -dc "$tmp.gz" > "$tmp"
  rm -f "$tmp.gz"
else
  # the website keeps only the gzipped copies of a large release; the plain ones stay on GitHub
  # under the release's tag, which the list gives away in its APK's name (inyon-<version>.apk).
  # Either way the file is checked against the same list below
  plain="$BASE/$name"
  case "$BASE" in
    "$SITE"/download | "$SITE"/download/ | "$OLD_SITE"/download | "$OLD_SITE"/download/)
      version="$(printf '%s\n' "$sums" | awk '{ f = $2; sub(/^\*/, "", f) } f ~ /^inyon-[0-9]+\.[0-9]+\.[0-9]+\.apk$/ { sub(/^inyon-/, "", f); sub(/\.apk$/, "", f); print f; exit }')"
      if [ -n "$version" ]; then plain="$GITHUB/download/v$version/$name"; fi ;;
  esac
  echo "Downloading $name…"
  curl -fL --progress-bar "$plain" -o "$tmp"
fi
[ -n "$want" ] && [ "$want" = "$(sum_of "$tmp")" ] || mismatch
chmod +x "$tmp"
mv -f "$tmp" "$dir/inyon"
# Apple Silicon only runs signed programs; a local signature is enough
[ "$os" = darwin ] && command -v codesign >/dev/null 2>&1 && codesign --force --sign - "$dir/inyon" >/dev/null 2>&1 || true

# new terminals find inyon by name: one line marked "added by Inyon" in the startup file of your
# shell, written once (inyon uninstall takes it out again). INYON_NO_MODIFY_PATH=1 leaves it out
on_path=""
case ":$PATH:" in *":$dir:"* | *":$dir/:"*) on_path=yes ;; esac
if [ -z "${INYON_NO_MODIFY_PATH:-}" ] && [ -z "$on_path" ]; then
  mark="# added by Inyon"
  case "${SHELL##*/}" in
    zsh) files="${ZDOTDIR:-$HOME}/.zshrc"; line="export PATH=\"\$HOME/.inyon/bin:\$PATH\" $mark" ;;
    # a Mac's Terminal starts bash as a login shell, which reads .bash_profile instead
    bash) files="$HOME/.bashrc"; [ "$os" = darwin ] && files="$files $HOME/.bash_profile"; line="export PATH=\"\$HOME/.inyon/bin:\$PATH\" $mark" ;;
    fish) files="$HOME/.config/fish/conf.d/inyon.fish"; line="contains \$HOME/.inyon/bin \$PATH; or set -gx PATH \$HOME/.inyon/bin \$PATH $mark" ;;
    *) files="$HOME/.profile"; line="export PATH=\"\$HOME/.inyon/bin:\$PATH\" $mark" ;;
  esac
  changed=""
  for f in $files; do
    grep -qF "$mark" "$f" 2>/dev/null && continue
    mkdir -p "${f%/*}"
    # on a line of its own, even when the file doesn't end with one
    if [ -s "$f" ] && { ! command -v tail >/dev/null 2>&1 || [ -n "$(tail -c 1 "$f")" ]; }; then echo >> "$f"; fi
    printf '%s\n' "$line" >> "$f" || continue
    case "$f" in "$HOME"/*) f="~/${f#"$HOME"/}" ;; esac
    changed="${changed:+$changed and }$f"
  done
  if [ -n "$changed" ]; then
    echo "Added inyon to your PATH in $changed, so new terminals find it by name."
    echo "In this one, first run: export PATH=\"\$HOME/.inyon/bin:\$PATH\""
  fi
fi

# from 0.13 a release lists the inyon command and the engine ($engine), which older versions
# of inyon fetch; this one runs its own. Before 0.13, this one file was the whole program
if [ -n "$(sum_for "$engine")" ]; then
  if [ "$ROLE" = hub ]; then exec "$dir/inyon" setup --role hub; fi
  exec "$dir/inyon" setup
fi
if [ "$ROLE" = hub ]; then exec "$dir/inyon" hub install; fi
exec "$dir/inyon" setup
