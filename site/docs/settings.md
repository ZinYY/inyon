---
title: Settings
description: Every setting in the app, what it does and its default.
---

Settings live on the **Settings** tab. Display settings are saved per device; settings about machines and uploads are saved where they apply.

## Appearance

| Setting | Choices | Default |
|---|---|---|
| Theme | Light, Dark, Auto | Auto (follows the system) |
| Match the agent inside sessions | On, Off | On: a session takes its agent's colours and logo |
| Reply font | Auto, Serif, Sans, Mono | Auto: each agent's own reply font |
| Tool calls in chat | Everything, Compact | Everything |
| Haptic feedback | On, Off | On |

Under reduced motion (a system setting), nothing slides or moves; sheets and menus only fade.

## Agents

Where each agent is installed (*On laptop, server*), the command that starts it, and whether its sessions open as a chat or in the terminal.

From 0.22, **Add an agent** below them saves a command of your own with the agent it works like, and it shows up in **New session**. Right-click it, or use its **⋯** menu, to edit or remove it. See [Your own agents](agents.md#your-own-agents).

## Companion

From 0.22, on the hub's web app: start the [companion agent](companion-agent.md) with **Claude Code** or **Codex**, then **Open** its chat or **Stop** it. If its agent quits, **Start again** and **Turn off** take the place of the buttons.

## Live numbers

**Restart sessions that stop reporting.** If a session's live numbers stop updating, Inyon reconnects or restarts it by itself.

**Put idle agents to sleep** (from 0.24): **Off**, **30 min**, **1 hour** or **2 hours**; a time set through the API shows as one more choice. An agent idle that long exits to its shell, as **Exit to shell** does, keeping its pane, so it stops using memory. Open the session and choose [**Resume conversation**](sessions.md#when-nothing-is-running) when you need it again. Idle means the turn is over, nothing runs in the background and nothing is being asked, and the time counts from when the hub first saw it idle, so a hub restart starts it again. Pinned sessions stay awake, and so do sessions with a held message or a message sent later waiting, ones without a conversation to resume, and the companion. It's off unless you choose a time, and it applies to this hub's sessions; a paired hub has its own.

## Uploads and clipboard

| Setting | Choices | Default |
|---|---|---|
| Delete uploads after | 1, 7 or 30 days, Never | 7 days |
| Largest file | 25, 100 or 500 MB | 100 MB |
| Keep at most | 500 MB, 1 GB, 5 GB, No limit | 1 GB |

Uploads are stored on each machine in `~/.inyon/uploads`. The machine's page shows how much space they take, with buttons to clean older ones or delete them all.

From 0.21, **Agent temp files** below the machines opens [Temp files](temp-files.md): the pasted images, screenshots and scratch folders agents leave on each machine.

**Largest file** applies to each file, whatever its type. From 0.24 files go up a megabyte at a time, so the hub never holds more than that of a file however large it is.

From 0.17 the hub tidies every machine by itself about every six hours, and again when a machine comes back online: it deletes uploads past their age, then the oldest ones while the folder is over **Keep at most**. A file whose upload broke off keeps a `.part` ending and is deleted after an hour, so a half-sent file never looks finished. From 0.24 the hub also forgets an upload that got nothing for an hour, and deletes its `.part` file then; sending the file again starts it over. Computers paired only with a phone tidy their own uploads the same way, with the rules the hub last used there (7 days and 1 GB if there never was one).

## Notifications

**Allow**, **Send a test**, *When an agent finishes* and *When a page moves on*. On Android, also **Stay connected** and **Battery limits**. See [Notifications](notifications.md).

## Computers or Machines

A shortcut to the list. The app calls them *Computers*; a hub calls them *Machines*.

From 0.24 each one shows its last drop, such as *Last drop 2h ago, the network went away (down 3m)*, and a hub's page adds **This page's connection** once its own connection to the hub has dropped, with **Can't connect?**. See [Can't connect?](troubleshooting.md#cant-connect).

From 0.22 a hub adds **Machines can message agents** below the list. With it on, an agent on a joined machine can use [`inyon send`](cli.md#sessions-and-messages) to give agents on your other machines a message, which goes in once each is free. It's off until you turn it on. Listing machines, sessions and pages from a joined machine needs no switch.

## On a hub

- **Other devices.** The hub's address with a copy button, and how to open it in a phone's browser.
- **Paired devices.** From 0.20: **Show the code** to pair the Android app, **Pair a companion app** for an app like Legato (from 0.20.2 with **Copy pairing link**), the paired phones, each with **Forget**, and the companion apps with their permissions. A companion's menu (⋯, or a right-click) has **What it can do** and **Remove**. See [Companion apps](companions.md).
- **Account.** **Change password** (8 characters or more; signs out your other browsers) and **Sign out**.

## On the Android app

- **Hubs.** The hubs this phone is paired with, each with **Forget**. From 0.24 each shows its last drop, and **Can't connect?** while it's out of reach. [Why a link dropped](android.md#why-a-link-dropped) explains the reasons.
- **This phone's key.** The phone's SSH public key, and the command that adds it to a server's `~/.ssh/authorized_keys`. See [A server over SSH](android.md#a-server-over-ssh).

## Updates

What's installed and whether a newer version exists, with **Check now**:

- The Android app checks for a new release once a day and shows **Download** when there is one.
- A hub shows *Update available* with **Update**; see [Updating a hub](hub.md#updating).
- A browser copy reloads to its newest version.

## About

The version and build, the Android app's version, how the terminal is drawn, and **Copy version details** for a bug report. **How Inyon works** opens these docs (from 0.17; before, it opened the GitHub page). **Support Inyon** opens [Abhash's GitHub Sponsors page](https://github.com/sponsors/Abhash-Chakraborty) (from 0.16.1). Inyon is free and MIT licensed, so this is only for people who want to chip in.

## Keyboard shortcuts

| Keys | Does |
|---|---|
| `Ctrl K` / `⌘ K` | Search and jump to a session |
| `Alt N` / `⌥ N` | New session, even while typing a message (from 0.16; not in the terminal) |
| `N` | New session, when you aren't typing |
| `Enter` / `Shift Enter` | Send / new line |
| `Alt Enter` / `⌥ Enter` | [Hold the message](sessions.md#holding-a-message-until-the-agent-is-free) until the agent is free (from 0.22) |
| `Esc` | Stop the agent (with an empty message box), close a sheet or menu |
| `↑` `↓`, `Enter`, `Tab` | Move through and pick from menus, `/` and `@` lists |
| Any key | Focus the message box (on a computer) |
| `Ctrl V` / `⌘ V` | Paste into the terminal |

On Android, the back button closes the top sheet first, then goes back a page. A long press on a card opens its menu. On a computer, right-click for the same menus; see [Right-click menus](overview.md#right-click-menus).
