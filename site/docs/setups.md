---
title: Choose a setup
description: Three ways to connect a machine (paired, hub and direct SSH), what each needs and when to use it.
---

Every machine reaches Inyon one of three ways. You choose per machine, and you can mix them: a laptop paired with your phone, a home server running a hub, and a work server reached over SSH can all show up in the same list.

| | Paired | Hub | Direct |
|---|---|---|---|
| Runs on the machine | `inyon` as a small user service | A hub on one machine; others join it | Only tmux and your agents |
| You use it from | The Android app | Any browser, and the Android app | The Android app, or a hub |
| Open ports | None | The hub's, on a private network | SSH |
| Works from anywhere | Yes, through a free tunnel | Over a VPN or your own HTTPS address | Wherever SSH reaches |
| Best for | Laptops and desktops | A browser, many machines, one place | Servers you can't install on |

## Paired

Install `inyon` on the computer and scan its QR code with the app. That's the [quick start](quick-start.md).

The computer opens a free Cloudflare quick tunnel and publishes its current address, signed with its own key. The phone checks the signature, then the two run an encrypted handshake, so the tunnel only ever carries ciphertext. There's no account, no open port and no server in between. On Tailscale, NetBird, ZeroTier or WireGuard, the phone uses that network directly instead.

Use it for laptops, desktops and any machine you can install a program on. [Pairing a computer](computers.md) covers the details.

## Hub

A hub is one process on a machine that stays on, such as a home server, a VPS or a desktop that doesn't sleep. It serves the full web app to any browser and reaches other machines:

- its own machine, directly;
- laptops and desktops that join it by running `inyon` (they connect out, so they need no open ports);
- servers over SSH, with your existing `~/.ssh/config`, keys and jump hosts.

Your phone can pair with a hub by scanning a code in its web app. The hub's machines then appear in the app next to the phone's own computers.

Use it when you want a browser, when you have many machines, or when you want one place that sends notifications for all of them. [Running a hub](hub.md) explains how.

## Direct

Some servers don't let you install anything. They only need tmux and your agents:

- From the Android app, tap **Add a computer**, then **Connect over SSH**. The phone connects with its own key, a password or a private key you paste, and pins the server's host key. See [A server over SSH](android.md#a-server-over-ssh).
- From a hub, choose **Add a machine → Reach it over SSH**. The hub uses the system `ssh` with your config. See [Adding machines](machines.md#servers-over-ssh).

The phone has to reach the server itself, so use the same Wi-Fi or a VPN.

## Combining them

- **Phone only:** pair each laptop; add servers over SSH from the phone.
- **Browser and phone:** run a hub on a home server, have your laptops join it, add work servers over SSH from the hub, and pair your phone with the hub.
- **Both at once:** the app shows paired computers and a hub's machines side by side, so a laptop can stay paired with the phone even while a hub exists.
