---
name: inyon
description: Work on a computer that runs Inyon, the tmux and coding-agent remote (the `inyon` command). Use it whenever the user mentions Inyon, a hub, a joined or paired machine, the Inyon app or Pages, or asks you to set Inyon up, add a machine or phone, show them a site, find out what this computer is in their setup, or say which address to open; and before you guess anything about Inyon from its files or processes.
---

# Inyon

Inyon lets someone run coding agents in tmux and follow them from a phone or browser. Every computer
in their setup runs the `inyon` command, and each one plays one role. Find out which before you do
anything, then read the page of the docs that covers the task. The docs ship inside the program, so
they match the version installed.

## Where am I?

```sh
inyon status --json
```

| `role` | What this computer is | What you can do here |
|---|---|---|
| `hub` | Runs the hub: the web app people sign in to, which reaches every other machine | Add pages, show the setup code, see every machine and session, message any agent |
| `joined` | Joined to the hub named in `hub`. It connects out, so the hub shows its sessions | Add pages, see the hub's machines, sessions and pages, message an agent on another machine (all from 0.22) |
| `node` | Paired straight with the Android app over its own link, no hub | Pair another phone with `inyon pair` |
| `none` | Not set up yet | Tell the user to run `inyon setup`, which asks questions in their terminal |

The same object has `machine` (this computer's id), `jobs` (what runs and its version), `agents` (the
agents in tmux here), `hints` and `docs`. A note in `note` means something needs attention, such as
the hub being unreachable.

## Read the docs

```sh
inyon docs                 # every page with one line on what it covers
inyon docs machines        # one page as Markdown, with its address on inyon.tech
inyon docs add machine     # a name, the start of one, or words from its title all work
```

| For | Topic |
|---|---|
| Installing, the first run | `install`, `quick-start` |
| Which setup to pick: paired, hub or SSH | `setups` |
| Running a hub, reaching it from a phone | `hub` |
| Adding a laptop or server to a hub | `machines` |
| Pairing a computer with the phone | `computers` |
| Showing a site in the app | `pages` |
| Every command and flag | `cli` |
| What each agent supports | `agents` |
| Something doesn't work | `troubleshooting` |

Give the user the page's inyon.tech address when they'd rather read it themselves.

## Common tasks

**Show the user a site you built** (a plan, a report, mockups): a folder with an `index.html`.

```sh
inyon pages add <folder> --name "Readable name"
```

It answers with where the page sits in the app (Pages, then machine, then project). From 0.22.5 the
app files a page you add from inside a Claude Code or Codex session under that session's name, and
follows when the user renames it. Hand that line over, not a URL: a link that opens a page is made by the app and lasts 12 hours. On a joined machine `add`
also says the hub it's joined to shows its pages: that needs the hub and this machine on 0.22 or
later. With an older hub, tell the user to update it, or put the folder on the hub's computer. Don't
serve the folder on a public address. The `blueprint` skill builds and publishes plan sites this way.

**See what runs on the hub's machines, or message another agent** (hub or joined, from 0.22):

```sh
inyon sessions                                   # machine/session, agent, state, folder
inyon hub machines
inyon pages --hub
inyon send laptop/experiment "The report is in ~/inyon-reports/2026-10-24.md"
```

`send` doesn't type at once: the hub holds the message until that agent has been idle for a moment,
and the user sees it in the app until then. From a joined machine it works only when the user turned
on **Settings › Machines › Machines can message agents**; if the answer says it's off, tell them that
and don't look for another way in (no SSH and `tmux send-keys` into their sessions). Say who you are
and where you are in the message, since it arrives as if typed.

**Add a computer to a hub.** The hub makes the command: **Machines → Add a machine** in its app gives a
one-line command with a token for that one machine. Ask the user to run it on the new computer. Never
make up a token or ask the user to paste one into the chat.

**Pair a phone.** `inyon pair` prints a QR code and a code to type. The user scans it in the app.

**Run a hub.** `inyon hub start` on Linux or macOS. On Windows the hub runs inside WSL.

**Clean up temp files agents left.** `inyon temp` lists them. Once the user agrees, `inyon temp clean
--older-than 7d --yes` deletes the old ones (without a terminal it deletes nothing unless you pass `--yes`).

## Rules

- Every command takes `--json`. A failure is `{"v":1,"ok":false,"error":"…"}` with exit code 1. Read
  the error out to the user rather than retrying blindly.
- Ask before anything that cuts the user off from their own machines: `inyon stop`, `disconnect`,
  `uninstall`, `forget`, `hub stop`, `hub reset-password`. Don't restart the hub while they're using
  it.
- Never print or copy what's in `~/.inyon` (the machine's key and token, `hub.env`, the hub's
  records). Read state through `inyon status --json`.
- If your sandbox can't write to `~/.inyon`, the command says so. Give the user the exact line to run
  in their own terminal.
- Never type into the user's own tmux sessions to test something. Make a throwaway session.
