---
title: Troubleshooting
description: What to check when a computer won't pair, a hub won't answer, or a session doesn't update.
---

Start with `inyon doctor` on the computer. It checks the engine, tmux, the agents, the service, the ports, the networks and the last update, one line each; a line starting with `!` is the problem. `inyon logs` shows what the service has been saying.

## Can't connect?

From 0.24 the app checks a connection for you. Tap **Reconnecting** at the top of the overview on a hub's page, the **Can't connect?** link under a chat that's reconnecting, **Can't connect?** on a computer or machine that isn't connected, or the one next to a hub in **Settings → Hubs** while it's out of reach. The checks run in order, each with one sentence saying what it found and what to do:

| Check | What it looks at |
|---|---|
| Network | Whether this device is online |
| Address | Whether the hub's name points anywhere (the Android app looks it up; a browser can't, so it skips this) and whether the app will use plain `http://` with it |
| Hub answers | Whether anything answers at the hub's address in 8 seconds |
| Right address | Whether the hub answers to that name, or refuses it with *This hub doesn't answer to that address* (add the name to `INYON_ALLOWED_HOSTS`) |
| Signed in | On a hub's page, whether you're still signed in; in the app, whether the hub still knows this phone |
| Live connection | Whether the hub's WebSocket opens (a proxy has to pass WebSockets through) |
| Its address, Relay, Handshake | For a computer paired with the phone: whether it has published where it is, whether its relay answers, and whether the encrypted handshake works |
| Hub reaches it | For a machine on a hub: whether the hub is connected to it, and what the hub says if not |
| Battery limits | On Android: **Stay connected**, battery limits, a restricted app and battery saver |

When a step fails, the ones that depend on it are skipped. Some steps offer the fix as a button: **Sign in**, **Pair again**, **Allow** or **Turn on**. **Details** shows the raw answers, and **Check again** runs everything once more.

**Copy report** puts a plain-text report on the clipboard for whoever is helping you: the app's version and platform, the checks, Android's battery state, and the last ten times a hub or computer dropped, with when, why and for how long. Tokens, tickets, cookies, pairing codes and links, passwords, keys and anything after `?` or `#` in an address are replaced with `[hidden]`. Addresses themselves stay, so leave out any you don't want public.

## inyon: command not found

The installers put it in `~/.inyon/bin`. From 0.17.1 they add that to your `PATH` too, but only new terminals see it: open one, or run `~/.inyon/bin/inyon`. With an older installer or `INYON_NO_MODIFY_PATH=1`, [add it to your PATH](install.md#put-inyon-on-your-path).

## Something stopped working after updating to 0.23

0.23 removed the Bun engine and the Bun hub, so `INYON_LEGACY_ENGINE=1` and `INYON_HUB_IMPL=bun` no longer go back to them: `inyon` runs the link, the hub connection and the hub itself, and `inyon doctor` says when a switch is still set. If something worked on 0.22 and doesn't now, please [open an issue](https://github.com/Abhash-Chakraborty/inyon/issues) with what broke, and [pin 0.22.5](install.md#installing-from-somewhere-else) until it's fixed. [The Bun engine, removed in 0.23](install.md#the-bun-engine-removed-in-0-23) has the details.

## The phone reached the computer once, then couldn't

Before 0.17.2, `inyon pair` on Windows could miss the link already running in the background and start a second one. The second one published its own address, and when it ended with the terminal, the phone was left pointing at it. Update both sides; from 0.17.2 `pair` sees a running link by its port too. Until then, `inyon service install link` restarts the background link, which publishes the right address again.

## A console window opens, or the icon is missing (Windows)

Up to 0.20.2 the Inyon window's **Change**, **Show all** and **Details** each opened a terminal, **Uninstall** in Settings → Apps opened a console that waited for Enter, and the tray icon belonged to the engine, so it could be missing while the link ran. From 0.20.3 Inyon is an app: nothing opens a terminal, and its icon shows exactly while something runs. `inyon update` moves a computer to it; run `inyon status` afterwards to see what the app runs.

## The Inyon window doesn't open, or is blank (Windows)

From 0.21 the window is drawn by Microsoft Edge WebView2. When it's missing or won't start, Inyon says so and opens Microsoft's download page; install the Evergreen runtime and open Inyon again. The phone link and the hub connection keep running meanwhile. `%USERPROFILE%\.inyon\node\window.log` says what went wrong. In 0.20.1 to 0.20.5, after the window was closed once, clicking the icon didn't open it again until you chose **Quit Inyon**; update to 0.21. Up to 0.22.3, when WebView2 took more than 10 seconds to start (the first start after signing in, on a slow disk), Inyon closed before its window appeared and `app.log` said "nothing runs and no window is open: ending"; from 0.22.4 it waits for the window.

## The window keeps saying "Starting the tunnel" (Windows)

In 0.20.1 to 0.20.5 the window asked for a new pairing code every time it opened, and while Tunnel was on the code card said "Starting the tunnel…" until the tunnel's address came, which looked as if a change to the switches hadn't applied. From 0.21 the window makes a code only when you choose **Pair a phone**, and the switches on This computer show what the link uses. `link.log` shows when the tunnel starts and stops.

## The tray menu opens an empty window (Windows)

In 0.17.1, **Open Inyon** and **Pair a phone…** in the tray menu opened a window that closed at once. Run `inyon` or `inyon pair` in a terminal instead, or update to 0.17.2.

## Inyon isn't in Settings → Apps (Windows)

Setup and `inyon update` list it there from 0.17.2. An install from 0.17.1 appears after its next `inyon update` or `inyon setup`. `inyon uninstall --purge` works either way.

## The phone won't pair with a computer

- **The code expired.** Codes last 10 minutes and work once. Run `inyon pair` for a fresh one.
- **Five wrong typed codes** cancel the typed code. Scan the QR code instead, or run `inyon pair` again.
- **The link isn't running.** `inyon service status` should show `link` running. If not, `inyon service install link`.
- **"The node's files are busy"** from `inyon forget`, `inyon pair` or setup: another `inyon` process has held `~/.inyon/node/files.lock` for over 15 seconds. From 0.20 nothing writes the pairings without that lock, so the command stops instead of risking a forgotten phone coming back. Try again; if it stays busy, end the process whose pid is in that file.
- **"Busy saving its files".** The computer was in the middle of writing its pairings, from `inyon pair` or `inyon forget` in another window, and the phone doesn't wait more than a second for that. Try again. If it keeps saying so, a stuck `inyon` process is holding the lock on `~/.inyon/node/files.lock`: `inyon service install link` restarts the link, and ending the stuck process frees it. From 0.20 the lock goes away by itself the moment the process holding it ends, however it ended.
- **The tunnel can't start.** Some networks block Cloudflare's tunnel. `inyon doctor` shows whether `cloudflared` is there; `inyon logs` shows its errors. On a VPN, the phone can connect on the VPN's address instead.
- **The code had no address (0.19.0).** `inyon pair` could show a code before the tunnel was up, most often the first time, while it downloaded `cloudflared` (a download that could hang for minutes on an unreachable GitHub address). The phone then said it couldn't find the computer online. From 0.20 downloads give up on a dead address after 10 seconds, or after 30 seconds without data, and try the next, setup fetches `cloudflared` ahead of time, and `inyon pair` waits for the tunnel. On 0.19.0, wait a minute and run `inyon pair` again.

## The phone can't reach a paired computer

The computer shows as connecting and never arrives.

- Check the computer is on and the service is running (`inyon service status`).
- Quick tunnels have no uptime promise. If Cloudflare is having a bad moment, a private network (Tailscale, NetBird, ZeroTier, WireGuard) is the steadier route; turn it on in `inyon setup`.
- On a VPN, make sure the phone is connected to it too.
- On Windows, the firewall blocks the LAN and the VPN unless the rule "Inyon link" is there, and both are off by default, so the computer is reachable only through the tunnel. From 0.20 `inyon doctor` says so on its `reach` line. Run `inyon setup` again with *LAN* or *VPN* ticked and allow it when Windows asks, or see [The firewall on Windows](cli.md#the-firewall-on-windows).
- On Windows in 0.19, **Quit Inyon** left the tunnel's `cloudflared` running, and `inyon status` still listed the tunnel's address after the link had stopped. From 0.20 Quit stops the tunnel too, and status lists addresses only while the link runs. On 0.19, **Restart** from the tray menu starts the link and a fresh tunnel.

## "Couldn't pair with the hub"

The app explains what it found:

- **It timed out.** The phone can't reach the hub's address. Is it on the hub's network (the VPN or the Wi-Fi)?
- **Failed to fetch** on a plain `http://` address: update the app to 0.13.3 or later, which allows plain addresses.
- Check that the address opens in the phone's browser. If it doesn't, the app can't reach it either.
- The code expired, was used, or was mistyped five times. Show a new one with **Open on your phone**.

## The hub won't open

- **From another device:** the hub listens only on its own machine until you choose a network. See [Reaching it from your phone](hub.md#reaching-it-from-your-phone).
- **"Refusing to serve plain http"** in the log, with a network `BIND`: set `TLS_CERT` and `TLS_KEY`, or `INYON_ALLOW_HTTP=1` on a VPN. For the LAN, run `inyon setup` and choose *LAN*, which makes a certificate.
- **"Refusing to send the token over plain http"** on a machine: from 0.20 plain http only goes to a Tailscale or NetBird address. A machine added before 0.20 still connects and warns with **"Sending the token over plain http"** instead. See [Plain http](machines.md#plain-http).
- **"doesn't hand out Inyon's signed list of checksums"** from `inyon update` on a machine: update the hub first. See [Security](security.md#downloads).
- **"couldn't hand out Inyon's signed list of checksums"** from `inyon update` on a machine: the hub couldn't fetch its release's list from GitHub, or the list it got isn't the one for its version. The hub's log says which. Try again later; the machine changes nothing meanwhile.
- **"This hub doesn't answer to that address"** under a domain name: add it to `INYON_ALLOWED_HOSTS`. Behind a proxy on another hostname, add the origin to `INYON_ALLOWED_ORIGINS`.
- **A port-forwarding rule** (say, port 80 to 4777) only applies to traffic arriving from other machines. Opening the name from the hub's own machine reaches whatever else listens on port 80 there.
- **"Too many attempts. Try again in a minute."** when you sign in: many wrong passwords reached the hub. From 0.20.3, the hub's own machine and an address that signed in within the last 30 days carry on under their own limit; a new address gets one try every few seconds until it calms down, and the hub's log says it happened.
- **Forgot the password:** `inyon hub reset-password`. **Lost the setup code:** `inyon hub code`. From 0.20.3 the hub's log names the file the code is in rather than the code, unless the hub runs in a terminal.

## A machine shows as offline

- A joined machine: is it on, and is `inyon service status` showing `node` running? `inyon logs --role node` shows why it can't connect.
- The log says the hub's certificate isn't trusted, and the hub's certificate comes from your organisation's own authority: put that authority's certificate in a PEM file and set `NODE_EXTRA_CA_CERTS` to its path, for the service too (see [Computer settings](configuration.md#computer-settings)).
- The hub removed it, or its pairing command expired unused (after 24 hours): use **New pairing command** on its page.
- **"The hub refused this token"** from `inyon join`: from 0.20.3 a pairing command works once, since `join` trades its token for a new one. Get a new command from **New pairing command**. Running the same command again on a machine it already added keeps that machine's token.
- **"This hub hasn't fetched Inyon for … yet"** while installing: from 0.20.3 the hub fetches Inyon for a new system only while an **Add a machine** command is waiting. Open **Add a machine** on the hub (or **New pairing command**), then run the command again.
- An SSH server: check you can `ssh` to it from the hub's machine as the hub's user, without a password prompt.

## A session doesn't appear

- Inyon shows agents running in **tmux**. An agent started outside tmux isn't visible.
- The machine's **Setup** tab shows whether tmux and the agent are found.
- Give it a few seconds; machines are checked every few seconds.

## Messages stay on "Sending"

- The machine may be offline or reconnecting; the chat says so. Messages that failed because the connection dropped are sent again when it's back.
- The agent may be showing a prompt. While a question is open, answer it first.
- Open the terminal view to see what the agent is doing.

## An attachment won't upload

From 0.24 an attachment that says *Reconnecting…* is waiting for the connection to come back and carries on by itself. If it fails, tap it to try again: it picks up where it stopped, unless the hub restarted or an hour went by, when it starts over.

- **"… is larger than 100 MB"**: raise **Largest file** in Settings → [Uploads and clipboard](settings.md#uploads-and-clipboard), or send a smaller file.
- **"The file arrived damaged"**: the bytes that reached the machine didn't match the file's SHA-256. Nothing was kept. Send it again.
- **"Too many uploads at once"**: the hub takes 32 at a time. Wait for some to finish.
- **"Could not save …"** or a message from the machine's shell: the machine's disk may be full, or `~/.inyon/uploads` isn't writable.

## Live numbers stopped updating

For Claude Code, the numbers come from the integration. Turn it on, or **Update** it, from the machine's **Integration** tab. A session that stops reporting offers **Reconnect** or **Restart**; Settings → **Restart sessions that stop reporting** does it by itself.

## Notifications don't arrive

- **Android:** keep **Stay connected** on and allow the app to run without battery limits. A fully closed app gets no alerts yet. If alerts stop about a minute after you leave the app, update it to 0.23.1 or later.
- **Browser:** the hub must be on HTTPS, and on iPhone the page must be on the Home Screen. Use **Send a test** in Settings.

## Updates

- `inyon update` says nothing changed but you expected a new version: it compares against the latest release; check the [changelog](changelog.md).
- **"The release's signed checksum list isn't the one for Inyon …"**: the site was between two releases. Nothing was changed; try again in a few minutes.
- **"The website's checksum list isn't the one this installer was published with"**: the site was changing over to a new release while you ran `get.sh` or `get.ps1`. Nothing was installed; run the command again in a few minutes.
- **"Couldn't check the signature of the release's checksum list, because minisign isn't installed"**: the installer was pointed somewhere other than this site's copy of the newest release. Install minisign, or see [Without minisign](install.md#without-minisign).
- A machine joined to a hub updates to the hub's version, so update the hub first.
- A hub run from source on Bun stops working after `git pull`: 0.23 removed the Bun hub. Build `inyon` in the checkout and run the hub with it, as [From source](hub.md#from-source) shows.
- `cat ~/.inyon/node/update-status` shows the last background update's result, and `~/.inyon/node/update.log` its log.

## Still stuck

Open an issue on [GitHub](https://github.com/Abhash-Chakraborty/inyon/issues) with the output of `inyon doctor --json` and **Settings → About → Copy version details**, or, for a connection problem, the report from [Can't connect?](#cant-connect). Leave out tokens, codes and addresses you don't want public.
