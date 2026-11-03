---
title: Introduction
description: Inyon lets you watch, answer and steer the coding agents running in tmux on your computers, from your phone or any browser.
---

You start an agent on a computer: Claude Code, Codex, Gemini CLI, OpenCode, GitHub Copilot or Antigravity CLI, running in tmux. Inyon shows that session on your phone as a chat. Eleven more, such as Cursor, Grok, Pi and Amp, show as [terminal sessions](agents.md#terminal-first-agents). When the agent asks something, the question arrives as buttons. You tap one and the agent carries on, exactly as if you'd typed the answer at the keyboard.

![Inyon on a desktop, a tablet and a phone: the overview of every agent session across four machines, a Claude Code session read as a chat, and Codex asking to run the tests](../../docs/hero-light.webp)

## What makes it different

**It never signs in to anything.** Inyon drives the agent CLI that's already running. A laptop on your personal account, a server on your company's subscription and a box with only an API key all show up side by side, each keeping its own account.

**Nothing changes at the keyboard.** The agent keeps running in its tmux pane. Walk away, answer from your phone, sit back down to the same terminal. Close Inyon and nothing breaks.

**There's nothing to host.** The Android app talks to your computers itself, over a free encrypted tunnel or your VPN. A hub server is optional, for when you also want a browser.

**It's yours.** Inyon is open source under the MIT licence. Nothing on this site connects to your machines.

## How it fits together

Each computer runs tmux and your agents, plus Inyon's small `inyon` program, unless you reach it over SSH instead. Your phone pairs with each computer by scanning a QR code. If you'd rather use a browser, one machine that stays on runs a hub: it serves the web app, other machines join it, and your phone can pair with it too.

![The phone or browser talks to one hub. The hub reads its own machine directly, laptops connect out to it by running inyon, and servers are reached over SSH. The Android app can do the same without a hub.](../../docs/how-it-works-anim.webp)

On each machine Inyon reads tmux, the process tree and the agents' own session files, and types your answers back with `tmux send-keys`. [Choose a setup](setups.md) compares the three ways to connect.

## Where to go next

- [Quick start](quick-start.md): install the app, pair a computer and answer your first prompt, in about two minutes.
- [Choose a setup](setups.md): paired computers, a hub, or plain SSH, and when to use each.
- [Sessions](sessions.md): everything you can do inside a session.
- [The inyon command](cli.md): every command and flag.
- [Troubleshooting](troubleshooting.md): when something doesn't connect.

## Status

Inyon is young and moves quickly; the [changelog](changelog.md) lists every release. Linux, macOS and Windows (through psmux) work today. The readers for Gemini CLI, OpenCode and Copilot are in beta, the Android app is the only phone app, and the iOS app isn't built yet.
