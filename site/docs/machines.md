---
title: Adding machines
description: Add laptops and servers to a hub, set them up from the app, and what happens when one goes offline.
---

A hub reaches its own machine directly. Other machines join it, or it reaches them over SSH. Both start from **Machines → Add machine**.

## Laptops and desktops

Choose **Run a command on it**, name the machine, and tap **Create pairing command**. Pick macOS or Linux, or Windows, and run the one-line command it shows on that machine. The sheet waits with *Waiting for the machine to connect…* and then shows it.

The command downloads `inyon` from the hub, checks it against the hub's checksums, and installs it as a small user service that connects out to the hub. No `sudo`, no open ports on the machine.

- The command carries a token for that one machine. Treat it like a password until it's used. From 0.20.3 `inyon join` trades it for a new token the hub hands out once, before the machine first connects, so the command left in your shell history stops working. An older `inyon` keeps the pasted token.
- An unused command expires after 24 hours. **New pairing command** on the machine's page makes another.
- The machine needs to reach the hub's address. A plain `http://` hub address is only accepted on a VPN; see [Plain http](#plain-http).
- From 0.20, when the hub serves HTTPS on a certificate it made itself (setup's [LAN](hub.md#the-lan) choice), the command carries that certificate's SHA-256 as `INYON_HUB_CERT`. `inyon join` keeps the certificate only if it has that SHA-256, and from then on the connection to that hub's address accepts exactly that certificate. It's a pin, not an authority: a certificate signed with the hub's key isn't accepted, for the hub or any other name, and a certificate the hub gets later from an authority still works.

### Plain http

A machine refuses to send its token over plain `http://` unless the hub is on this machine or on a Tailscale or NetBird address (`100.64.0.0/10`, or `fd7a:115c:a1e0::/48`). A name counts when every address it points to is one of those. NetBird also gives each peer an IPv6 address in a unique-local range (`fd…`) picked at random per network, so from 0.20.2 a name with a NetBird `100.x` address may have such an IPv6 address beside it; a LAN or public address beside it still refuses. It's looked up again before each connection, and the connection goes only to the addresses that are one of those, never to whatever a second lookup says. Before 0.20 any private address was accepted, and names ending in `.local`, `.lan` or `.internal`. Anyone on a LAN can read plain http there, or answer for the hub's name, so from 0.20 `inyon join` and the **Add a machine** command, the Windows one included, stop with:

```
Refusing to send the token over plain http to 192.168.1.5: anyone on that network could read it, or answer in the hub's place. Use the hub's https address, or its Tailscale or NetBird address (100.x.y.z), or set INYON_ALLOW_INSECURE=1 to send it anyway.
```

A machine added that way before 0.20 keeps connecting after it updates, so nothing goes dark, as long as the address is one 0.19 sent the token to: a private address, or a name ending in `.local`, `.lan`, `.internal`, `.ts.net` or with no dot at all. A public address stays refused, however old the machine. It warns in its log, in `inyon status` and `inyon doctor`, and on its page in the hub:

```
Sending the token over plain http to 192.168.1.5:4777, where anyone on that network could read it or answer in the hub's place. This machine was added before Inyon 0.20, so it still connects. …
```

To move it to https, run `inyon setup` on the hub and choose *LAN* again, which makes a certificate, then run a new **Add a machine** command on the machine. Or add it through the hub's VPN address. `INYON_ALLOW_INSECURE=1` when joining keeps the old behaviour, at that risk; `inyon join` notes it in `~/.inyon/node/env`, so the connection keeps it.

The background connection never exits over a setting it can't use, such as no hub saved or an address plain http mustn't go to: a service would only start it again every few seconds. It says why in its log, in `inyon status` and in `inyon doctor`, and tries again every few minutes. Run in a terminal with `inyon connect`, it stops with the reason.

## Servers over SSH

Choose **Reach it over SSH** and give it `user@host` or an alias from the hub's `~/.ssh/config`, plus a port and a key file if needed. The server only needs tmux and your agents.

The hub uses the system `ssh` with your config, keys and jump hosts, and stores no keys of its own. The first connection to a new server accepts its host key and remembers it in `~/.ssh/known_hosts`; a key that changes later is refused. Connections are reused for ten minutes, so opening sessions is quick.

## The machine's page

Tap a machine for its page. **New session**, **Rename** and **Remove** are at the top. Removing a machine never touches it: *Its tmux sessions keep running on the computer.*

The tabs:

- **Sessions:** its tmux sessions and panes.
- **Setup:** what it has and what it's missing (below).
- **Config:** each agent's config files, editable, with JSON checked before saving and the last five versions kept as backups.
- **Uploads:** how much space uploaded files take, with buttons to clean older ones or delete them all.
- **Temp files:** the files its coding agents left behind, to preview and delete (from 0.21, see [Agent temp files](temp-files.md)).
- **Integration:** Inyon's optional Claude Code integration, and which agents are signed in.

### The Setup tab

It checks the machine and fixes what it can:

- **tmux**, and a tmux session to work in (it can keep one called `main` running).
- **Node.js**, needed for the agents installed through npm (Codex, Gemini CLI and GitHub Copilot).
- **Each agent:** installed or not, its version, whether it's signed in, and from 0.17.4 whether it's up to date: the row says *up to date*, or shows **Update to 0.50.0** when a newer version is on npm.

**Install** uses the agent's official installer, or npm when there isn't one. Fixes run with the machine's package manager (apt, dnf, yum, pacman, apk, zypper or Homebrew); when one needs your password, the tab shows the command to run yourself instead.

**Update** runs `npm install -g <package>@latest` when the agent came from npm (or `npm install --prefix <folder>` when it sits in a folder of its own, so the copy on your PATH is the one that changes), otherwise the agent's own updater (`claude update`, `codex update`, `opencode upgrade`, `copilot update`, `agy update`); Gemini CLI has none, so it's installed again from npm. Antigravity CLI isn't on npm, so Inyon can't tell whether it's behind; its row offers a plain **Update** that runs `agy update`. The hub asks npm for the newest versions at most once an hour, and when it can't reach npm the rows just show the installed version.

When an install or update fails, the row opens with the likely cause (npm can't write to its folder, no Node.js, no network), the last lines of what the step printed, the command to run on the machine yourself with a copy button, and a link to the agent's install guide. The version shown comes from the agent itself after the step, so an installer that reports success without installing anything still counts as a failure.

**Sign in** runs the agent's own sign-in in a hidden tmux session and turns it into steps: a code to copy, a page to open, a menu as buttons, or a box to paste a code into. Closing the sheet stops it.

**Providers** (OpenCode, from 0.17.6): OpenCode signs in per model provider, and its free models need none, so its row names the providers connected on the machine (*free models only* when there are none) and the button opens the list. Connected providers come first, each saying how (an API key, a sign-in, an environment variable), with **Disconnect** for a saved credential; the rest are there to connect, with a search box. Connecting runs `opencode auth login` for that provider the same way Sign in does, as steps in the app: a menu, a link and a code, or a box for an API key. **Sign out of OpenCode** disconnects every saved credential at once. A provider set through an environment variable is listed, but only that variable's removal disconnects it. The same list opens from an OpenCode session's model menu, on the phone too.

### The Claude Code integration

Turning it on gives Inyon exact session matching and live numbers (context, cost, limits) for Claude Code. It adds a small hook script and wraps your status line, backing up `~/.claude/settings.json` first, and turning it off puts everything back. From 0.22 the same hook goes into GitHub Copilot CLI, Gemini CLI and Antigravity CLI when they're installed, so their turns show as working or finished the moment they start or end. An agent installed later gets it within ten minutes. [Security](security.md#what-inyon-changes-on-a-machine) lists every file.

When a new version of Inyon changes these scripts, it updates them on the machine by itself, as long as it has seen the integration on there before. Otherwise the button says *Update* instead of *Turn on*. From 0.17.1, an older copy of Inyon (a phone that hasn't updated yet) leaves scripts from a newer one alone.

## From a machine's terminal

From 0.22, on a joined machine, `inyon hub machines`, `inyon sessions` and `inyon pages --hub` show what the hub sees, and `inyon send` gives an agent on another machine a message once it's free, if the hub allows that. See [Sessions and messages](cli.md#sessions-and-messages).

## When a machine goes offline

A machine that runs inyon and drops for up to 15 seconds, as a laptop on Wi-Fi or a VPN does now and then, isn't treated as gone from 0.22.2: its sessions keep their state, and a message sent meanwhile goes in once it's back.

For longer, the hub keeps the last thing it saw. After a minute away, the machine shows as *Offline · last seen 5 min ago*, with its sessions and recent conversations still readable, and you're notified. You can't send to it until it's back; trying says *This machine is not connected. Is inyon running on it?* When it reconnects you're told how long it was away, and a turn that finished while it was gone still counts as finished.

## Updating machines

A machine that joined the hub updates from its page under Machines, to the hub's own version. The hub sends a short script that checks the download first, so this works for any version.

## Removing a machine

**Remove** on its page disconnects it and revokes its token. From 0.16 the machine then leaves the hub by itself, as `inyon disconnect` would: it forgets the hub and removes its background service, so nothing starts it again. A machine that's off at the time does this the next time it starts. Its tmux sessions keep running, and Inyon stays installed; `inyon uninstall` removes it.

From 0.17, if the machine is online, the hub first takes back what it put there: the Claude Code and Antigravity integration, the uploads and the prompt log. For an SSH server it also closes the connection it kept open. See [Security](security.md#removing-a-machine-cleans-it-up).

Before 0.16 the service kept starting it again, and it called the hub every 30 seconds. On a machine you removed before then, run `inyon disconnect`.
