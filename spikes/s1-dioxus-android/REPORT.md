# S1: Dioxus 0.7 as Inyon's Android app

Spike r0-s1. Built and run in CI only (`.github/workflows/spike-s1-dioxus.yml`): dx 0.7.10, NDK 30,
an API 34 x86_64 emulator with software rendering (swiftshader, WebView 113). App id
`tech.abhashchakraborty.inyon.spike`.

## Verdict

Go. A Dioxus app with Inyon's tokens, xterm.js at the 200x50 resting grid, a composer on the soft
keyboard and the keep-alive service started from Rust all work on Android. Package it with our own
Gradle project, with dx only compiling the Rust (see "dx or our own Gradle").

## What was built

- A sidebar of machines and sessions, a chat view and a Chat/Terminal switch, styled with the
  tokens from `apps/web/src/styles.css`. Geist and Geist Mono are embedded, light and dark.
- xterm.js 6.0.0 with the WebGL addon 0.19.0 (the versions apps/web uses, checked against
  bun.lock). It opens at 200x50 and prints 5000 coloured lines. Its typed input goes to Rust.
- A composer: a textarea read by Rust, and a Send button.
- `KeepAliveService`, cut down from apps/mobile, started from Rust over JNI. The service class is
  Kotlin; Rust builds the Intent and calls `startForegroundService`. The app's own classes come
  through the activity's class loader, since `FindClass` on a Rust thread only sees system classes.
- Timings and results go to logcat (tag `InyonSpike`). `ci/emulator.sh` reads them, drives the
  page over DevTools and taps through adb.

## Numbers

APK, arm64, release, R8, stripped, signed:

| build | bytes |
|---|---|
| our own Gradle project (shrinkResources) | 2,821,982 |
| dx's generated project | 3,520,934 |
| libmain.so, uncompressed / in the APK | 3,808,512 / 1,267,910 |
| classes.dex in the APK (appcompat, material, wry glue) | 615,296 |

About 0.75 MB of libmain.so is xterm.js, the WebGL addon and the fonts, embedded as strings. The
dex and resources come from dx's template dependencies (appcompat, material) and can shrink
further. Today's Capacitor APK is about 2 MB.

Emulator, four starts in a row (the first is right after install). Times count from process start,
read over JNI from `Process.getStartUptimeMillis`:

| run | am start TotalTime | Rust main | first render | terminal ready (5000 lines) |
|---|---|---|---|---|
| 1 | 3564 ms | never reported in 90 s | | |
| 2 | 1195 ms | 451 ms | 1746 ms | 4628 ms |
| 3 | 866 ms | 119 ms | 997 ms | 4417 ms |
| 4 | 867 ms | 90 ms | 1019 ms | 3175 ms |

Across six CI runs, warm starts took 0.6–1.4 s and the first render 1.0–1.8 s. Writing 5000 lines
took 0.66–5 s, depending on the runner. Memory was 63–83 MB PSS. These are slow software-rendered
emulators with 2 cores; a phone will be faster. In one of the six runs the first start after
install never reported in.

CI build: dx 230 s cold and 56 s with a warm cache, then 62–82 s for the Gradle packaging.

## What worked and what didn't

- Soft keyboard: a real tap on the composer opens Gboard. `adjustResize` keeps the composer above
  the keyboard, and the typed text reaches Rust (screenshot 04-keyboard).
- A controlled textarea loses keys. With `value` bound to a signal, "hello from the keyboard"
  arrived as "hm the keyrd": Rust wrote stale values back while keys were still arriving. Leaving
  the value to the page (Rust reads it on input and clears it after Send) fixed it. The R6
  composer must work the same way.
- Terminal input: a tap on the terminal focuses xterm, and `ls -la` and Enter reach Rust key by key.
- Keep-alive: the service runs in the foreground (specialUse on API 34) and the notification shows
  (06-notification).
- WebGL: the addon loads with no context loss, but on this emulator the glyphs come out garbled
  (02-terminal). The same screen on xterm's DOM renderer is right (02b-terminal-dom). It looks
  like the software-WebGL problem `apps/web/src/components/terminal/webgl.ts` already falls back
  from. Check on a real phone before relying on WebGL, and keep that fallback.
- The page's origin is `https://dioxus.index.html`, so it can't see the Capacitor app's storage at
  `https://localhost`. That is why S5 exports the data explicitly.

## dx or our own Gradle

Both were built in CI from the same Rust.

- dx alone works. Dioxus.toml can supply our own AndroidManifest.xml and MainActivity.kt, and the
  manifest can declare the service. But dx's generated `build.gradle.kts` hardcodes
  `versionCode = 1` and resolves the keystore through a fixed relative path. The Capacitor app's
  versionCode is the CI run number, and Android refuses an upgrade to a lower one, so a dx-built
  APK can't install over today's app.
- Our own Gradle project (`gradle-app/`, packaged by `ci/own-gradle.sh`) takes dx's `libmain.so`
  and wry's generated Kotlin, and adds our manifest, activity and service. It owns versionCode,
  signing (from env, like apps/mobile), R8 and shrinkResources, and it came out 0.7 MB smaller.
  On the emulator it installed over the dx build as an upgrade and ran.

Use dx to compile, our Gradle to package. Pin dx to the exact Dioxus version: the wry Kotlin glue
and the `dev.dioxus.main` package are tied to it.

## Risks and open items

- WebGL glyphs need a real-phone check (above).
- First start after install was slow once; measure on a phone.
- The spike targets SDK 35. On Android 15 the app draws under the status and navigation bars,
  which the emulator (API 34) didn't show. R6 needs insets.
- Typing was tested with `adb input text`. Real Gboard composition (swipe, autocorrect) needs a
  person on a phone.
- Embedding xterm.js and the fonts as strings costs about 0.75 MB of the library. Serving them as
  APK assets would compress them better.
