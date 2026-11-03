# Security

Inyon gives whoever signs in to the hub a shell on every connected machine, as the user the
agents run as. Treat the hub like an SSH jump host. The full account of how each connection is
protected is in [Security](https://inyon.tech/docs/security/) on the website; this page is the
summary.

## How it is meant to be deployed

- **Keep the hub private.** It binds to `127.0.0.1` by default. Reach it over a private network
  (Tailscale, NetBird, ZeroTier, WireGuard) or your LAN, where from 0.20 setup gives it HTTPS on a
  certificate of its own, or put it behind HTTPS with a reverse proxy. Don't expose plain HTTP to
  the internet.
- **One strong password.** The first visit creates it, with the setup code the hub prints. The hub
  stores only a scrypt hash. Sign-in attempts are rate limited per address and in total. Sessions
  are signed, HttpOnly cookies, and the API only accepts changes from the hub's own origin.
- **Only its own names.** The hub doesn't answer browsers that reach it under a public domain it
  wasn't told about (`INYON_ALLOWED_HOSTS`), so a web page can't point its own domain at your
  hub (DNS rebinding) and use it from your browser.
- **Laptops connect out, not in.** Machines running `inyon` open an outbound connection
  to the hub. Nothing on them listens for connections, so you never open SSH on a laptop. Each
  machine gets its own random token, the hub stores only its SHA-256, and removing the machine
  revokes it. A token no machine has used stops working after a day. `inyon` refuses to send its token over plain HTTP unless the hub is on
  the same machine or on a Tailscale or NetBird address (from 0.20). A hub on a certificate it made
  itself is pinned by the SHA-256 in its **Add a machine** command on Linux and macOS. The
  Windows command and the Android app can't check that certificate yet, so they reach such a hub
  through a VPN address or a certificate the device trusts.
- **Downloads are signed.** Releases, the one-line installers and the machine updates a hub hands
  out are checked against Inyon's signed release list for the version asked for, and an older
  signed list is refused.
- **Typed text stays off command lines.** Messages, drafts and pasted API keys reach tmux through
  a private file, so other users on the machine can't read them in the process list.
- **Companion apps and Pages get less than you do.** A companion app such as Legato pairs with a
  one-time code and sees only its four permissions; removing it cuts it off at once. A page an agent
  made runs sandboxed under a link that lasts 12 hours and can't see your sign-in or call the hub.
- **Servers can use SSH.** The hub uses your existing `ssh` config, keys and known hosts. It never
  stores SSH keys.

## The Android app

The app is its own hub, so whoever holds the unlocked phone can reach your servers, the same as
with a terminal app that has your keys.

- **Host keys are pinned.** The app shows a server's fingerprint before the first sign-in and
  stores the one you trust. A different key later stops the connection until you confirm it.
- **Secrets stay on the phone.** The app's ed25519 key, passwords and pasted keys live in
  EncryptedSharedPreferences (Android Keystore). Android backups are off for the app.
- **Nothing in between.** The app connects straight to each server over SSH. No relay or hub sees
  the traffic.

## Laptops paired with `inyon pair`

A paired phone gets a shell on the laptop as the user running `inyon`, the same as SSH.

- **The pairing code is the secret.** It holds a one-time code that works for 10 minutes and for
  one phone. Don't share a screenshot of it. `inyon forget` revokes a phone.
- **End-to-end encrypted.** Phone and laptop run Noise IK (X25519, ChaCha20-Poly1305, SHA-256).
  The phone already knows the laptop's key from the code, so a tunnel, relay or network in between
  can't read or change the traffic, or stand in for the laptop. The laptop only lets in phone keys
  it has paired.
- **The tunnel is public, the listener isn't open.** The Cloudflare quick tunnel gives the laptop
  a public address, but it only reaches the link's listener on `127.0.0.1`, which answers nothing
  until the handshake proves a paired phone. The link also listens on the laptop's
  private-network addresses (Tailscale, NetBird, ZeroTier, WireGuard; not on Windows) with the
  same handshake, and `--no-vpn` turns that off. `--lan` also listens on the local network.
- **Notifications for a closed browser.** A paired browser that turns on notifications gives the
  laptop its push subscription and the key that signs pushes to it. They're kept in
  `~/.inyon/node/push.json`, readable only by you. Each message is encrypted to that browser,
  so the push service (Google, Mozilla or Apple) sees only ciphertext. `inyon forget` drops
  that phone's subscriptions.
- **Addresses are signed.** The laptop publishes where it can be reached as a pkarr record signed
  by its own key. The record names addresses only; it says nothing about you or your sessions.
- **Keys on the laptop** live in `~/.inyon/node/` (`identity.json`, `phones.json`), readable only
  by you. The phone derives its link key from its own ed25519 key in the Android Keystore.

## What Inyon changes on a machine

Only if you turn on the Claude integration: it writes `~/.inyon/hook.sh` and
`~/.inyon/statusline.sh`, registers them in `~/.claude/settings.json` (backed up first), and
keeps your original status line in `~/.inyon/statusline.orig`. Turning it off restores both.
If Antigravity CLI is installed, it does the same for agy: `~/.inyon/agy-statusline.sh` goes into
`~/.gemini/antigravity-cli/settings.json` (backed up first), your own agy status line is kept in
`~/.inyon/agy-statusline.orig`, and turning the integration off restores it.
If the integration disappears from `settings.json` (say, a sync script replaced the file), Inyon
puts it back, at most once every ten minutes. Turning the integration off stops that.
A session that was already running when its settings were replaced keeps ignoring
`settings.json`, so Inyon also adds its status line to that project's
`.claude/settings.local.json`. It does this only when the project has no status line of its own,
and it adds the file to `.git/info/exclude` so git never sees it. Inyon keeps a list of these
files in `~/.inyon/project-statuslines` and removes the line from each one when you turn the
integration off.
Uploads go to `~/.inyon/uploads` and are deleted after a retention period you choose.
The hook log holds your prompts, so `~/.inyon` and what the scripts write there are readable only
by you.

## What Inyon reads

For Claude plan limits, the hub runs a short script on the machine that reads Claude Code's
access token from `~/.claude/.credentials.json` (or the macOS Keychain) and asks
`api.anthropic.com` for the account's usage, as `/usage` does. The token is passed to `curl` on
stdin, so it never appears in the process list, and it never leaves that machine. Only the
percentages and reset times come back to the hub.

## Reporting a vulnerability

Please report it privately through
[GitHub's vulnerability reporting](https://github.com/Abhash-Chakraborty/inyon/security/advisories/new)
instead of opening a public issue, with steps to reproduce. You will get a reply within a few days.
