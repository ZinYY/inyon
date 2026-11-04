# Claude conversation forks

Ordinary forks use Claude Code's native `--resume <id> --fork-session`. After a confirmed conversation rewind, Inyon uses Anthropic's public `forkSession(sessionId, { upToMessageId })` API to create a new session at the exact selected branch, then opens that new ID in the terminal. The source session and its input draft are left untouched. Inyon does not rewrite or construct Claude transcript files.

The SDK branch copies conversation history but does **not** inherit file rollback checkpoints from before the fork. New file checkpoints made within the fork still belong to that fork. See the [official SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript#forksession) and [session browser example](https://platform.claude.com/cookbook/claude-agent-sdk-05-building-a-session-browser).

On each machine that runs Claude, install Node.js >= 18.14 and npm, then run `python3 scripts/install-claude-sdk.py` (`python` on Windows) from the Inyon repository. This installs the official SDK at the pinned version in `~/.inyon/claude-sdk`; it does not replace Claude Code, change API settings, or require a Claude sign-in. The optional SDK agent binaries are omitted because session forking is a local file-management API. If the SDK is missing or fails, Inyon reports the error and does not fall back to a fork containing retracted history.
