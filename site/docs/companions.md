---
title: Companion apps
description: Pair an app like Legato with your hub so it can show your sessions, answer an agent's question and send it text, and nothing more.
nav: Companion apps
---

From 0.20 a companion app can pair with your hub. Legato, a speech-to-text app for Windows, is the first: it shows your sessions, answers an agent's question and sends a dictated instruction to a session. A companion gets four permissions and the hub refuses everything else, so it can't open a terminal, read files or change settings.

The first half of this page is for pairing one. The second half is the contract, version 1, with the exact requests and answers, for anyone writing a companion.

## Pair a companion app

1. In Inyon, open **Settings › Paired devices** and press **Pair a companion app**. If you signed in more than 10 minutes ago, Inyon asks for the password again first.
2. Type the six-character code and the hub's address into the companion app. In Legato that's **Settings › Connections › Inyon**.
3. The sheet says when the app is paired and lists what it may do.

The code works once, for 5 minutes, and the sheet makes a new one when it runs out. **New code** ends the old one straight away.

From 0.20.2 **Copy pairing link** copies the hub's address, the code and, when the hub serves HTTPS itself, its certificate's fingerprint as one link, for an app that takes a pasted link instead of two boxes:

```text
inyon://companion?u=https%3A%2F%2Fhub.example%3A4777&c=K7M2QX&f=5e0c…64 hex digits
```

On the hub's computer you can make a code without a browser:

```sh
inyon hub companion-code
# Companion code: K7M-2QX
# Type it into the companion app (Legato: Settings > Connections > Inyon) with this hub's address, http://100.64.0.5:4777. It works once, for 5 minutes.
```

A hub that listens only on `127.0.0.1`, behind a proxy or `tailscale serve`, doesn't know its outside address, so the command then says to use the address you open the hub at. With `--json` it prints `code`, `expiresAt`, `url` and `message`, and from 0.20.2 `tlsFingerprint` when the hub serves HTTPS itself.

## What a companion can do

| Permission | What it gives the app |
|---|---|
| Overview | every session with its state and machine, live changes, and alerts |
| Chats | the screen and the details of a session it watches |
| Answer | an answer to an agent's question, once, for the question it saw |
| Send | plain text typed into a session; the hub says whether it arrived |

Interrupting, keys, the terminal, uploads, files, model and mode switches, past conversations and every setting are refused, on every route and every WebSocket message. The check is on the hub, so a companion that misbehaves gets the same refusal as one that doesn't. **Send** and typed answers take printable text only. Tabs and line breaks are fine; control characters and escape sequences are refused, and so is any line that starts with `/` or `!`, so a companion can't run an agent's commands or a shell command itself. It can still ask the agent to do something, and the agent does it under its own permission mode, as it would a message typed in the terminal. **Answer** approves or declines what the agent asks, so pair only an app you trust with that.

**Settings › Paired devices** lists each companion with its name, app, permissions, contract version, when it paired and when it was last seen. **What it can do** in its menu spells out the permissions. **Remove** ends it at once: its token stops working and its open connection closes. To use it again, pair it with a new code.

## The contract, version 1

Everything below is version 1. Changes within a version only add things: a new field, a new frame, a new `reason`. Ignore what you don't know.

Version 1.1, from 0.20.2, adds `tlsFingerprint` to the claim and the code, the pairing link, and `"reason": "revoked"` on a token the hub doesn't know. The header still says `1`.

### Every request

Every request after pairing carries the token and the contract version:

```http
Authorization: Bearer <token>
X-Inyon-Contract: 1
```

The claim carries `X-Inyon-Contract` too. It needs no token, and one sent along with it is ignored, so an app can pair again before it drops the old token. Without the header, or with a version the hub doesn't speak, the answer is `426`:

```json
{ "error": "Send the X-Inyon-Contract header. This hub supports companion contract 1.", "reason": "contract", "supported": [1] }
```

```json
{ "error": "This hub doesn't support companion contract 2. It supports 1.", "reason": "contract", "supported": [1] }
```

A token the hub doesn't know, or one that was removed, gets `401 {"error":"Not signed in","reason":"revoked"}` (hubs before 0.20.2 leave out the `reason`). Forget the token and pair again only on that answer: a `401` or `403` without it came from something in between, such as a Wi-Fi sign-in page or a proxy.

Errors are JSON with an `error` written for people, which you can show as it is, and a `reason` to act on:

| `reason` | Means |
|---|---|
| `contract` | the `X-Inyon-Contract` header is missing or names a version the hub doesn't speak |
| `revoked` | the hub doesn't know the token, or it was removed; pair again |
| `scope` | companions can't do this |
| `bad-code` | the pairing code is wrong, used or expired |
| `rate-limit` | too many wrong codes; wait a minute |
| `bad-request` | the request body is missing something; `error` says what |
| `bad-option` | the question has no option with that number |
| `already-answered` | the question was answered or is no longer on screen |
| `pane-gone` | the session isn't there any more |
| `offline` | the session's machine isn't reachable right now |
| `no-agent` | no agent runs in that session |
| `prompt-open` | the agent is asking a question; answer it first |
| `busy` | the agent's input box isn't there to type into right now |
| `not-landed` | the hub typed the text but couldn't confirm it arrived |

### Claim the code

```http
POST /api/companions/claim
X-Inyon-Contract: 1
Content-Type: application/json

{ "code": "k7m-2qx", "name": "Legato on abhash-laptop", "app": "Legato for Windows" }
```

The code may come in any case, with or without a space or dash. `name` is what Inyon lists, up to 60 characters; `app` is optional.

```json
{ "token": "Jb2y…43 characters", "hubId": "3f9c1a7e2b6d4c80", "scopes": ["overview", "chat", "answer", "send"], "contractVersion": 1 }
```

When the hub serves HTTPS itself, the answer also has `tlsFingerprint`: the SHA-256 of its certificate (the leaf, in DER form) as 64 lowercase hex digits, the same value as `f` in the pairing link. Pin it, and refuse the hub if the certificate ever changes without a new pairing. It's left out on plain http and behind a proxy or `tailscale serve`, where the hub doesn't hold the certificate.

```json
{ "token": "Jb2y…43 characters", "hubId": "3f9c1a7e2b6d4c80", "scopes": ["overview", "chat", "answer", "send"], "contractVersion": 1, "tlsFingerprint": "5e0c…64 hex digits" }
```

Keep the token somewhere safe; the hub keeps only its hash. A wrong, used or expired code gets `401`:

```json
{ "error": "That code is not right or has expired. Make a new one in Inyon.", "reason": "bad-code" }
```

Eight wrong codes in a minute from one address, or thirty from all of them, get `429` with `"reason": "rate-limit"` until the minute is up.

`GET /api/companions/me` says who you are, for checking a saved token:

```json
{ "id": "a1b2c3d4e5f6", "name": "Legato on abhash-laptop", "app": "Legato for Windows", "scopes": ["overview", "chat", "answer", "send"], "contractVersion": 1, "hub": { "id": "3f9c1a7e2b6d4c80", "name": "abhash-arm", "version": "0.20.0" } }
```

### The overview

`GET /api/overview` returns `{ "hosts": [...] }`, a cut-down form of the app's overview (the `HostState` type in [packages/protocol](https://github.com/Abhash-Chakraborty/inyon/blob/main/packages/protocol/src/index.ts)) with only what it takes to list sessions and label them:

| Level | Fields |
|---|---|
| machine | `config` with only `id`, `name` and `kind`, `status` (`online`, `connecting` or `error`), `sessions` |
| session | `id`, `name` (the tmux session's name), `windows` |
| window | `id`, `index`, `name`, `active`, `panes` |
| pane | `id`, `index`, `active`, `title`, `label` (the name you gave it in Inyon), `dead`, `agent`, and from 0.22 `companion` |

A companion never sees how the hub reaches a machine, folders, transcript paths, session or process ids, or the agent's usage numbers. A pane running an agent has an `agent` with:

| Field | Holds |
|---|---|
| `kind` | `claude`, `codex`, `opencode`, `copilot`, `gemini` or `antigravity`; from 0.23, with `catalog=2` (see below), also `cursor`, `grok`, `pi`, `amp`, `goose`, `hermes`, `kimi`, `kiro`, `qwen`, `qoder` or `codebuddy` |
| `custom` | from 0.22, the name of [your own agent](agents.md#your-own-agents) when the pane runs one; `kind` is then the agent it works like |
| `state` | `idle`, `working`, `waiting` or `unknown` |
| `waitingFor` | a short line about the question, while `state` is `waiting` |
| `options` | the answers on offer, `[{ "n": 1, "label": "Yes" }, …]`, for a single-choice question. From 0.22 it's left out when `waitingFor` doesn't hold the whole question: a command or diff of more than one line, a Codex edit, an OpenCode permission, or a line cut at 200 characters. Answer those from `chat.screen`, which has the whole prompt |
| `activity` | the working line, such as `Undulating… (2s · thinking with high effort)` |
| `mode` | the agent's mode, such as `manual` or `plan` |

A pane is named by its machine's `config.id` and its tmux pane id, such as `box` and `%3`.

From 0.23 the hub knows more agents than the first six. A companion that can show an agent kind it doesn't know (with a plain icon, say) adds `catalog=2` to the URL of each request and of the WebSocket, or sends the header `X-Inyon-Catalog: 2`. Without it, a pane running one of the newer agents comes without `agent`, as a plain terminal, and its notices without `agent` either, so a companion written for the six never meets a kind it can't handle.

From 0.22 the pane running the [companion agent](companion-agent.md) has `"companion": true`. At most one pane has it, across all the hub's machines; send your user's words to that pane to talk to the companion.

### The WebSocket

Open `/ws` with the same two headers. A client that can't set `X-Inyon-Contract` on the upgrade can add `?contract=1` to the URL instead; the token still goes in the `Authorization` header. A missing or unsupported version is refused with `426` and the same JSON as above.

A companion may send:

| Frame | Does |
|---|---|
| `{"t":"ping"}` | the hub answers `{"t":"pong"}` |
| `{"t":"sub.overview"}` | an `overview` frame now and after every change |
| `{"t":"sub.chat","host":"box","pane":"%3"}` | `chat.info` and `chat.screen` for that pane now and on every change |
| `{"t":"unsub.chat","host":"box","pane":"%3"}` | stops them |

Anything else closes the socket with code `4403` and a reason naming the frame, such as `Not for companion apps: term.open`. When you're removed in Inyon, the socket closes with `4001` and `Companion removed`.

It receives `overview`, `chat.info`, `chat.screen`, `notify`, `companion.reply` and `pong`, and never the chat history, live text, drafts, the queue, the text of sends, Remote Control or share links, or terminal data. `chat.info` is `{ "t": "chat.info", "key": "box:%3", "info": { … } }` with only `hostId`, `paneId`, `agent`, `title`, `model`, `mode`, `running`, `status` (the agent's live numbers), `turn` (when the turn under way started) and `sends`, each send as `{ "id": "companion-legato-7f3a", "state": "delivered", "at": 1792038540000 }` without its text. The title is the name you gave the conversation, else the agent's own title for it, else the terminal's title, which Claude Code sets to the task and tmux otherwise sets to the machine's name. The tmux session's name is `name` in the overview's `sessions`. Alerts look like this, with `kind` one of `waiting`, `done`, `offline` and `online`:

```json
{ "t": "notify", "host": "box", "pane": "%3", "kind": "waiting", "title": "Claude needs you", "body": "Bash: rm -rf build", "agent": "claude" }
```

From 0.22, with the **chat** permission, each turn the companion agent finishes comes as a `companion.reply`, with no `sub.chat` needed:

```json
{ "t": "companion.reply", "id": "1a16fa3b4de-2", "host": "local", "pane": "%1", "text": "Two sessions need you. Codex on laptop wants to run the tests.", "at": 1792038540000 }
```

`text` is the reply as plain text, markdown taken out and line breaks kept, cut at a word near 2,000 characters. `id` is different for every reply, so a reply that comes twice can be shown once. `"error": true` marks a turn that stopped on an error, and `text` then says why. The companion's own questions, such as asking to run `inyon send`, come the usual way: `waiting` in the overview and a `notify`. On the WebSocket the hub sends no `done` notice for the companion, since `companion.reply` takes its place. Web Push still gets one, titled "Companion", with the start of the reply as its text.

### A question and its key

A companion's `chat.screen` has only `working`, `activity`, `since`, `inputVisible`, `mode`, `model`, `effort`, `dialog` and `width`, never what's typed in the box. While an agent asks something, it carries the question as `screen.prompt`, and the hub adds the question's `key`:

```json
{
  "t": "chat.screen",
  "key": "box:%3",
  "screen": {
    "working": false,
    "inputVisible": true,
    "mode": "manual",
    "width": 100,
    "prompt": {
      "body": ["Bash command", "rm -rf build", "Do you want to proceed?"],
      "options": [
        { "n": 1, "label": "Yes", "selected": true },
        { "n": 2, "label": "Yes, and don't ask again for rm commands in this project", "selected": false },
        { "n": 3, "label": "No", "selected": false }
      ],
      "multi": false,
      "footer": "Esc to cancel · Tab to amend",
      "key": "Bashcommandrm-rfbuildDoyouwanttoproceed?1.Yes2.Yes,anddon'taskagainforrmcommandsinthisproject3.No"
    }
  }
}
```

Treat the key as an opaque string and send it back as it came. Answer with the option's `n` and the key, so a late tap never lands on the next question. In the path a pane id goes without its `%`, so `%3` is `3`:

```http
POST /api/hosts/box/panes/3/answer

{ "option": 1, "key": "Bashcommandrm-rfbuildDoyouwanttoproceed?1.Yes2.Yes,anddon'taskagainforrmcommandsinthisproject3.No" }
```

| Status | Body | When |
|---|---|---|
| 200 | `{"ok":true}` | the agent took the answer |
| 409 | `"reason": "already-answered"` | that question was answered (in the app, on a phone, in the terminal) or is gone |
| 409 | `"reason": "pane-gone"` or `"offline"` | the session or its machine isn't there |
| 400 | `"reason": "bad-option"` | the question has no option with that number |
| 400 | `"reason": "bad-request"` | no `key`, an `option` that isn't a number, or `text` that isn't allowed |
| 403 | `"reason": "scope"` | `text` without the **Send** permission, or a line of it that starts with `/` or `!` |

`text` goes only with the question's free-text choice, the option numbered `prompt.freeTextOption` (Claude's "No, and tell Claude what to do differently", say). It needs the **Send** permission and follows Send's rules. With any other option it gets `400`.

### Send text

```http
POST /api/hosts/box/panes/3/send

{ "text": "Run the tests again", "requestId": "legato-7f3a" }
```

`requestId` is optional, letters, digits, `-` and `_`, up to 128 characters: a retry with the same one is never typed twice, and gets the first one's answer. The hub answers once it knows whether the text landed:

| Status | Body | When |
|---|---|---|
| 200 | `{"ok":true,"state":"delivered"}` | the agent recorded it |
| 200 | `{"ok":true,"state":"queued"}` | the agent is busy and queued it for after this turn |
| 200 | `{"ok":true,"state":"sent"}` | typed and submitted; the agent hasn't written it down where Inyon can read it yet |
| 409 | `"reason": "prompt-open"` | the agent is asking a question; answer it first |
| 409 | `"reason": "busy"` | the agent's input box isn't there to type into right now |
| 409 | `"reason": "not-landed"` | the hub typed it but couldn't confirm it arrived; check before sending again |
| 409 | `"reason": "no-agent"` | no agent runs in that pane |
| 409 | `"reason": "pane-gone"` or `"offline"` | the session or its machine isn't there |
| 403 | `"reason": "scope"` | a line of the text starts with `/` or `!`: commands aren't for companions |
| 400 | `"reason": "bad-request"` | empty text, a control character or escape sequence, or a `requestId` with other characters |

A send shows in the session's list of sends in Inyon, whether it landed or not.

### What's refused

Every other route answers `403`:

```json
{ "error": "Companion apps can't use this", "reason": "scope" }
```

A companion token never opens `/node`, the `/v/` pages, or the `/api/local` routes of the `inyon` command on the hub's computer.
