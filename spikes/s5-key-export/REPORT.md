# S5: moving the phone's keys and pairings from Capacitor to Dioxus

Spike r0-s5. The Dioxus app (R6) installs over the Capacitor app as an upgrade. Can it keep the
phone's identity, its pairings, its hubs and its server sign-ins, so nobody has to pair again?
Two sketches, neither built: `MigrationExport.kt` (the last Capacitor builds, R5) and `import.rs`
(the Dioxus app's first start, R6).

## Verdict

Go. The last Capacitor builds write everything into one file sealed with a Keystore key, and the
Dioxus app opens it on its first start. Same app id and signing key means the same uid, files
folder and Keystore entries, so nothing leaves the phone.

## Where the data lives today

| What | Where |
|---|---|
| The phone's ed25519 seed (`phone-key`): its SSH key and, as X25519, its Noise static key | `inyon_secrets`, the native encrypted file InyonSshPlugin keeps |
| Server passwords and pasted keys (`server:<hostId>`) | `inyon_secrets` |
| Pairings (laptops over the link, SSH servers) | WebView IndexedDB `inyon` / `kv` / `hosts` |
| Session names, the core's settings | IndexedDB `kv` / `names`, `settings` |
| Hubs and their bearer tokens | localStorage `inyon.hubs.v1` |
| App settings (theme, fonts, haptics) | localStorage `inyon.settings.v2` |

S1 measured the Dioxus page's origin as `https://dioxus.index.html`, while Capacitor's is
`https://localhost`. The new app can't see the old WebView storage, so the data has to be exported.

## The export

R5 (last Capacitor builds):
- The web app calls `exportForUpgrade` with what lives in WebView storage on every start and every
  time it goes to the background, so the file is never older than the last time the app was open.
  Calls run one at a time, in order, so an earlier export never overwrites a later one.
- Kotlin adds `secrets` straight from `inyon_secrets`, so they never cross the JS bridge.
- The file is `files/inyon-migrate.bin`: `"INYM" | format (1) | IV length (12) | IV | ciphertext + tag`,
  AES-256-GCM with the 18-byte header as associated data. The key (`inyon-migrate-v1`) never
  leaves the Keystore; no user authentication or unlocked-device requirement, since the export
  runs as the app goes to the background, often with the screen off.

R6 (Dioxus first start):
- Rust asks Kotlin over JNI for `read` (plaintext or nothing; never throws), parses it, and
  commits everything to its own store in one transaction.
- Only then does it call `discard`, which deletes the file and the key. A crash between the two
  leaves the file; the next start sees the import already done and only deletes it.
- The identity is all or nothing: a seed that doesn't match its public key is a corrupt export and
  is refused, since importing it would leave the phone unknown to every laptop and server.
- Hosts that can't be used as they are (an unknown kind, a malformed machine id) are left out with
  a reason, logged and shown as a one-line notice.
- A file this build can't read (an unknown format, a missing key) is kept, so a later build may.

## What 0.23 shipped

The file and the seal are as above (`MigrationSeal.kt`, `MigrationExport.kt` and
`InyonMigratePlugin.kt` under `apps/mobile/android/app/src/main/java/.../migrate/`). The payload
differs from `import.rs`, because hub tokens moved to the secret store in 0.20.2 and the core's
records are written by the Rust core now. The web app (`apps/web/src/lib/upgradeExport.ts`) sends
everything rather than picking fields:

- `v` (1), `exportedAt`, `appVersion`
- `records`: every record in IndexedDB `inyon` / `kv` (key to JSON text, as the store keeps it),
  apart from the chat caches under `chats/`
- `local`: every `inyon.*` localStorage key, apart from the first-frame snapshots
  (`inyon.overview.`, `inyon.chat.`, `inyon.chats.`)

Kotlin adds `secrets`: everything in `inyon_secrets` (`phone-key`, `hub-token:<id>`,
`server:<hostId>`). The R6 import reads these maps back into its own store.

## Why not read the old storage directly

The WebView's IndexedDB is a leveldb in the app's data folder, in Chromium's own format, which
changes between WebView versions. Reading it from Rust would mean parsing an undocumented format on
every phone's WebView version. The export goes through the web app's own code, which already reads
it correctly.

## Users who skip the export build

Someone who updates straight from an older Capacitor build to the Dioxus app has no file. The app
then starts fresh: a new identity, and the pairing screen. Their laptops still list the old phone
until `inyon forget` removes it. R6 should say this in one line on the pairing screen, and the R5
release should stay up long enough that most phones open it once.

## Risks

- Device-to-device transfer and backups can copy `inyon-migrate.bin` to a phone without the
  Keystore key. `read` then returns nothing (a failed tag), which is safe, but add
  `data_extraction_rules.xml` so the file isn't copied at all.
- The Dioxus APK must use the same signing key and a higher versionCode (S1: our own Gradle
  project, not dx's template, which hardcodes `versionCode = 1`).
- R8 removes JNI entry points it can't see being called: keep `MigrationExport` and the plugin
  class in `proguard-rules.pro`.
- Keystore bugs on some vendors' phones: `read` treats any failure as "nothing to import" and
  keeps the file.

## What R5 and R6 must test

- Export, upgrade, import on a real phone: the same laptops connect without pairing, hubs sign in,
  SSH servers open with their saved fingerprints.
- A tampered file, a changed header, a missing key and an unknown format all import nothing and
  leave the phone working.
- A crash between commit and discard: the next start deletes the file and keeps the import.
- Upgrade straight from an older build: a clean start with the notice.
