---
title: Agents
description: What works with each coding agent, where Inyon reads it from, and the limits of each.
---

Inyon works with the agent CLIs you already use, signed in however you like. It reads each one from its own records and its screen, and never asks it to do anything you couldn't do at the keyboard.

## What works with each

| | Claude Code | Codex | Gemini CLI | OpenCode | Copilot | Antigravity CLI |
|---|---|---|---|---|---|---|
| Chat view | Full | Full | Beta | Beta | Beta | Yes |
| Reply shown as it's written | From 0.22 | From 0.17.6 | No | No | From 0.22 | From 0.22 |
| Prompts as buttons | Yes | Yes | Yes | Yes, several answers too | Yes | Yes |
| Sub-agents listed | Yes | From 0.17.6 | No | No | No | No |
| Model | Yes | Yes | No | Live list | Live list | Yes |
| Effort | Yes | Yes | No | Every variant | No | Yes |
| Mode | Yes | Yes | No | Its agents, custom too | Yes | Yes |
| Providers | No | No | No | Connect, disconnect | No | No |
| Goal | Yes | Yes | No | No | No | Set only |
| Compact | Yes | Yes | Yes | Yes | Yes | No |
| Rewind | Yes | Per message | No | Undo, redo | No | Yes |
| Share | No | No | No | Link, stop sharing | No | No |
| [Past conversations](sessions.md#past-conversations) | Yes | Yes | No | Yes | Yes | Yes |
| `!` shell commands | Yes | Yes | No | Yes | Yes | No |
| Remote Control | Yes | No | No | No | No | Yes |
| [Ultracode](controls.md#ultracode) | Yes | No | No | No | No | No |
| Cost | Yes | No | No | Yes | No | No |
| Plan limits | 5-hour and weekly | 5-hour, weekly, credits | No | No | No | 5-hour and weekly |

## How Inyon reads them

| What | Where it comes from |
|---|---|
| Panes and agents | tmux and the process tree; live changes through a read-only tmux control client |
| Conversations | Each agent's own session files or local server |
| Replies as they're written | Codex: the live events of its own app-server, joined on the machine (see below). Claude Code, Copilot and Antigravity: the reply as their screen shows it (see below) |
| Prompts and spinners | The pane's screen, parsed into options |
| When a turn starts and ends | The screen, and with the integration the agent's own hooks: Claude Code's, and from 0.22 Copilot's, Gemini CLI's and Antigravity CLI's, so a turn shows as working or finished at once, even while the screen looks idle |
| Plan limits | Claude: the account's usage, read on the machine the same way `/usage` does. Codex: its status events, and from 0.21 the account itself through `codex app-server`, with no session running. Antigravity: its status events |
| Context and cost | Claude's status line data (with the integration) and transcripts; OpenCode's own record, plus its sidebar and status row; the others' own events |
| Typing and answering | tmux, exactly as if you typed it: keys with `send-keys`, and from 0.20 text through a tmux paste buffer, so it never shows on a command line |

From 0.22 the chat also shows Claude Code's, Copilot's and Antigravity's reply while it's being written. They save a message to their records only once it's finished, but their screen shows it as it streams, so Inyon reads it from there, the text after the last thing the agent ran, and shows it under the working line. When the message lands in the record it takes the place of that copy. Read off a screen, it can lose some formatting, such as a table, until then. When a long reply scrolls its start off the screen, Inyon keeps the part it already read.

The agents never know Inyon is there. Close it and nothing breaks.

## Claude Code

Read from its transcripts in `~/.claude`. With the optional integration (turned on from a machine's Integration tab), a hook gives exact session matching and the status line gives live context, cost and limits. Models, effort, permission modes, goals, Remote Control, [ultracode](controls.md#ultracode), compact, clear and `/rewind` all work.

Claude Code can move a window's conversation to a background session, which then runs as a process of its own, outside tmux, with a new transcript. The window keeps showing it and takes your messages for it. From 0.22.1 Inyon follows the conversation there too: the chat, sends and alerts go with the background session for as long as the window shows it.

## Codex

Read from its session files in `~/.codex/sessions`. Model and effort go through Codex's own picker. It shows plan limits and credits, every message of yours can **Rewind to here**, and `$` lists skills. Codex runs its hooks inside a background process that can't tell which pane it belongs to, so Inyon reads Codex's prompts from the screen.

From 0.17.6 the chat shows Codex's reply while it is being written, the way the terminal does, with its thinking folded above it. Codex's terminal is a client of a background process on the machine (its app-server), which other clients may join; Inyon joins the open conversation from the machine with a small `node` script and listens, without sending anything. It needs `node` on the machine, which an npm install of Codex has; without it the reply still appears when it's complete. Joining also brings the turn's timer, token usage the moment Codex reports it, and sub-agents as they start and finish.

The same release reads more of Codex's record: a question Codex asks mid-turn shows as a card with its choices and your answer, a plan written in Plan mode shows as a plan card, a script that ran several tools lists each step with the diffs of its patches, sub-agents appear under **Running** with what they reported back, and each turn carries how long it took. While Codex works, the switch by the send button decides whether a message is queued or steered in; see [Writing messages](sessions.md#writing-messages).

## Gemini CLI (beta)

The app lists it as *Gemini CLI (legacy)*, next to Google's newer Antigravity CLI. Read from Gemini's chat recordings in `~/.gemini/tmp`. Prompts become buttons and context is shown. From 0.22 the integration adds Inyon's hook to `~/.gemini/settings.json`, next to any of your own. Inyon can't change Gemini's model yet, and doesn't list its past conversations: resume those in the terminal. `/compress` compacts.

## OpenCode (beta)

Made for OpenCode 1.19. Inyon reads a session two ways: one it started (`--hostname 127.0.0.1 --port 0`) through OpenCode's own local server, any other through OpenCode's database and its screen. Either way the terminal's footer (agent · model · variant) is what the app shows, and a pick made in the app goes through OpenCode's own keys, so the two never disagree. While a menu or a question hides the footer, the last values stand.

From 0.17.6:

- **Providers.** A machine's Setup tab lists OpenCode's providers, connected ones first, and connects or disconnects them with OpenCode's own `opencode auth login` and `opencode auth logout`. The same list opens from a session's model menu (*Connect a provider…*), on the phone too, and the model list follows at once. See [Adding machines](machines.md#the-setup-tab).
- **Agents and variants.** The agent list comes from the machine, custom agents included, and every variant a model has is read, whatever it's called; *Default* means none.
- **Context and cost** come from OpenCode's own record (the last reply's whole usage, counted as its sidebar counts it), and the sidebar and status row fill in on the session cards.
- **Undo, redo, share and compact**, and a new conversation, work in both modes; a session Inyon started gets them through its server.
- **Questions** that take several answers, or ask several things at once, show as checkboxes and tabs with a Confirm button.
- OpenCode's key bindings are read from its config (user and project), so a remapped Tab or Ctrl+T doesn't break the controls.
- With several conversations in one folder, the one the terminal shows is found by its title.

A password-protected OpenCode server isn't supported. A plain `opencode` already running sees a provider connected from the app after it restarts; a session Inyon started sees it at once.

## GitHub Copilot (beta)

Found through its process locks and event log. Its models come from the CLI's own catalog, so they match what your account can use. Assisted approval and effort changes stay in the terminal, and sign-in uses Copilot's device-code flow. From 0.22 the integration adds Inyon's hook to Copilot, in a file of its own (`~/.copilot/hooks/inyon.json`).

## Antigravity CLI

Google's `agy`. Read from its transcripts, with plan quotas from its own status line. From 0.22 the integration also adds an entry named `inyon` to its hooks in `~/.gemini/config/hooks.json`. Models, effort, modes, goals (set, but not cleared yet), Remote Control and `/rewind` work.

## Terminal-first agents

From 0.23 Inyon knows eleven more agent CLIs. Their conversations stay in their terminal: Inyon doesn't read their session files yet, so their sessions open on the terminal and there is no chat view for them.

| Agent | Command | Signs in with |
|---|---|---|
| Cursor CLI | `cursor-agent` | `cursor-agent login` |
| Grok Build | `grok` | `grok login --device-auth` |
| Pi | `pi` | `/login` inside Pi |
| Amp | `amp` | `amp login` |
| goose | `goose` | `goose configure` |
| Hermes Agent | `hermes` | `hermes setup` |
| Kimi Code | `kimi` | `kimi login` |
| Kiro CLI | `kiro-cli` | `kiro-cli login --use-device-flow` |
| Qwen Code | `qwen` | `/auth` inside Qwen Code |
| Qoder CLI | `qodercli` | `qodercli login` |
| CodeBuddy Code | `codebuddy` | its own first run |

What works with them:

- A pane running one shows its logo and colours, found by its process the way the others are.
- Its screen says when it's working (a line such as *esc to interrupt*) and when it asks something, so the session's state and its notifications work.
- **New session** starts it. It starts without a first message; type that in its terminal.
- A machine's **Setup** tab installs it with its official installer or npm, updates it, and **Sign in** opens a terminal session running its sign-in.

Not yet: the chat view, model, effort and mode controls, past conversations, and [your own agents](#your-own-agents) that work like one of them. The [Android app](android.md) on its own, for a computer it pairs with directly or a server over SSH, shows their panes as plain terminals and leaves them out of the Setup tab; through a hub the phone shows them in full.

An app from before 0.23 doesn't know these agents either, so a hub shows it their sessions as plain terminals. Update the Android app to see them.

## Your own agents

From 0.22, **Settings › Agents › Add an agent** saves any command under a name of your own, with the agent it works like. Use it for an agent started with flags you always want (`claude --dangerously-skip-permissions`), for a build of an agent under another name (`openclaude`), or for a wrapper of your own.

| Field | What it does |
|---|---|
| Name | Shown in **New session** and on the session's card. Up to 40 characters, unique |
| Command | What a new session types. Model, mode and the first message are added after it, the same flags the agent it works like takes |
| Works like | Claude Code, Codex, Gemini CLI, OpenCode, Copilot or Antigravity CLI. The session gets that agent's logo, chat view, prompts as buttons and controls |

Inyon spots one already running in any pane, too: a process whose program is the command's first word (by name, by path, or as the script `node`, `bun`, `deno`, `python` or `uv` runs) and whose command line holds every other word of it. Yours are tried before the built-in agents, so plain `claude` stays Claude Code while `claude --dangerously-skip-permissions --model opus` is the agent saved with that flag. When the command you saved is a script that starts the agent without `exec`, Inyon follows it to the agent and keeps your name.

The list is the hub's, for every machine, with at most 24 agents. A hub on Bun (up to 0.22, with `INYON_HUB_IMPL=bun`) can't save them.

Hooks come with it when it reads the same settings files as the agent it works like. One with hooks of its own can tell Inyon when a turn starts and ends the same way the integration does: from inside tmux, pipe the event's JSON to `~/.inyon/hook.sh <agent> <event>`, where `<agent>` is the agent it works like (`copilot`, `gemini` or `antigravity`) and `<event>` that agent's event name. For one that works like Claude Code, run `~/.inyon/hook.sh` with no arguments and Claude Code's hook JSON.

## Pages an agent makes

From 0.20 any of them can put a site it built, such as a plan or mockups, in the app with `inyon pages add <folder>`. The [Blueprint](skills.md#blueprint) skill does it on its own. The page records who added it: `claude`, `codex`, `gemini` or `opencode` when one of those ran the command, `cli` for anything else. See [Pages](pages.md).

## Installing and signing in

A machine's **Setup** tab installs any of them, with the official installer or npm (Codex, Gemini CLI and Copilot need Node.js, which it can install too), and runs each one's sign-in as steps in the app. From 0.17.4 it also shows which agent has a newer version and updates it with one button, from the browser or the phone. See [Adding machines](machines.md#the-setup-tab).

## Adding support for an agent

Most of what Inyon knows about an agent lives in one file, `packages/protocol/src/agents.ts`. A chat view also needs a reader for the agent's session files. [CONTRIBUTING.md](../../CONTRIBUTING.md) explains how.
