---
title: The Android app
nav: Android app
description: Install the app, pair computers and hubs, reach servers over SSH, and keep it connected.
---

The Android app runs Inyon's core on the phone itself. It pairs with your computers and reaches them directly, so there's no server in between and no password. It can also pair with a hub, and reach servers over SSH with nothing installed on them.

## Install

Download the APK from the [download page](/download/) and open it. Android asks once to allow installs from your browser. The app needs Android 7.0 or later, and from 1.0 Android 9 or later (see [The app from 1.0](#the-app-from-10)).

Because it doesn't come from the Play Store, Google Play Protect may say *App scan recommended*. Scanning it is fine and takes a few seconds. To check the file yourself first, see [Downloads](security.md#downloads).

The app checks for a new release once a day (and whenever you open Settings). When one is out, it tells you once, and **Settings → Updates → Download** fetches the new APK, which Android installs over the old one. Your pairings stay.

## Add a computer

Tap **Add a computer** (or **+** on the Computers tab). There are four ways in, side by side:

### Scan its code

Run the installer, or `inyon pair` if it's already installed, on the computer. Tap **Scan the code** and point the camera at the QR code. The app asks *Pair with &lt;name&gt;?* and shows the computer's ID, which `inyon pair` prints under the code from 0.20.2; tap **Pair** if they match. Nothing pairs until you tap, and a pasted link gets the same question. The camera is on only while the scanner is open, and nothing is recorded.

If you scan with the phone's own camera app instead, the code opens this website, which offers to copy the pairing link for the app.

### Type a code

When the terminal is on the same phone, tap **Type a code** and enter the ID (like `K7Q4-MX2T`) and the 6-digit code printed under the QR code. It pairs as soon as both are complete, and from 0.20.3 says *Its ID is …*: if that isn't the ID `inyon pair` printed, remove that computer. Or tap **Paste the link** and paste the link the computer printed.

A code works once, for 10 minutes, and five wrong tries cancel it.

### Connect to a hub

If you run a [hub](hub.md), sign in to its web app and choose **Open on your phone** (on the overview, under Machines, or in Settings). In the app tap **Connect to a hub**, then scan the code, or type the hub's address and the ID and code it shows.

From 0.20.2 a scanned or pasted code shows the hub's address, the name it gives itself, and its **Hub ID** (plus its **Certificate** when the hub serves HTTPS itself), and pairs only when you tap **Pair**. The hub shows the same ID under its code, so check they match: anyone can put any name in a code. A hub that answers with a different ID than its code said isn't kept.

The hub's machines then appear next to the phone's own computers. The phone talks to the hub with a token of its own. A hub that's out of reach shows its machines as connecting until it answers, even when the app opens that way (off your VPN, say): the list stays as it was the last time the hub answered, and before it ever has, the Computers page says it's connecting to the hub. One that forgets the phone drops out with a message. **Settings → Hubs** lists them, each with **Forget**.

The phone has to reach the hub's address: over your VPN or an HTTPS address. From 0.20.2 the app uses plain `http://` only for a hub on this phone, on Tailscale or NetBird (`100.64.0.0/10`, or Tailscale's `fd7a:115c:a1e0::/48`), or at a name that points only there (a NetBird name's own IPv6 address, beside its `100.x` one, counts), such as `http://homebox` when that name is the machine on NetBird. It looks the name up first and says why when it won't. A LAN address like `http://192.168.1.5:4777` is refused, since anyone on that Wi-Fi could read the phone's token; give the hub HTTPS there (setup's *LAN* choice does) or use its VPN address.

The phone forgets a hub only when that hub says it no longer knows the phone. A Wi-Fi sign-in page or a proxy that answers in its place doesn't unpair it.

### Connect over SSH

For a server you can't install on. See [A server over SSH](#a-server-over-ssh) below.

## How the phone reaches a computer

When you pair, the computer and the phone swap long-term keys. After that, nothing about the connection depends on where either one is:

1. **Its address.** The computer opens a free Cloudflare quick tunnel and publishes its current addresses on [pkarr](https://pkarr.org), signed with its own key. The phone looks them up and checks the signature, so a forged address fails.
2. **A private network.** On Tailscale, Headscale, NetBird, ZeroTier or WireGuard, the computer also answers on that network's address, and the phone connects straight to it with no tunnel. Setup says which networks it found.
3. **The local network**, if you turned on LAN.

The phone tries the addresses it knows first, then fresh ones from pkarr. Whichever answers, the two run a Noise IK handshake (X25519, ChaCha20-Poly1305), so a tunnel or network in between only ever carries ciphertext. From 0.20, an address where something other than your computer answers (another device now has that IP) no longer stops the search: the phone tries the rest and keeps trying, and if nothing works the message names the address that answered wrongly. A lost connection is tried again after a random wait that grows to 30 seconds, so a computer that restarts isn't met by every app at the same moment. [Pairing a computer](computers.md) has the details and the options.

## A server over SSH

If you can't install anything on a server, the phone can use SSH, through a native plugin built on ConnectBot's sshlib. The server only needs tmux and your agents.

1. Tap **Add a computer**, then **Connect over SSH**, and give it `user@host` and the port.
2. Choose how to sign in:
   - **Phone key:** an ed25519 key made on the phone. The app gives you one command that adds it to the server's `~/.ssh/authorized_keys`. It's also in **Settings → This phone's key**.
   - **Password.**
   - **Key:** paste a private key.
3. The app shows the server's host key fingerprint. Compare it with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the server, then confirm. If the key ever changes, the app refuses to connect and warns you until you check it again.

Removing the server from the phone (from 0.17) takes the phone key back out of its `~/.ssh/authorized_keys`, along with Inyon's uploads and integration there, if the phone can still reach it.

You can also connect *from a paired computer*: the phone asks that computer to SSH onward, using its own `~/.ssh/config`, keys and aliases.

Passwords and keys stay in Android's encrypted storage, and the app's data is excluded from backups and from moving to a new phone. The phone has to reach the server itself, so use the same Wi-Fi or a VPN. A server on Tailscale SSH lets the phone in with no key at all.

## Pages

The **Pages** tab shows the plans and mockups your agents build, from every hub you paired and, from 0.24, from the computers paired with the phone directly. A computer's pages come over the same encrypted link as its sessions, with no hub needed; it needs `inyon` 0.22 or later. [Pages](pages.md#pages-on-the-phones-own-computers) has the details.

## Staying connected

Android pauses apps in the background, which drops their connections. **Settings → Notifications → Stay connected**, on by default, keeps a small ongoing notification so your sessions stay live and alerts arrive at once. The notification has **Open** and **Stop**.

Nothing is asked before you've added a computer or server. Once the first one appears, the app says why it wants notifications (so it can tell you when a session asks something or finishes), asks for them, then asks once to run without battery limits; **Battery limits** in Settings asks again. If Android stops the service, the app says why when you come back.

From 0.23.1 the app does less while it's in the background. After 15 seconds away it stops following open chats, and a hub sends it the session list only when an agent starts, finishes or asks something, or a machine goes offline or comes back, not with every line an agent writes. Everything catches up when you open the app again. From 0.23.2 its animations, such as the mark reeling while a hub is out of reach, also stop while the app is off screen.

A fully closed app gets no alerts yet. See [Notifications](notifications.md#the-android-app).

### Why a link dropped

From 0.24 the app keeps a log of when each hub and computer connected and dropped, and why: how the connection closed, the network going away, the app being in the background, a hub that forgot the phone, a handshake that failed, or nothing answering. It also asks Android whether the phone was in Doze (the deep sleep Android puts a phone in when it lies still with the screen off), whether Inyon was limited by battery settings, and whether **Stay connected** was on. Android often freezes the app during Doze, so the app only sees the drop when it wakes; a drop it notices within a minute of Doze ending counts as Doze.

**Settings** shows the last drop under each computer and hub, like *Last drop 12m ago, Android's Doze paused the phone (down 4m)*. [Can't connect?](troubleshooting.md#cant-connect) checks the connection and puts the log in its report. The log stays on the phone, keeps the last few drops of each, and never holds tokens or codes.

If drops say Doze or battery limits, keep **Stay connected** on and allow Inyon to run without battery limits.

## Moving to the next app

From 0.23 the app keeps a sealed copy of its key, pairings, hubs and settings each time it starts or goes to the background. The next Inyon app for Android installs over this one and picks that copy up, so your computers, servers and hubs stay paired. Open the app once on 0.23 or later before you install it. [Security](security.md#the-android-app) says how the copy is kept.

The app from 1.0 is that next app. On its first start it reads the copy, keeps your computers, servers, hubs, session names and settings, then deletes the copy and its key. It takes nothing secret from the copy: the phone's key, hub tokens and server sign-ins are already in the secret store both apps share, and it reads them from there. The secrets in the copy are deleted with it, unread. If the app was last opened on a version before 0.23 there's no copy, and the app starts fresh: pair your computers again and remove the old phone from each with `inyon forget`.

## The app from 1.0

From 1.0 the Android app is built from the same Rust code as the rest of Inyon. Its screens are the new app's, the one a hub serves at `/next`, and the phone's core runs as native code inside the app rather than as WebAssembly in a web page. It keeps the app ID and signing key, so it installs over the app you have.

What changes for you:

- It needs Android 9 or later. A phone on Android 7 or 8 keeps the current app.
- The core starts with the app, before any screen opens, and posts alerts itself, so they keep coming while Android has closed the app's window and only **Stay connected** keeps the app running.
- Notifications and **Stay connected** look and behave as before: the same channels, the same ongoing notification with **Open** and **Stop**. Turning **Stay connected** off with **Stop** keeps it off.
- Answering a question from its notification works as before, with the fingerprint or PIN on the lock screen for anything that approves. See [Answering from the notification](notifications.md#answering-from-the-notification).
- Off screen, the app's page stops completely: no timers, no animations, no layout. Only the core keeps running, for your connections and alerts.
- The app's page loads from inside the app and never over plain http. Connections to your computers are the core's own, with the link's end-to-end encryption as before.

## Phone conveniences

- From 0.15, a phone stays upright. A tablet, or a foldable when it's open, turns with the device and gets the wide layout: a sidebar with every session, and on a large tablet in landscape a side panel next to the chat.
- The back button closes the top sheet first, then goes back a page.
- Pasting an image from the keyboard attaches it, as on a computer.
- The keyboard goes down after you send a message, so you can read the reply.
- Keyboard shortcut hints such as `Ctrl K` are hidden on touch screens.
- Long-press a session card for its menu.
- `inyon://pair` and `inyon://hub` links open the app at the right sheet.

## Building it yourself

`apps/mobile` wraps the web app with Capacitor. With Android Studio installed:

```sh
bun install
bun run build
cd apps/mobile
bun x cap sync android
bun x cap open android
```

The app from 1.0 is in `apps/app/android`. With the Android SDK and NDK, Java 21, the Rust Android targets and the Dioxus CLI (`dx`, at the version `apps/app/build.sh` names):

```sh
INYON_ABIS=arm64-v8a apps/app/android/build.sh
```

It writes `apps/app/android/app/build/outputs/apk/release/`, signed when `INYON_KEYSTORE` and its passwords are set. Its version code is 1 unless `INYON_VERSION_CODE` sets one, and Android won't install a lower version code over the app you have, so to install it over a release, set `INYON_VERSION_CODE` to at least the release's (the GitHub build's run number) and sign it with the same key.

Every push to `main` refreshes the *Latest Android build* pre-release on GitHub, and each version tag attaches `inyon-X.Y.Z.apk` to its release. A fork without the signing secrets gets a debug build.

## iPhone

There's no iPhone app yet. On an iPhone or iPad, use a [hub](hub.md) in Safari, or a [browser copy](browser-copy.md). Add either to the Home Screen first: Safari forgets a site's data after 7 days without a visit, and only Home Screen apps get notifications.
