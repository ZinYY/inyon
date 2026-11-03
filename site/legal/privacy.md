---
title: Privacy policy
description: What this site, the inyon program and the Android app send anywhere, and to whom. Short version: nothing about you comes to me.
updated: 2026-10-23
---

Inyon is an open-source project I build and run on my own, not a company. There are no Inyon accounts and no Inyon servers that hold your data. Your sessions, prompts, code and settings stay on your own machines and phone.

This page covers the website at inyon.tech, the `inyon` program and the Inyon Android app. The [source code](https://github.com/Abhash-Chakraborty/inyon) shows everything described here.

## This website

- **No cookies, analytics or trackers.** The site doesn't count visits or keep anything in your browser. Search runs in your browser on a file downloaded from the site.
- **Hosting.** GitHub Pages serves the site and the downloads. Like any web host, GitHub receives your IP address and the pages you request, under [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement). I don't get those logs.
- **Fonts and scripts come from this site.** Nothing else loads from another server, with two exceptions:
  - the home page shows the Product Hunt badge, an image from `api.producthunt.com`;
  - the video under the home page's hero loads from YouTube's privacy-enhanced domain (`youtube-nocookie.com`) only when you press play.

## The inyon program

`inyon` runs on your computers and servers. It sends no telemetry, usage statistics or crash reports, anywhere.

It keeps its state in `~/.inyon` on each machine, readable only by your user: keys, paired phones, the hub's account and settings. [Security](/docs/security/) lists every file.

It talks to these outside services, and only for the reason given:

- **inyon.tech** (GitHub Pages), when installing, updating and checking for a new version. It sees your IP address and which file was fetched.
- **Cloudflare quick tunnels** (`trycloudflare.com`), on a computer paired with a phone directly, unless you turn the tunnel off. Cloudflare sees your IP address and encrypted traffic: phone and computer encrypt end to end (Noise IK), so it can't read or change what passes.
- **pkarr relays** (`relay.pkarr.org`, `dns.iroh.link`), on the same computers, every 30 minutes and when an address changes. They get a record signed by the computer's key, listing the addresses where it can be reached.
- **The npm registry** (`registry.npmjs.org`), on a hub, when a machine's Setup tab opens, at most hourly per package. It gets the name of an agent's npm package.
- **Anthropic** (`api.anthropic.com`), on a machine signed in to Claude Code, to show plan limits. The request uses Claude Code's own sign-in and goes from that machine only; the token never reaches the hub.
- **Browser push services** (Google, Mozilla, Apple), when a computer sends alerts to a browser copy you allowed notifications in. Each message is encrypted for that browser (RFC 8291), so the push service can't read it.
- **Agents' installers**, only when you press **Install** or **Update** for an agent. They fetch what that agent's official installer or npm fetches.

Coding agents such as Claude Code, Codex and Gemini CLI send your prompts and code to their own providers, under those providers' terms. Inyon shows and types into the agents you already run; it doesn't change what they send.

## The Android app

- **No account and no tracking.** Everything the app knows stays on the phone. Its key, passwords and hub tokens are in Android's encrypted storage, under a key in the phone's Keystore, and none of its data goes into backups. Uninstalling removes it all.
- **Camera.** Used only to scan a pairing code, and only while the scanner is open. Scanning uses Google's ML Kit barcode scanner from Google Play services. Google says ML Kit may send diagnostic and usage information about the scanner to Google; it doesn't send the images or the code.
- **Notifications.** The app watches your computers itself and posts its notifications on the phone. No notification service in between sees them.
- **Updates.** Once a day the app asks this site for the newest version, and downloads it from here when you tap **Download**. It doesn't use Google Play.
- **Files you attach** go to your own computer or hub, nowhere else.

## Things you post in public

Issues, discussions and vulnerability reports on GitHub are under GitHub's terms. Issues and discussions are public; a private vulnerability report is seen only by me until it's published.

## Your choices

Since I hold no data about you, there's nothing for me to export or delete. On your own machines, `inyon uninstall` removes Inyon and its state, and [Security](/docs/security/#removing-a-machine-cleans-it-up) explains what removing a machine cleans up.

## Children

Inyon is a developer tool. It isn't aimed at children and collects nothing from anyone.

## Changes

When this policy changes, the date at the top changes too, and the change is in the site's [history on GitHub](https://github.com/Abhash-Chakraborty/inyon/commits/main/site/legal/privacy.md). A change that makes Inyon send something new will also be in the release notes.

## Contact

Questions about privacy: open an [issue on GitHub](https://github.com/Abhash-Chakraborty/inyon/issues). For anything you'd rather not post in public, use [a private report](https://github.com/Abhash-Chakraborty/inyon/security/advisories/new).
