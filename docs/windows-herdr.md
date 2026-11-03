# Native Windows Herdr sessions over SSH

For an existing Herdr installation on native Windows, select **Windows / Herdr**
when adding or editing the SSH machine. Keep the SSH alias that works from the hub
(for example `qian-windows`) so its user, port, identity and jump configuration apply.
The default Linux / macOS mode sends POSIX shell syntax, which PowerShell cannot parse.

Requirements: Python 3 on the SSH user's PATH, Herdr 0.9.5 with its existing agent
integrations, and BusyBox. Run `scripts/setup-windows-herdr.ps1` once as that user.
It installs the same checksum-pinned BusyBox used by Inyon's Windows engine. No
SSH configuration, firewall rule, Herdr access policy or Claude hook is changed.

The hub carries the Python bridge in its binary. Every command sends a bounded,
length-prefixed frame over SSH stdin. Commands and pasted text never appear in the
SSH command line. A versioned helper is cached under `~/.inyon/bridges` for child
commands; updating the hub updates the bridge without rebuilding Windows binaries.

The bridge lists all running named Herdr sessions and workspaces. It maps Claude's
exact `agent_session` ID to its JSONL transcript, and reads the transcript through
the same lossless reader as other machines. Ambiguous or missing IDs do not fall
back to a different conversation. The local Herdr named pipe is the only control
interface; there is no new network listener.

Terminal uses Inyon's Windows mirror and forwards text/keys to the selected Herdr
pane. It preserves Herdr's layout and viewport size. Herdr's public read API does
not expose the cursor position, so the mirror hides the cursor. Screens refresh
on changes detected by a 750 ms poll, in addition to Inyon's normal refresh.

Existing Claude chats and terminal input are supported. Session creation, process
restart, rewind and other tmux management actions are not provided by this bridge;
manage those in Herdr. Unsupported terminal commands return an explicit error.
Live account limits and status-line cost are not available through this connection.
Herdr's hooks remain in charge of session identity; Inyon does not replace them.

Named-pipe connection attempts wait up to 5 seconds when Herdr is busy. Requests
are never replayed after being written, so a retry cannot duplicate a submitted
message. Terminal input failures are displayed and require reconnecting before
more keys are sent. Chat displays completed transcript blocks; screen-based
streaming previews are disabled for Windows / Herdr.
