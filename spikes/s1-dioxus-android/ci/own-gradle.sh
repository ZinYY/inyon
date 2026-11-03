#!/usr/bin/env bash
# Packages dx's output with the app's own Gradle project (gradle-app/): copies in the Rust library,
# wry's generated Kotlin and dx's default resources, adds android/ (manifest, activity, service),
# then builds a signed release APK. Usage: own-gradle.sh <abi> <out.apk>; INYON_KEYSTORE etc. set.
set -euo pipefail
abi=$1
out=$2
here=$(cd "$(dirname "$0")/.." && pwd)
dxproj=$(dirname "$(find "$here/target/dx" -name gradlew -path '*android*' | head -1)")
work=$(mktemp -d)
cp -r "$here/gradle-app/." "$work/"
main="$work/app/src/main"
mkdir -p "$main/jniLibs/$abi" "$main/kotlin/dev/dioxus/main"
cp "$dxproj"/app/src/main/jniLibs/"$abi"/*.so "$main/jniLibs/$abi/"
# wry's glue (RustWebView, WryActivity, Ipc, ... and proguard-wry.pro); our activity replaces dx's
cp "$dxproj"/app/src/main/kotlin/dev/dioxus/main/* "$main/kotlin/dev/dioxus/main/"
cp "$here/android/MainActivity.kt" "$main/kotlin/dev/dioxus/main/MainActivity.kt"
cp "$here/android/AndroidManifest.xml" "$main/AndroidManifest.xml"
cp -r "$dxproj/app/src/main/res" "$main/"
[ -d "$dxproj/app/src/main/assets" ] && cp -r "$dxproj/app/src/main/assets" "$main/"
cp -r "$dxproj/gradle" "$dxproj/gradlew" "$work/"
ls -la "$main/jniLibs/$abi" "$main/kotlin/dev/dioxus/main"
(cd "$work" && INYON_ABI=$abi ./gradlew --no-daemon -q assembleRelease)
cp "$work/app/build/outputs/apk/release/app-release.apk" "$out"
