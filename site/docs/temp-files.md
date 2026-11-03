---
title: Agent temp files
nav: Temp files
description: See the pasted images, screenshots and scratch folders your coding agents leave on each computer, preview them, and delete what you no longer need.
---

Coding agents leave files behind as they work. Claude Code keeps every pasted image, its scratchpads and the output of background tasks in a temp folder for each session, and over a few weeks that can grow to gigabytes. From 0.21 the **Temp files** screen lists them for each computer, lets you look inside, and deletes what you pick.

Open it from a machine's page (the **Temp files** tab), from a machine's menu (right-click, or long-press on a phone), or from Settings → **Uploads and clipboard** → **Agent temp files**.

## What's listed

Only folders that hold throwaway files. Each one is shown with its total size, and its contents as items: one per session folder, one per day of uploads, one per loose file.

| Folder | What's in it |
|---|---|
| Claude Code temp files | `$TMPDIR/claude-<uid>` on Linux and macOS (`/tmp/claude-<uid>` when `TMPDIR` isn't set), `%TEMP%\claude-0` on Windows, or `claude-<uid>` inside `CLAUDE_CODE_TMPDIR` when that is set. One item per session, named after its project, such as `custom-tmux · 7409eef9`, with its pasted images, scratchpad and task output |
| Claude Code pasted text | `~/.claude/paste-cache` |
| Claude Code shell snapshots | `~/.claude/shell-snapshots` |
| Inyon uploads | `~/.inyon/uploads`, one item per day (see [Settings](settings.md#uploads-and-clipboard) for how long they're kept) |
| Gemini CLI temp files | `~/.gemini/tmp`, except each project's saved chats, `logs.json`, shell history and checkpoints |
| Codex shell snapshots | `~/.codex/shell_snapshots` |

Transcripts and history are never listed, so deleting here can't lose a conversation: `~/.claude/projects`, `~/.codex/sessions`, file history and Gemini CLI's saved chats stay out of reach.

Each item shows its size, how long ago it changed and up to four of its newest images. An item marked **recent** changed in the last 30 minutes, so an agent may still be using it. A folder Inyon couldn't read all the way through (one it isn't allowed into, or nested more than 64 folders deep) is marked recent too, since what it couldn't see may have changed.

## Look inside

Tap an item to see its files, newest first, with image thumbnails. Tap a file to preview it: images at full size, and text, Markdown, code and logs as plain text. HTML is shown as its source, never run. Text stops after the first 256 KB, and images over 8 MB aren't sent to the app.

## Delete

Tick items (shift-click picks a range on a desktop), or use **Select all** or **Older than a week**, which leaves recent items out. **Delete** says how much space it frees and warns when any item is recent.

> **Warning:** Deleting is permanent. There's no undo and no bin.

The computer checks every path itself before deleting it: it must be inside one of the folders above, not the folder itself, with no `..` in it. A symlink is deleted as a link, and a path that leads out of these folders through one is refused. Whatever stays is listed with the reason.

## From the terminal

The same list is `inyon temp` on each computer. See [the inyon command](cli.md#temp-files).

## When it doesn't open

- **Update Inyon on … to see its temp files:** that computer runs a version from before 0.21. Update it from its page, or run `inyon update` there.
- **Install Inyon on … to see its temp files:** a server added over SSH has no `inyon` on its `PATH` or in `~/.inyon/bin`. The screen asks that computer's own `inyon` for the list, so install it there (see [Install](install.md)).
