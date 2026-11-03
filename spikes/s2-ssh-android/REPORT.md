# S2: russh as the phone's SSH client

Spike r0-s2. Can russh replace `SshCore.kt`? Tested in CI (`.github/workflows/spike-s2-ssh.yml`)
against a throwaway OpenSSH 9.6 on the runner, locally against OpenSSH 10.2, and as an x86_64
Android build on an API 34 emulator.

## Verdict

Go. russh 0.64.1, with the `ring` backend (no cmake or C build to cross-compile), covers every
part of SshCore the phone uses. It builds for aarch64 and x86_64 Android with the stock NDK in 43
seconds, and passes the same tests on the emulator.

## What was tested

21 tests (`cargo test`), all passing on Linux:

- Key loading: OpenSSH ed25519, RSA and ECDSA; ed25519 and RSA with a passphrase; PKCS#8 RSA with
  a passphrase; legacy PEM RSA with and without a passphrase. A wrong or missing passphrase is
  `AUTH_FAILED`. Test keys come from `ssh-keygen` at test time; none are checked in.
- Fingerprints are `SHA256:` plus unpadded base64, equal to `ssh-keygen -l -E sha256` and to
  SshCore's text. `normalize_fingerprint` matches SshCore's.
- SshCore's login order: `none` first, then the phone's ed25519 seed, then a pasted key, then a
  password. A real `none` login runs against an in-process russh server, since OpenSSH can't
  allow it without a passwordless account. The phone-seed login works against OpenSSH.
- Probe returns the host key; a wrong host key gives `HOST_KEY_MISMATCH` with both fingerprints.
- 16 sessions at once on one connection, each running its own command.
- Exec returns the exit code, stdout and stderr, and keeps leading spaces.
- A PTY at 200x50 reports `50 200`, follows a resize to 120x30 and echoes typed input.

On the emulator (Android 14, x86_64): the `exec` example (probe, login, 12 sessions, a command),
the `none` test and 8 of the 9 sshd tests pass. The phone-seed test was skipped there because it
edits the runner's authorized_keys. The key tests need ssh-keygen, so they run only on Linux.

## Numbers

| | arm64 | x86_64 |
|---|---|---|
| `exec` example, release, stripped, LTO, opt-level z | 1,753,480 B (831,580 gzip -9) | 2,057,640 B |
| same runtime without SSH (`baseline`) | 400,128 B | 429,808 B |
| what russh adds | 1.35 MB | 1.63 MB |

- Login with an unencrypted ed25519 key: 37–61 ms.
- Login with an encrypted OpenSSH key: 0.7–1.5 s. That is the bcrypt key derivation in a debug
  build: decrypt once per session and keep the key, don't decrypt on every connect.
- 16 sessions opened in 3–43 ms; all 16 commands done in about 0.3 s.

## Findings for R6

- OpenSSH's default MaxSessions is 10, and the 11th session is refused (`ConnectFailed`). The app
  still needs SshCore's extra connections per server.
- Dropping a russh `Channel` does not close it on the server, so the session slot stays taken.
  Every channel must be closed explicitly, or the pool's count drifts.
- A host certificate is fingerprinted here by the key it certifies. SshCore hashes the whole blob,
  so the two differ for certificates. That's rare for Inyon's hosts, but settle it before
  migrating saved fingerprints.
- Not in the spike: keyboard-interactive login (SshCore answers plain password prompts), the
  keepalive and dead-link timer, ProxyJump/`via`. russh has the pieces for all three.
- The emulator reaches the runner's sshd through `adb reverse`. 10.0.2.2 often isn't reachable
  right after boot.
