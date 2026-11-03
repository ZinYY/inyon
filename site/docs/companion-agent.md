---
title: Companion agent
nav: Companion agent
description: Run a Claude Code or Codex session that reads your other sessions, tells you what needs you and messages them when you ask.
---

From 0.22 Inyon can run a companion agent: a Claude Code or Codex session you talk to about your other sessions. Ask what's going on and it reads every session on every machine, then answers in a few lines. Tell it what one of them should do next and it sends the message once you've agreed. It runs on your own sign-in, so it needs no API key.

Talk to it from its chat in the app, from your phone, or from a companion app such as Legato, whose island takes your dictation and shows the reply.

## Start it

In **Settings › Companion**, pick **Claude Code** or **Codex** and press **Start**. Or, on the hub's computer:

```sh
inyon companion start                # with Claude Code
inyon companion start --agent codex
inyon companion                      # whether it runs
inyon companion stop
```

The hub starts it in a tmux session called `companion` on its own computer, in the folder `~/.inyon/companion`. A hub on Bun (0.22 with `INYON_HUB_IMPL=bun`) has no companion. If a tmux session called `companion` that Inyon didn't start is already open, Inyon leaves it alone and asks you to rename or close it first.

While it runs it shows as **Companion** in the sidebar under **Overview**, and at the top of the Overview with its state and what it needs. **Settings › Companion** has **Open** and **Stop**. If its agent quits, the same place offers **Start again**, which starts it in a fresh session.

## What it can do

It works through the `inyon` command, the same one you have:

| Command | Does |
|---|---|
| `inyon sessions` | every session on every machine, with its agent, state and folder |
| `inyon hub machines` | the machines, and whether each is online |
| `inyon read <machine>/<session>` | the latest messages of a session's conversation |
| `inyon send <machine>/<session> "text"` | a message for that agent, held until it's free |
| `inyon answer <machine>/<session> <choice>` | answers the question an agent is asking, by number or by words from the choice |
| `inyon pages --hub` | the pages your agents published |

[The CLI page](cli.md#sessions-and-messages) has the details of each.

Its instructions are in `~/.inyon/companion/AGENTS.md`, with the same text in `CLAUDE.md` for Claude Code. They tell it to:

- keep replies short and plain, since they may be read aloud or shown on a small screen;
- lead with what needs you, such as an agent waiting for an answer;
- send a message or answer a question only when you asked for it in that conversation, and say first what it will send;
- leave your projects alone, and reach other sessions only through these commands, never with tmux keys or SSH.

Inyon rewrites both files each time it starts the companion, as long as their first line is still there. Delete that line to keep your own changes.

## What asks first

For Claude Code, Inyon writes `~/.inyon/companion/.claude/settings.json` when it isn't there yet. It lets the commands that only read run without asking (`inyon sessions`, `inyon read`, `inyon hub machines`, `inyon pages`, `inyon docs`). `inyon send` and `inyon answer` still ask, so each one shows as a question in the app and on Legato's island, and nothing goes out until you allow it. A `settings.json` you wrote yourself is left as it is, so if it allows `inyon send` or `inyon answer`, those run without asking.

Codex starts with `--sandbox workspace-write` and network access turned on, so the `inyon` command can reach the hub on the same computer. Inyon writes a rule to `~/.inyon/companion/.codex/rules/companion.rules` that makes `inyon send` and `inyon answer` ask first. Codex reads a folder's rules only once the folder is trusted, so Inyon also adds the folder to `~/.codex/config.toml` as trusted. If that file already names the folder and doesn't trust it, Inyon leaves it alone and starts Codex with `--ask-for-approval untrusted`, so it asks before every command that isn't known to be safe, reading ones included.

## Its replies

When the companion finishes a turn, its reply shows as a notice in every open copy of the app, unless you're in its chat already. The phone app shows it as a notification while it's in the background, and so do browsers you turned notifications on for. The usual "finished" alert isn't sent for the companion.

A companion app with the **chat** permission gets each reply as a `companion.reply` frame, without subscribing to anything. See [the WebSocket](companions.md#the-websocket) for its fields.
