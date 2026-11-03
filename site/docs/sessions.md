---
title: Sessions
description: Reading a session as a chat, answering prompts, and writing messages with commands, files, images and shell commands.
---

Tap a session to open it. If an agent is running in the pane, it opens as a chat; otherwise it opens the [terminal](terminal.md). The **Chat · Terminal** switch in the header moves between the two at any time.

![A desktop browser showing a Claude Code session's terminal, with the sidebar of machines and sessions on the left and the edited file's diff on the right](../../docs/terminal-light.webp)

## The chat

The chat is the agent's own record of the conversation, read from its session files, not a copy of the screen. It shows:

- **Replies**, formatted: headings, lists, tables and code with syntax highlighting and a copy button. With Codex, from 0.17.6, a reply appears as it is written and settles into place when it's complete.
- **Tool calls**, one line each with a verb (*Ran*, *Read*, *Edited*, *Searched the web*…), the target, `+`/`−` line counts and how long it took. Tap one to expand it: a command and its output, an edit as a diff, a file read, or the raw input and result. A Codex script that ran several tools reads *Ran a script* with each step, its diffs and the script itself behind **Script**.
- **Plans, to-dos and questions** as cards: a proposed plan and whether it was approved, a checklist, or the questions the agent asked and your answers. Codex's plans (Plan mode) and questions are cards too, from 0.17.6.
- **Thinking**, collapsed.
- **Goals**, as *Goal set*, *Goal met* or *Goal cleared*.
- **Notices**: compaction, Remote Control turned on, and a *While you were away* recap when you come back.

Settings → Appearance → **Tool calls in chat** switches between *Everything* and *Compact*. Compact folds a long run of tool calls down to the last four, with the rest behind *N more steps*.

### Following along

The chat follows new replies as they arrive, unless you've scrolled up to read; then it keeps your place and offers **Jump to latest**. Scroll up to load earlier messages. If you've been away for more than five minutes, it opens at the newest reply.

While the agent works, a line under the last message says what it's doing; for Codex it counts how long the turn has run and, from 0.17.6, shows the last line a running command printed. Background work (shells, monitors, subagents) shows under it too, and stays after the reply until it's done; see [Background work](controls.md#background-work).

### Images

Images the agent shows inline are drawn only when they're local: pasted, uploaded or from the same machine. A remote image link stays a link, so a page can't learn what you're reading by loading an image.

## Answering prompts

When the agent asks something, the composer is replaced by the question:

- **Options** become numbered buttons. Tap one to answer. Prompts that allow several answers show checkboxes and a **Next →** button.
- **Free text** ("Type your answer…") opens a text box.
- **Links**, such as a sign-in page, get **Open** and **Copy link**.
- **A pasted value**, such as a sign-in code, gets a box and **Submit**.
- **Several questions** at once show a strip of tabs, marking which are done.
- **Choices with previews.** When Claude Code's question gives each choice a description or a preview (a small mockup), the question's card in the chat shows all of them, and the buttons under it pick as usual. The terminal shows only the highlighted choice's preview. From 0.22.5.

**Cancel** sends `Esc`, as you would at the keyboard. If Inyon can't read the options on a screen it doesn't recognise, it shows `↑`, `↓` and `Enter` buttons instead.

Each answer is tied to the question it was shown for. If the agent has already moved on, an old card can't answer the wrong question.

From 0.22 a question that one line can't hold, such as a command of several lines, a diff or an OpenCode permission, is answered only here, where it shows whole. Companion apps get no quick answers for it, and while its machine reconnects the session doesn't show it from memory.

Codex can ask a question without stopping its turn: the terminal shows *1 question* with a hint to press `shift+←`. From 0.17.6, Inyon presses it for you as soon as nothing is being typed in the terminal, so the question shows here with its choices, and the session reads *A question is waiting for your answer* on the overview until you pick one. Codex's approvals say what they're for in the alert and on the overview: *Run `printf 'hi' > hello.txt`*, or *Edit notes.txt?*

If the app has just woken up and is still reconnecting, the chat says *Reconnecting to* the machine, and a question you saw on the overview stays on screen with its buttons greyed out until the machine answers.

You can also answer straight from the [overview](overview.md#whats-on-it) or from a notification, without opening the session.

## Writing messages

Type in the box at the bottom and press `Enter` to send. `Shift Enter` adds a line. On a phone, `Enter` adds a line and the send button sends.

While the agent is working, sending queues the message instead: the box says *Queue a message for Claude…*, and the agent reads it at its next step. With nothing typed, the button becomes **Stop**, which sends `Esc`.

Codex takes a message two ways while it works, and from 0.17.6 a switch next to the send button picks one. **Queue** (the default, `Tab` in the terminal) holds the message for after this turn; it shows as a queued bubble until Codex takes it. **Steer now** (`Enter` in the terminal) hands it to Codex in the middle of the turn, so it changes course without finishing first. The switch goes back to Queue when Codex stops working.

### Commands and skills

Type `/` for the agent's commands, skills and plugins, fetched from the machine each time, with where each comes from (project, user, plugin). Keep typing to filter, then `↑` `↓` and `Enter` or `Tab` to pick.

A few commands open Inyon's own controls instead of being typed: `/model`, `/models`, `/effort`, for OpenCode `/agents`, `/variants` and `/connect` (its providers), and the agent's resume command, which opens [Past conversations](#past-conversations). Commands that only change a setting (model, effort, mode, goal…) don't clutter the chat. In Codex, `$` lists skills.

### Files with @

Type `@` and part of a path to search the session's folder. Picking a file inserts its path; picking a folder keeps the list open inside it.

### Shell commands with !

Start a message with `!` to run a shell command in the session's folder, through the agent: Claude Code, Codex, OpenCode and Copilot. The agent runs it and handles permissions, and the output shows in the chat.

### Images and files

Attach with the paperclip, take a photo on a phone, paste a screenshot, or drag files onto the chat. Files upload to the machine (into `~/.inyon/uploads`) with progress, and the message lists their paths for the agent. A failed upload can be retried, and you can't send until it's either done or removed.

From 0.24 a file goes up a megabyte at a time, from a browser and from the phone app, to any machine. If the connection drops, the attachment says *Reconnecting…* and carries on from the last megabyte that arrived once it's back, for up to about ten minutes. After that it shows as failed, and tapping it to try again still picks up where it stopped. Inyon checks the file's size and SHA-256 before it takes its name, so an agent never gets a file that is cut short or damaged. Removing an attachment while it uploads stops it and deletes what arrived. A hub older than 0.24 still takes files the old way.

Pasting more than 40 lines of text attaches it as a file instead (*Pasted text · 120 lines*), so long logs don't flood the box. On a computer, typing anywhere on the page goes into the box, and pasting anywhere attaches.

Settings → **Uploads and clipboard** sets how long uploads are kept (1, 7 or 30 days, or forever), the largest file allowed and, from 0.17, how much space they may take on each machine. See [Settings](settings.md#uploads-and-clipboard).

### Drafts follow you

What you type is saved as you go, and mirrored into the agent's own input box in the terminal. Start a message on your phone and finish it at the keyboard, or the other way round. A draft picked up from elsewhere is marked *From the terminal*.

### Message states

A message shows **Sending**, then **Sent** once the agent has it. If it can't be delivered, it says so, with **Retry** and **Dismiss**; messages that failed because the connection dropped are retried by themselves when it comes back. A message that hasn't appeared in the conversation after ten minutes says so.

Inyon presses Enter only once your message shows in the agent's input box. For Claude Code, Codex, Antigravity, Copilot and OpenCode it reads the box itself; for any other agent, such as Gemini CLI, it waits until the end of your message (or a paste preview) appears on the screen, and if it doesn't, nothing is submitted and the message says so. From 0.22 the hub keeps the last 20 failed sends with the bottom of the screen each failed on, without the message itself: right-click (or long-press) a message that wasn't sent and choose **What was on screen**. Before 0.22 Gemini CLI got Enter 0.3 seconds after the text was typed, whether or not it had arrived.

### Holding a message until the agent is free

From 0.22 the hub can hold a message for you and type it in once the agent has been idle for a moment, with no question open and nothing running in the background. Right-click (or long-press) the send button and choose **Hold until … is free**, or press Alt+Enter (⌥Enter on a Mac). Held messages show under the conversation with a dashed outline, in the order they go. Each has **Send now** and **Drop** beside it, and its menu also has **Edit**, **Move up** and **Move down**; dropping one can be undone. Unlike a message the agent queues itself, a held message can still be changed from any device, and it survives a restart of the hub.

If a held message can't be sent, it stays first in line with the reason, and nothing more goes until you edit it, send it now or drop it. Up to 50 messages can be held per session. The phone app holds messages only through a hub of 0.22 or later.

### Sending a message later

From 0.24 the hub can keep a message and send it at a time you pick. Right-click (or long-press) the send button, choose **Send later…**, then **In 30 minutes**, **In an hour**, **In three hours** or **Tomorrow morning** (9:00 by the clock of the device you're on). The message waits under the conversation with a dashed outline and *Sends at …*, with **Send now** and **Cancel** beside it, on every device. When its time comes it joins the held messages and goes once the agent is free, so it never cuts into a turn. It goes once, even if the hub restarts around that time.

A message sent later follows its conversation if you resume it in another pane, and shows in that pane's chat. If its machine is offline when the time comes, it waits until the machine is back. If the conversation is gone by then, it stays with *The session isn't there any more* until you dismiss it; it never goes to another conversation that took over its pane. **Send now** takes it off the schedule and hands it to the held messages in one step, and **Undo** after **Cancel** puts it back for the same conversation. Times more than six days off show their date. Up to 50 messages can wait at once, each up to 4,000 characters.

Newer Claude Code versions show *← 1 agent* in their footer, a count of the other Claude Code sessions on the machine. Until 0.22.3 Inyon read it as this session's background work, so the session never looked free and held messages waited for good. From 0.22.3 that count is ignored.

## Renaming

Click or tap the title in the header to rename a session (up to 80 characters), or choose **Use automatic name** to go back. The name stays with the conversation, even if you resume it later. Renaming a plain terminal renames its tmux window.

From 0.22 you can rename the tmux session itself, on any machine (Linux, macOS or Windows): right-click its heading in the sidebar, or choose **Rename tmux session…** from a session's menu. tmux shows the new name too, in `tmux ls` and its status bar. tmux doesn't allow a dot or a colon in a session's name, so those become underscores, and a `#` becomes a dash. Two sessions on one machine can't share a name.

## Pins, tags and notes

From 0.24 a session's menu on the overview, in the sidebar or on a machine's page also has **Pin**, **Park**, **Snooze…**, **Tags…**, **Note…** and **Close when done**, which ends the session (as **End session…** does) once its agent finishes its next piece of work. The hub keeps them, so every device shows the same, and they follow the conversation into another pane when you resume it. See [The overview](overview.md#pinning-parking-and-snoozing).

## Past conversations

From 0.15, Inyon lists an agent's past conversations itself, since its own resume picker only works in the terminal. Open the list with **Past conversations…** in the header's **⋯** menu, with **History** in the controls sheet on a phone, or by sending the agent's resume command from the message box:

| Agent | Command |
|---|---|
| Claude Code, Codex | `/resume` |
| GitHub Copilot, Antigravity CLI | `/resume` or `/switch` |
| OpenCode | `/sessions` |

The list holds the conversations the agent saved in the session's folder, newest first, up to 50. Each shows its title and when it was last active: the agent's own title, a name you gave it, or else your first message. The one open now is marked *Open now*.

Tap one to switch to it. The agent exits the way `/exit` would, so the current conversation is saved and can be resumed later, then starts again on the one you picked, in the same pane. The chat loads it once the agent is back. If the agent is working, Inyon asks first.

A switch stops with a message instead when:

- shells or monitors are running in the background, since exiting would stop them;
- the agent doesn't exit (close it from the terminal);
- another switch in the same session is still under way.

The list works for every machine, whether you reach it through your hub, a paired hub or straight from the phone. A paired hub needs to be up to date. Gemini CLI isn't supported: resume its conversations in the terminal.

## When nothing is running

If the agent has exited, the session offers **Resume conversation**, **Past conversations** (to pick another one), **Start** for each installed agent, or **Terminal**. A pane that has closed says *This pane is gone*.

## Signing in

If an agent isn't signed in on that machine, the session shows a banner with **Sign in**. Inyon runs the agent's own sign-in and turns it into steps: a code to copy, a page to open, or a code to paste back. See [Setting up a machine](machines.md#the-setup-tab).
