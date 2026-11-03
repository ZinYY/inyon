---
title: Steering an agent
nav: Controls
description: Model, effort, permission mode and goals; compact, clear and rewind; background work, usage and limits.
---

Everything you'd change at the keyboard has a control in the session. On a computer they sit as chips under the message box and in the header's **⋯** menu. On a phone, tap **More** in the header for the controls sheet.

While the agent is asking you something, the controls are hidden: typing would answer the question, so they can't work until it's answered.

## Model, effort and mode

| Control | What it changes | Agents |
|---|---|---|
| **Model** | The model for the next reply | Claude Code, Codex, OpenCode, Copilot, Antigravity |
| **Effort** | How hard the model thinks, from *Minimal* to *Max* (Codex adds *Ultra*). Until you pick one it shows the model's own default: *High* for Claude Opus, *Medium* for Sonnet. For OpenCode it's the model's variants, with *Default* for none | Claude Code, Codex, OpenCode, Antigravity |
| **Mode** | How much the agent may do without asking | Claude Code, Codex, OpenCode (*Agent*), Copilot, Antigravity |

Each agent offers its own choices. Claude Code's modes are *Ask each time*, *Accept edits*, *Plan*, *Auto* and *Bypass permissions*. Codex's are *Ask for approval*, *Approve for me*, *Read-only*, *Full access* and *Plan*. OpenCode's are its agents, *Build*, *Plan* and any custom one on the machine, and Copilot's *Manual approval* and *Full access*. Model lists for Codex, OpenCode, Copilot and Antigravity come live from the agent and refresh every minute, so they match what your account can use. OpenCode's list is what its connected providers offer; from 0.17.6 the model menu ends with **Connect a provider…**, which adds one (see [Adding machines](machines.md#the-setup-tab)).

Inyon changes them the agent's own way, through its picker or its keys, so the terminal shows the same thing. For OpenCode, from 0.17.6, the terminal's footer is what the app shows, in both directions: a pick in the app goes through OpenCode's own keys, a pick in the terminal shows in the app at its next screen read, and while a menu or a question hides the footer the last values stand. Some can't change mid-reply: a mode change waits until the agent finishes working.

From 0.22, a model, effort or mode you pick for Claude Code, Codex or Antigravity while it's working, or while it asks you something, waits until it's free. The control shows your pick with a small clock until then, on every device, and its menu has **Don't switch** to keep what runs now. Pick again before it's made and the newer pick is the one that goes. Messages held for the agent go after it. If the agent doesn't take the pick, the control shows why, and nothing more is tried until you pick again or dismiss it from the menu.

A model or effort you pick in Inyon is for that session only. Claude Code saves a `/model` or `/effort` pick as your default for new sessions, so from 0.17.1 Inyon puts `model`, `effortLevel` and `modelSettings` in `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`) back the way they were once Claude Code has written them. Nothing else in the file changes. To change the default, pick the model in the terminal.

## Goals

A goal is a condition the agent checks before it stops: it keeps going until the goal is met. Tap the **Goal** chip and write one, such as *All tests pass and the PR is open*, then **Set goal**. The chat marks when a goal is set, met or cleared, and the sheet shows the last goal's outcome. **Turn off** clears it.

Goals work with Claude Code and Codex, and with Antigravity (which can't clear one yet).

## Remote Control

Claude Code and Antigravity can hand a session to their own apps. **Turn on Remote Control** in the menu starts it; an icon in the header then offers **Open on claude.ai** (or **Open in Antigravity**) and **Turn off**.

## Ultracode

Claude Code has a switch next to its effort levels: with **ultracode** on, it runs a dynamic workflow, many agents working in parallel, for every substantial task. From 0.17.6 the switch is in Inyon too, for the session only, the way `/effort ultracode on` is. On a computer it's the last row of the **Effort** menu; on a phone it's **Ultracode** in the controls sheet. While it's on, the message box carries the same *ultracode* label that Claude Code draws on its input box, and the header shows a gauge mark that turns it off. Claude Code says when it can't: dynamic workflows have to be enabled in its `/config`, and not every model has them. Inyon shows that answer.

## Compact, clear, rewind

| Action | What it does | Agents |
|---|---|---|
| **Compact context** | Summarises the conversation to free up context | Claude Code, Codex, Gemini CLI, OpenCode, Copilot |
| **Clear conversation** | Starts a fresh conversation in the same pane, after asking | Every agent |
| **Rewind…** | Goes back to an earlier point | Claude Code, Antigravity, Codex |
| **Undo last message…** | Takes back your last message and its file changes | OpenCode |
| **Redo last undo** | Puts back what Undo took (from 0.17.6) | OpenCode |
| **Share link…** | Publishes the conversation at a public link, after asking; the link is under Usage, and **Stop sharing** takes it down (from 0.17.6) | OpenCode |

In Codex, every one of your messages also has **Rewind to here** while the agent is idle.

## Stopping and ending

- **Stop** (or `Esc` in an empty message box) interrupts the agent, like pressing `Esc` in the terminal. From 0.22 a stop pressed on two devices at once, or twice within a second, sends `Esc` once: a second `Esc` after the agent stopped would open Claude Code's rewind menu.
- **Exit to shell** quits the agent and leaves the pane at a shell prompt, after asking.
- **End session…** closes the pane, its window or the whole tmux session. The confirmation says exactly which, and the session disappears at once.
- **Copy session ID** copies the agent's conversation ID, for resuming it elsewhere.
- **Past conversations…** (**History** on a phone) lists the agent's earlier conversations in this folder and switches to one. See [Past conversations](sessions.md#past-conversations).
- **Fork conversation** (**Fork** on a phone), from 0.17, opens a copy of the conversation in a new tmux window next to this one, in the same folder and with the same flags, and takes you there. From then on the two carry on separately, so you can try another approach without losing this one. It works for Claude Code, Codex and OpenCode once the conversation has a first message; Inyon starts the agent with its own fork option (`claude --resume <id> --fork-session`, `codex fork <id>`, `opencode --session <id> --fork`).

## Background work

The layers button in the header (or the side panel on a wide screen) shows what's running beside the conversation:

- **Running:** queued messages, subagents with their transcripts, background shells with live output, and monitors. Shells and monitors each have a real **Stop**. Codex's sub-agents are listed here too from 0.17.6, by the name Codex gave them, from the moment they start until they report back; what they said back shows in the chat.
- **Changes:** what changed, two ways. **This chat** lists every file the agent changed with its edit tools in this conversation, with the diff of each edit, newest first. Changes it made by running commands (a formatter, `sed`, a script) aren't there; they show under Working tree. **Working tree** (from 0.17) shows what git sees: every change since the last commit, whoever made it. See [Reviewing changes](#reviewing-changes).
- **Files** (from 0.17): the session's folder, read only. Open a folder, read a file with syntax highlighting, see an image, or read a Markdown file rendered (**Source** shows the text). **Mention** adds `@path` to your message. Files over 512 KB show their first 512 KB.
- **Preview:** a dev server the agent started, in a frame, with a phone-width toggle. Inyon suggests the local ports it finds.
- **Artifacts** (Claude Code): pages the agent published, with **Open**, **Copy link**, and a button that asks the agent to delete one.
- **Pages** (from 0.22.3, when the session has any): the [pages](pages.md) in the session's project, with their progress. Tap one to open it.

## Reviewing changes

From 0.17, **Changes → Working tree** reads the git repository the session's folder is in: the branch, how many commits it is ahead of or behind its upstream, and each changed file marked **M** (modified), **A** (added), **D** (deleted), **R** (renamed) or **U** (not tracked yet), with line counts. Tap a file for its diff. It refreshes when the agent does something, or with the refresh button.

- **Commit all** commits every change shown, with your message, as `git add -A` and `git commit` would. Your git hooks and commit signing run as usual; one that needs a password typed in the terminal fails with git's message instead of waiting.
- **Push** appears when there's a remote and something to push. A branch with no upstream yet goes to `origin` under the same name.

To comment on a line, tap its number in any diff, in either view. Comments collect under the diff; **Add to message** puts them all in your message box as one review, each under the file, line and code it's about, so you can add to it before sending. **Clear** drops them. Comments stay in the app until you add or clear them, and aren't saved anywhere.

## Usage and limits

The ring in the header shows how full the context window is; it turns amber past 60% and red past 85%. Tap it, or open **Usage** in the side panel, for:

- the context window, with **Compact now**;
- the plan and its limits: the 5-hour window and the week, with when each resets, plus Codex credits;
- the session's cost (Claude Code and OpenCode), run time and turns, and lines changed;
- the model, effort, fast mode, output style, the agent's version, the git branch, the folder, the session ID, and OpenCode's shared link while there is one;
- how Inyon matched the pane to its conversation, from an exact hook to a best guess by folder.

For Claude Code, the live numbers come from its status line, through Inyon's optional integration (turn it on from the machine's **Integration** tab). If a session stops reporting numbers, a note offers to reconnect or restart it. Settings → **Restart sessions that stop reporting** does that by itself.
