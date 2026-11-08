# Devin conversations

On a connected Unix machine, open **Overview → machine → Devin conversations**
or **Machines → machine → Devin conversations**. The desktop sidebar also has
**Devin conversations** under each machine. Both entries remain available when
no CLI is running; Overview's Working/Idle filters do not hide saved history.
Every connected machine with Devin installed gets its own library and handoff,
including multiple Macs in the same Hub.
Python 3, tmux and Devin CLI must be installed on that machine.
Existing Devin terminal links and their Chat tab also open the saved conversation,
identified by its native writer lock rather than its title or working directory.

The list reads the native Devin session database, including Desktop App sessions,
without creating or resuming an agent. History follows the selected message
branch and explicit compaction references. Only displayable conversation fields
are sent to the browser. The database is opened read-only.

When sending, Inyon finds the process holding that exact session's native lock.
If the process belongs to an existing tmux pane, that CLI is reused, including
manually started CLIs and sessions left from an earlier Inyon run. Directory and
title matching are never used to choose a conversation.

Otherwise Inyon starts `devin --resume <session-id>` in a visible tmux session
named `devin-<conversation title>` (Unicode, spaces and parentheses are preserved;
duplicate titles get a short ID suffix). The supported CLI currently offers **Force open**
as an interactive choice rather than a global `--force` flag. Inyon selects that
choice only after validating the displayed session and owner PID and checking
that the previous turn has finished. Native Force open terminates the former
Agent process, so other conversations held by that process must also be idle.
Unknown status, unfinished turns, questions, approvals and existing input drafts
block new messages. Complete those interactions in Devin or its terminal.

The CLI stays running in tmux. The Chat header shows its name and links to its
terminal. Exit it manually when returning to the App; Inyon observes release of
the native lock and returns to reading saved history. Reopen the conversation in
the App to reload its latest messages. Avoid sending from App and CLI together.

The chat shares the existing message, header and composer presentation components
with Codex; unsupported controls are omitted. Saved messages update incrementally. While a CLI is generating, a bounded **Live
terminal preview** shows its changing screen; the completed native history is
the canonical chat. Reading an App-only session shows updates as Devin saves
them and does not promise token-by-token App streaming. Long messages are folded
for rendering, while copying preserves their full text. Earlier messages load
into the view in groups of 80.

Only reading and plain-text sending are supported here. Answer questions and
approvals in the native terminal. Slash commands, shell commands, rewind, clear,
stop, attachments and model changes are not Chat controls in this first version.
Sends use a per-machine dispatch lock and persistent request receipts; an
uncertain delivery is not automatically submitted again. The reader never
changes App startup or ACP configuration.
