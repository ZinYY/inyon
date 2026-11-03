---
title: Quick start
description: Install the Android app, pair your first computer and answer your first prompt.
---

This takes about two minutes and needs nothing hosted anywhere. You need an Android phone and a computer with tmux and at least one coding agent. If the computer has neither yet, the app can install them for you after pairing.

## 1. Install the app

Download the APK on your phone and open it:

<p><a class="btn" href="/download/inyon.apk">Download for Android</a></p>

Android asks once to allow installs from your browser. Later versions install over this one, and the app tells you when one is out.

## 2. Set up the computer

On macOS or Linux, open a terminal (on a server, your SSH session) and run:

```sh
curl -fsSL https://inyon.tech/get.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://inyon.tech/get.ps1 | iex
```

It downloads `inyon`, checks it against the release's checksums, installs it as a background service and prints a QR code. [Install](install.md) explains each step.

## 3. Pair

In the app, tap **Add a computer**, then **Scan the code**, and point the camera at the QR code. The app asks *Pair with &lt;computer&gt;?*; tap yes. The computer appears under **Computers**, with its tmux sessions on the overview.

If the terminal is on the same phone, you can't scan it. Tap **Type a code** instead and enter the ID and the 6-digit code printed under the QR code. Each code works once, for 10 minutes.

> **Tip:** To show the code again later, run `inyon pair` on the computer.

## 4. Start an agent

Start one the way you always do, in tmux:

```sh
tmux new -s work
claude
```

Or start it from the app: tap **+**, pick the agent, the computer and a folder, and tap **Start Claude Code** (or whichever agent you picked).

The session appears on the overview within a few seconds. Tap it to read it as a chat.

## 5. Answer something

Ask the agent to do something that needs permission, such as running the tests. The session moves to the top of the overview under **Needs you**, with the agent's options as buttons. Tap one; the agent carries on, and the card says **Sent**.

## Next

- Turn on notifications in **Settings → Notifications**, so you hear about prompts while the app is in the background. See [Notifications](notifications.md).
- Pair your other computers the same way, or reach a server over SSH. See [Choose a setup](setups.md).
- Want a browser too? [Run a hub](hub.md) on a machine that stays on.
