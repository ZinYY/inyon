---
title: Pairing a computer
nav: Paired computers
description: How a computer pairs with your phone, how the phone finds it from any network, and the options.
---

A paired computer runs `inyon` as a small background service and talks to your phone directly. There's no hub, no account and no open port. Pairing takes one command and one scan; see the [quick start](quick-start.md).

From 0.19 the link runs inside `inyon` itself, so a paired computer has no separate engine to download or keep updated. Phones, pairings and keys from earlier versions keep working unchanged.

## Pairing

```sh
inyon pair
```

prints a QR code, an ID like `K7Q4-MX2T` with a 6-digit code, and a link. Scan the QR code in the app, or type the ID and code, or paste the link. From 0.20.2 it also prints the computer's own ID (*This computer's ID is ybnd rfg8 ejkm*), and the app shows the same before a scanned or pasted code pairs.

- A code pairs one phone, once, within 10 minutes.
- Five wrong typed codes cancel the typed code (the QR code still works until it expires).
- `inyon phones` lists paired phones by maker and model (such as *Nothing A142*) and when each was last seen (updated at most once a minute, and only after the phone has proved its key); `inyon forget <name>` unpairs one at once, and from 0.20 it holds even while that phone keeps trying to connect. From 0.17, removing the computer in the app unpairs the phone on the computer too. From 0.20.3, if the computer is off then, the app says *&lt;name&gt; still trusts this phone* and tells the computer once it's back online; `inyon forget` there ends it at once.

Pair as many phones as you like, and pair the same phone with as many computers as you like.

## How it stays private

- **Keys.** Each computer has an ed25519 key made on it, stored in `~/.inyon/node/identity.json` and readable only by you. Its public half is the computer's ID. From 0.20, a damaged or unreadable `identity.json` stops the link with a message and is left as it is, so restore it from a backup; Inyon only makes a new key when the file is missing, and every phone then has to pair again. Pairing tells the phone that key, and tells the computer the phone's.
- **The handshake.** Every connection starts with a Noise IK handshake (X25519, ChaCha20-Poly1305) between those two keys. Only a phone the computer has paired, or one holding a live pairing code, gets through. Everything after the handshake is encrypted end to end.
- **The tunnel.** A Cloudflare quick tunnel reaches only the computer's link port on `127.0.0.1`, and carries only ciphertext.
- **Addresses.** The computer publishes where it can be reached on [pkarr](https://pkarr.org), signed with its key, every 30 minutes and whenever an address changes. A record that isn't signed by that key is ignored. From 0.20, a publish the pkarr relays turn down is tried again after about 10 seconds, then less often, up to every 10 minutes, until one takes it; before, a new tunnel address could stay unpublished until the next half hour.

[Security](security.md) covers the whole model.

## How the phone finds it

The computer can answer in several ways at once, and the phone races them:

| Way | Default | Option |
|---|---|---|
| Free Cloudflare quick tunnel | On | `--tunnel`, `--no-tunnel` |
| Private network: Tailscale, Headscale, NetBird, ZeroTier, WireGuard | On (off on Windows) | `--vpn`, `--no-vpn` |
| Local network | Off | `--lan`, `--no-lan` |
| An address you publish yourself | None | `--url wss://…/link` |

With the local network on, the link listens on every interface. From 0.20.3 it publishes only private, link-local and VPN (`100.64.0.0/10`) addresses, so a server's public address stays out of its record; a phone on the same network finds it by its private address.

Pass options to `inyon pair`, or choose them in `inyon setup`; they're saved. The link listens on port 4780 (`--port` changes it). `cloudflared`, which runs the tunnel, is downloaded into `~/.inyon/bin`, pinned to a known version and checked. From 0.20 `inyon setup` and `inyon service install` download it, so the first pairing doesn't wait for it; before, the link fetched it the first time it opened a tunnel. On Windows from 0.20, `cloudflared` stops with the link however the link ends.

The tunnel works from any network but has no uptime promise: if Cloudflare's quick tunnels are slow, a private network is the steadier route. The private network is off by default on Windows, because Windows Firewall blocks it until a rule allows it. So a Windows computer is reachable only through the tunnel until you change that, and from 0.20 `inyon`, `inyon status` and `inyon doctor` say so. When you turn on *LAN* or *VPN* in `inyon setup`, setup adds that rule with an administrator's permission (see [The firewall on Windows](cli.md#the-firewall-on-windows)) and restarts the link with the new settings.

While the tunnel starts, `inyon pair` says so instead of showing a code. From 0.20 it shows the code once the tunnel's address is in it. If the tunnel takes more than 30 seconds, it shows the code with whatever addresses already work (a LAN or VPN address), or says it's still starting, and shows the code again when the tunnel is up. Before 0.20 it showed a code with no address after 30 seconds, and a phone couldn't find the computer with it.

### Your own HTTPS address

To publish the link at an address of your own, such as through `tailscale serve`:

```sh
tailscale serve --bg --https=8443 http://127.0.0.1:4780
inyon pair --url wss://laptop.tail1234.ts.net:8443/link
inyon service install link    # restart the link with the new address
```

## Windows

On Windows, Inyon drives [psmux](https://github.com/marlocarlo/psmux), the native tmux for Windows. The machine's Setup tab installs it with winget. Everything works as on Linux, except that the terminal shows the current screen without scrollback.

psmux numbers panes per session, so every session has a pane `%1`. Before 0.17.1, two Claude Code sessions on one Windows machine shared the integration's files and the app lost their live model, context and limits. From 0.17.1 each session keeps its own, once the integration's scripts are updated (see [the Claude Code integration](machines.md#the-claude-code-integration)).

## Moving to a hub later

A paired computer can join a [hub](hub.md) instead: run the hub's *Add a machine* command on it. To stop pairing directly afterwards, `inyon service uninstall link` removes the link.
