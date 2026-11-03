#!/usr/bin/env bash
# Builds the Android app (from 1.0): dx compiles apps/app with the phone core for each ABI, and this
# folder's Gradle project packages the result with the Kotlin the Capacitor app shares. Needs dx at
# the version apps/app/build.sh names, the Rust Android targets, the Android SDK and NDK
# (ANDROID_HOME, ANDROID_NDK_HOME) and Java 21.
#
#   INYON_ABIS        arm64-v8a (default), x86_64, or both separated by a comma
#   INYON_KEYSTORE    signs the release APK (with INYON_KEYSTORE_PASSWORD, INYON_KEY_ALIAS and
#                     INYON_KEY_PASSWORD); without it the APK comes out unsigned
#   INYON_VERSION, INYON_VERSION_CODE   the version name and code
#
# Writes app/build/outputs/apk/release/app-release.apk (app-release-unsigned.apk unsigned).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
app=$(dirname "$here")
want=0.7.10
have=$(dx --version 2>/dev/null | awk '{print $2}' || true)
if [ "$have" != "$want" ]; then
  echo "dx $want is needed, found ${have:-none}: download dx-<target>.tar.gz from https://github.com/DioxusLabs/dioxus/releases/tag/v$want" >&2
  exit 1
fi
target=$(cd "$app" && cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)
dxout="$target/dx/inyon-app/release/android/app/app/src/main"
into="$here/app/build/dx"
rm -rf "$into"
mkdir -p "$into/kotlin/dev/dioxus/main" "$into/jniLibs"

IFS=',' read -r -a abis <<< "${INYON_ABIS:-arm64-v8a}"
for abi in "${abis[@]}"; do
  case "$abi" in
    arm64-v8a) triple=aarch64-linux-android ;;
    x86_64) triple=x86_64-linux-android ;;
    *) echo "unknown ABI $abi" >&2; exit 1 ;;
  esac
  rm -rf "$dxout"
  # --package as in apps/app/build.sh; dx writes its own Gradle project, of which only the library,
  # wry's Kotlin and the assets are used
  (cd "$app" && dx build --platform android --release --package inyon-app --features mobile --target "$triple" --debug-symbols false)
  mkdir -p "$into/jniLibs/$abi"
  cp "$dxout/jniLibs/$abi/"*.so "$into/jniLibs/$abi/"
done
# wry's glue (WryActivity, RustWebView, Ipc, ... and proguard-wry.pro); the app's own MainActivity
# (app/src/main/java/dev/dioxus/main) replaces dx's
cp "$dxout/kotlin/dev/dioxus/main/"* "$into/kotlin/dev/dioxus/main/"
rm -f "$into/kotlin/dev/dioxus/main/MainActivity.kt"
[ -d "$dxout/assets" ] && cp -r "$dxout/assets" "$into/"
ls -lR "$into" | head -40

# the Capacitor project's Gradle wrapper, so both builds use one Gradle
"$app/../mobile/android/gradlew" -p "$here" --no-daemon assembleRelease
ls -l "$here/app/build/outputs/apk/release/"
