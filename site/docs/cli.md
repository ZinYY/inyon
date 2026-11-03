---
title: The inyon command
nav: inyon command
description: Every command and flag, the setup screens, the status dashboard and JSON output.
---

`inyon` is the command you run on each computer. It sets the computer up, shows what it's doing, pairs phones, manages the background service and updates itself. From 0.19 it also runs the link to your phone and the connection to a hub itself, and from 0.21 the hub too. Before 0.19 all of the work happened in `inyon-engine`, a separate program built with Bun, which 0.23 removed.

If `inyon` isn't found, open a new terminal: the installers put `~/.inyon/bin` on your `PATH` from 0.17.1. Before that, or with `INYON_NO_MODIFY_PATH=1`, see [Put inyon on your PATH](install.md#put-inyon-on-your-path).

## At a glance

| Command | What it does |
|---|---|
| `inyon` | The status dashboard |
| `inyon setup` | Choose how this computer connects, then pair |
| `inyon pair` | Show the pairing QR code and code again |
| `inyon phones` | List paired phones |
| `inyon forget <phone>` | Unpair a phone |
| `inyon join <hub>` | Add this computer to a hub |
| `inyon disconnect` | Leave the hub |
| `inyon hub start` | Run a hub here, as a service |
| `inyon hub stop`, `restart` | Stop or restart the hub |
| `inyon hub code` | Show the hub's setup code |
| `inyon hub companion-code` | Make a code for a companion app, such as Legato |
| `inyon hub reset-password` | Choose a new hub password |
| `inyon hub machines` | List the hub's machines |
| `inyon sessions` | List the tmux sessions on the hub's machines, and their agents |
| `inyon send <machine>/<session> "text"` | Give an agent on any of the hub's machines a message |
| `inyon push` | List the browsers this computer sends notifications to |
| `inyon service install`, `uninstall`, `status` | Manage the background service |
| `inyon stop` | Stop everything Inyon runs in the background here |
| `inyon logs` | Show the service's output |
| `inyon doctor` | Check that everything is in place |
| `inyon update` | Update to the newest release |
| `inyon skill list`, `add`, `remove` | Install the agent skills that come with each release |
| `inyon pages add`, `ls`, `rm` | Show a site's folder in the app under Pages |
| `inyon temp`, `files`, `show`, `rm`, `clean` | List, look at and delete the temp files agents leave here |
| `inyon docs [topic]` | Read these docs in the terminal |
| `inyon uninstall` | Remove Inyon |
| `inyon version` | Show the version |

Every command takes `--json` and `-h`/`--help`. `inyon -V` prints the version. From 0.22 `inyon --help` ends with where the docs are.

## Status

```sh
inyon
inyon status --plain
```

In a terminal, plain `inyon` opens a dashboard that refreshes every 2 seconds. It shows this computer's role (hub, joined to a hub, paired with phones, or not set up), its addresses while the link runs, whether phones reach it only through the tunnel and how to change that (from 0.20), the background jobs and their versions, paired phones, a hub's machines, the agents running in tmux, and the last lines of the log. Times are in this computer's time zone. At 72 columns or wider it uses two columns. From 0.20.1 every part keeps its rows, so a refresh changes what a row says but never moves it: a part with less to show leaves blank rows, one with more ends in "and 3 more", and a note (the hub can't be reached, say) shows at the top of the log.

| Key | Does |
|---|---|
| `p` | Show the pairing code |
| `s` | Open setup, with its questions |
| `r` | Read everything again now |
| `q`, `Esc`, `Ctrl-C` | Quit |

On Windows before 0.20.1 a key could act twice, because Windows also reports a key's release.

The dashboard and setup draw in greys that suit the terminal's background. From 0.20.2 they ask the terminal whether it's light or dark and use darker greys on a light one, so muted text stays readable on white. A terminal that doesn't answer gets the greys for a dark one; set `INYON_THEME=light` to say otherwise.

From 0.20.3 it says what runs in the background and how to stop it. On Windows that's the Inyon app (**Quit Inyon** in the icon's menu, or `inyon stop`); elsewhere `inyon stop` for the link and the hub connection, and `inyon hub stop` for a hub.

`--plain` prints the same as lines of text, and `--json` as one object. Outside a terminal, `inyon` prints the plain form on its own.

From 0.22 the plain form ends with where the docs are, and on a machine joined to a hub, a line saying its sessions and pages show in the hub's app. `--json` carries the same lines as `hints`, and the docs' address as `docs`.

From 0.17.2 the link counts as running when something answers on its port the way the engine does, not only when the process it last recorded is alive, so a stale record never shows a running link as stopped.

The dashboard draws on the terminal's second screen, so what was in the window before comes back when you quit, and from 0.17.2 the status it showed is printed there as plain lines, like any command's output.

## Setup

```sh
inyon setup
```

Setup asks a few questions in full-screen steps, applies the answers, and ends by showing the pairing code. The installers run it for you.

### The steps

1. **Role.** *Pair it with the phone* (the default), *Make it a hub* (Linux and macOS), or *Add it to a hub*.
2. **Network.** For a computer paired with your phone, tick any of:
   - *Tunnel*: a free Cloudflare tunnel, so the phone reaches it from any network. On by default.
   - *LAN*: also answer on the local network. Off by default.
   - *VPN*: also answer on Tailscale, Headscale, NetBird, ZeroTier or WireGuard. Setup lists the networks it found, and the computer answers on all of them. On by default, except on Windows, where it needs a firewall rule (see [below](#the-firewall-on-windows)).
   - *Own address*: a `wss://` address you publish yourself.
   - *Port*: 4780 by default.

   For a hub, pick where it listens: *This machine only*, the LAN, or one of the VPN networks found, or type an address. The port is 4777. From 0.20 the LAN, or any address that isn't a VPN's, gets HTTPS on a certificate setup makes, with its fingerprint printed; see [The LAN](hub.md#the-lan). To join a hub, paste the hub's whole *Add a machine* command (from 0.20.3), or type the hub's address and then the token. The token shows as dots, also inside a command typed into the box, and one hub's token is never sent to a different hub you type in afterwards. A phone's pairing link (`inyon://hub?…`, what the hub's QR code holds) is turned away with where the machine command is. Before 0.20.3 *Add it to a hub* took only the address, so it failed with a usage line unless `INYON_TOKEN` was set.
3. **Background.** *Run in the background* (a service that starts at login or boot), or *Only while this window is open*.
4. **Sign in** (hub only). How you'll create the hub's password from the browser.
5. **Summary.** What will happen, with every network it will answer on. Press Enter to set it up.

Setup no longer downloads an engine. Before 0.19 every computer needed `inyon-engine`, and setup downloaded it in the background while you answered, with the progress in the footer; in 0.19 and 0.20 every hub did, and up to 0.22 a hub on Bun.

After the screens, from 0.17.2, setup installs tmux (psmux on Windows) if it's missing: it asks first, then runs winget (or cargo) on Windows, Homebrew on macOS, or your package manager with `sudo` on Linux. With `--yes` it doesn't ask; without a terminal on Linux it prints the command to run instead, since `sudo` needs one. On Windows it also lists Inyon under Settings → Apps.

| Key | Does |
|---|---|
| `↑` `↓`, `Tab`, `j` `k` | Move between rows |
| `Space` | Choose the row, or tick and untick a box |
| `Enter` | Go on. On a step with one choice (Role, Background, a hub's network) it picks the row first; ticked boxes stay as they are |
| `→` / `←` | Next or previous step |
| `Esc`, `Ctrl-C`, `q` | Quit without changing anything |

A mouse click chooses a row.

### The firewall on Windows

Windows Firewall lets nothing in to a program it hasn't been told about, and it counts VPNs such as NetBird as public networks. So when you tick *LAN* or *VPN* on Windows, setup from 0.17.1 adds a firewall rule named "Inyon link", and Windows asks an administrator to allow it. The rule lets in TCP on the link's port, to `inyon-engine.exe` only, from the local network and the VPN ranges (`100.64.0.0/10` and Tailscale's `fd7a:115c:a1e0::/48`).

If you say no, phones can only connect through the tunnel, and setup prints the command to add the rule later from PowerShell run as administrator. `inyon uninstall` removes the rule. From 0.20, when the link already runs and you change these settings, setup restarts it so they take effect, and `inyon doctor` checks the rule.

### Without the screens

With `--yes`, or without a terminal (as when piped from `curl`), setup takes flags and defaults instead of asking:

| Flag | Meaning |
|---|---|
| `-y`, `--yes` | Don't ask |
| `--role hub`, `node` or `join` | What this computer is. `node` pairs with your phone; `join` needs `--hub` |
| `--network <list>` | For a node: any of `tunnel`, `lan`, `vpn` and one `wss://` address, comma-separated. For a hub: `localhost`, `lan`, `vpn` or an address |
| `--port <n>` | The port to listen on |
| `--autostart`, `--no-autostart` | Install the background service, or not. A hub without it runs in the foreground |
| `--hub <url>` | The hub to join. The machine's token goes in `INYON_TOKEN`. From 0.20.3 it also takes the hub's whole *Add a machine* command, token included |

```sh
inyon setup --yes --role node --network tunnel,vpn
inyon setup --yes --role hub --network vpn
INYON_TOKEN=… inyon setup --yes --hub https://hub.example.ts.net
```

## Pairing phones

```sh
inyon pair
```

Shows the QR code, the one-time ID and 6-digit code, and the link. A code works once, for 10 minutes, and five wrong tries cancel it. While the tunnel starts it says "Starting the tunnel…" first; from 0.20 it waits for the tunnel's address and shows the code again if the address arrives after the first 30 seconds (see [How the phone finds it](computers.md#how-the-phone-finds-it)). When its output isn't a terminal, as in an installer run by a script, the code it shows after those 30 seconds is the last one, and the phone finds the computer by its ID once the tunnel is up. `--json` keeps waiting either way. It also prints the computer's ID, which the app shows before it pairs.

From 0.20.3, inside tmux it leaves the link out and says why: Inyon shows every tmux pane in the app and to companion apps, and the link pairs a phone by itself. `inyon pair --show-link` prints it anyway, and `--json` always carries it. Any link options you add are saved, the same ones `inyon link` takes:

| Option | Default | Meaning |
|---|---|---|
| `--tunnel`, `--no-tunnel` | On | Use a free Cloudflare quick tunnel |
| `--lan`, `--no-lan` | Off | Answer on the local network: it listens on every interface, and from 0.20.3 publishes only private, link-local and VPN addresses |
| `--vpn`, `--no-vpn` | On (off on Windows) | Answer on a private network's address |
| `--url <wss://…/link>` | | An address you publish yourself (repeatable); `--no-url` clears them |
| `--port <n>` | 4780 | The link's local port |

```sh
inyon phones                 # paired phones, with when each was last seen
inyon forget pixel           # unpair by name or key prefix
```

[Pairing a computer](computers.md) explains how the phone finds the computer and keeps the connection private.

## Hubs

```sh
inyon hub start              # install and start the hub service
inyon hub stop               # stop it and remove the service; its records stay
inyon hub restart
inyon hub code               # the setup code for the first sign-in
inyon hub companion-code     # a one-time code for a companion app, such as Legato
inyon hub reset-password     # asks for the new password without showing it, then restarts the hub
```

`hub start` waits until the hub answers, then prints its address and setup code. Hubs run on Linux and macOS. Plain `inyon hub` runs one in the foreground; on Windows it says to use WSL (from 0.22; before, it fetched the engine and failed, `inyon hub --help` too).

From 0.20 `hub companion-code` asks the running hub for a pairing code, the one **Settings › Paired devices › Pair a companion app** shows, and prints it with the hub's address (a hub that listens only on `127.0.0.1` says to use the address you open it at). It works once, for 5 minutes, and a new code ends the last one. See [Companion apps](companions.md).

```sh
INYON_TOKEN=… inyon join https://hub.example.ts.net
inyon disconnect
```

`join` saves the hub's address and this machine's token (readable only by you) and installs the `node` service. From 0.20.3 it first trades the pasted token for a new one the hub hands out once, so the command in your shell history no longer works; running the same command again on the machine keeps the token it got. An older hub doesn't trade, and the pasted token stays. When the hub says the token was used already or replaced, `join` stops with "The hub refused this token", and you need a new command from **Add a machine**. Before 0.19 it also fetched the matching engine from the hub. You rarely type it: the hub's *Add a machine* gives you a one-line command that does this. `disconnect` removes the service and the saved hub.

Given a phone's pairing link instead of a hub's address, `join` says from 0.20.3 that the link pairs a phone, and that the command to add this computer is under the hub's **Machines → Add a machine**.

## Sessions and messages

```sh
inyon hub machines                         # the hub's machines and what runs on them
inyon sessions                             # every tmux session and the agent in it
inyon pages --hub                          # the pages on all the hub's machines
inyon send laptop/experiment "The report is in ~/inyon-reports"
inyon send abhash-arm/main:1.0 - < notes.md
```

From 0.22, on the hub's computer or one joined to it, with no sign-in. On the hub's computer they use the token the hub writes for the `inyon` command as it starts; on a joined computer, the token that computer keeps for its link to the hub. Both need hub 0.22 or later; a hub on Bun (up to 0.22, with `INYON_HUB_IMPL=bun`) refuses them.

```
abhash-arm/main    claude  working  /home/abhash/development/custom-tmux
abhash-arm/notes   none
laptop/experiment  codex   idle     C:\demo\robot
```

`send` names a session as `machine/session`, or as `machine/session:window.pane` when it runs more than one agent, and `-` reads the message from stdin. The hub holds the message and types it in once the agent has been idle for a moment, the same as a message you hold in the app, so it shows in the app until then and you can edit or drop it there. A machine name is matched in any case; a session name as tmux has it.

- **From the hub's computer**, `send` always works.
- **From a joined computer**, it works only when **Settings › Machines › Machines can message agents** is on. It's off until you turn it on, because a joined computer could then type into agents on all your machines. Reading machines, sessions and pages needs no switch.

These commands can only read and hold messages. Everything else the hub answers with 403. `--json` gives the hub's answer as it came: `hosts` for `hub machines` and `sessions`, `pages` for `pages --hub`, and `held` with the message's `id` for `send`.

### Reading and answering, on the hub's computer

```sh
inyon read abhash-arm/main                 # the last 20 messages of that conversation
inyon read abhash-arm/main -n 40
inyon answer laptop/experiment 1           # picks choice 1 of the question on screen
inyon answer laptop/experiment "don't ask again"
```

From 0.22, on the hub's computer only; from a joined computer the hub refuses them with *Only the inyon command on the hub's own machine can read conversations, answer questions and run the companion.* The [companion agent](companion-agent.md) uses them.

`read` prints one line per message: `you:` for what was typed, `agent:` for the agent's words, `tool:` for each command or file it used, and `error:` for a tool that failed. Thinking and tool output are left out. `-n` takes 1 to 200.

`answer` takes the choice's number, or words from it: the whole label in any case, else the one label that starts with them, else the one that contains them. When more than one matches, or none, it lists the choices. It answers only the question on screen now, and says *No question is waiting in that session* when there is none.

### The companion agent

```sh
inyon companion                            # whether it runs
inyon companion start                      # with Claude Code
inyon companion start --agent codex
inyon companion stop
```

From 0.22, on the hub's computer. It starts or stops the [companion agent](companion-agent.md), as **Settings › Companion** does.

## Notifications to browsers

```sh
inyon push
```

Lists the browsers this computer sends alerts to directly, for [browser copies](browser-copy.md) of the app. `inyon push test` sends a test.

## The background service

```sh
inyon service status
inyon service install [link|node|hub]
inyon service uninstall [link|node|hub]
```

Without a role, `install` picks `node` if a hub is saved and `link` otherwise. On Windows from 0.20.3 the Inyon app runs it, and `install` asks the running app to start it.

```sh
inyon stop
```

From 0.20.3 `stop` stops everything Inyon runs in the background on this computer: on Windows the Inyon app and all it runs, elsewhere the link and hub connection services. They start again at the next sign-in, or with `inyon setup` or `inyon service install`. A hub keeps running; `inyon hub stop` stops it. From 0.20, installing the link also downloads `cloudflared` when the tunnel is on and it isn't there yet. [Install](install.md#the-background-service) has the details for each system.

## Logs

```sh
inyon logs                   # the last 50 lines
inyon logs -f                # follow
inyon logs --role hub -n 200
```

On Linux the service's output is in the systemd journal, and `inyon logs` reads it from there. Elsewhere it reads `~/.inyon/node/<role>.log`.

## Doctor

```sh
inyon doctor
```

Checks, one line each: the engine (from 0.19 "built in", plus the hub's engine on a hub) and the command, tmux (psmux on Windows), which agents are on the `PATH`, each service, leftovers from tether-node, the link's and the hub's ports, the private networks found, `cloudflared`, the clock and the last update. A line starting with `!` needs attention. With `--json`, `ok` is `false` when any check failed.

From 0.20 a `reach` line says when phones reach the computer only through the tunnel (the default on Windows), and how to let phones on the same Wi-Fi or VPN in. On Windows with *LAN* or *VPN* on, doctor also checks the link's port and the firewall rule "Inyon link", and prints the command that adds the rule when it's missing. On a machine added to a hub, a `hub` line says whether its connection may go to the saved address, and repeats a warning or problem the connection reported, such as a hub on plain http (see [Plain http](machines.md#plain-http)).

## Update and uninstall

```sh
inyon update
inyon update --background
inyon uninstall
inyon uninstall --purge
inyon uninstall --yes
```

`--purge` removes the pairings, keys and logs too. `--yes` (from 0.17.2) asks nothing: tmux or psmux that setup installed stays.

See [Updating](install.md#updating) and [Uninstalling](install.md#uninstalling).

## Skills

```sh
inyon skill list
inyon skill add blueprint
inyon skill add blueprint --to ./.claude/skills
inyon skill remove blueprint
```

From 0.17.7. `add` downloads the skill from the newest release, checks it against the signed SHA256SUMS, and installs it for each coding agent on the computer. `--to` picks one folder instead, and `--force` replaces a skill of the same name that Inyon didn't install. See [Agent skills](skills.md).

## Pages

```sh
inyon pages add ~/development/handy-redesign
inyon pages add ./private/plans/ghostex --name "Inyon plan" --slug ghostex
inyon pages                  # the pages on this computer; the same as inyon pages ls
inyon pages rm ghostex
```

From 0.20. `add` lists a site's folder in `~/.inyon/pages.json`, so the app shows it under Pages. The folder needs an `index.html`. It can't be `/`, your home folder, or anything under `~/.ssh` or `~/.inyon`, judged by the path and by where its symlinks lead. On Windows a mapped network drive works like any other drive letter, and the share's top folder counts as too wide. No hub has to be running: the hub reads the file again when it changes. From 0.22.5, run inside a Claude Code or Codex session, `add` also records that session's conversation, so the app groups the page under the session's name.

```
"Inyon plan" is in Inyon: Pages > abhash-arm > custom-tmux
Open it in the app at /pages/local/ghostex
```

The second line is where the page is in the app, not a link to share. A link that opens the page directly is made by the app each time you open it, and lasts 12 hours (see [Security](security.md#pages)).

| Option | Default | Meaning |
|---|---|---|
| `--name <text>` | The site's own name, its `<title>`, or the folder's name | What the app calls the page |
| `--slug <slug>` | The site's own slug, or the folder's name | The page's name in links: lowercase letters, digits and dashes, up to 63 characters, unique on the computer. A slug that's taken gets `-2`, `-3` and so on, unless you asked for it with `--slug`, which is refused instead |

- **The site's own name and slug** come from `.inyon-page.json` in its folder. Blueprint writes that file when it makes a site, and `add` writes or updates it, so the site keeps its slug if it's ever added again.
- **The project** the app groups it under is the folder's git repository, or the folder itself outside one. In a linked worktree it's the repository's main folder, so the page keeps its place after the worktree is removed. If git doesn't answer within 3 seconds, it's the folder itself.
- **Who added it** is saved too: `claude`, `codex`, `gemini` or `opencode` when one of those agents ran the command, `cli` otherwise.
- **Adding a folder again** is safe. It keeps its slug and its place, and changes only the name or slug you pass.

`ls` prints each page's slug, name and folder, with a note when a page can't open (its folder is gone, say). `rm` takes a page out of the app and leaves its folder as it is; from 0.22.4 the folder is remembered, so the hub doesn't [find](pages.md#pages-the-hub-finds) it again until you `add` it. On the hub's own computer `--json` gives each page's place in the app as `app`. From 0.22 a computer joined to a hub leaves `app` out and names the computer to look under instead, since only the hub knows the computer's place in the app.

On a computer joined to a hub, the hub shows its pages too, from 0.22 (both the hub and that computer need 0.22). `add` there says so instead of giving a place in the app:

```
"Inyon plan" is in Inyon: Pages > laptop > custom-tmux
This machine doesn't run a hub. The hub it's joined to shows its pages (hub 0.22 or later).
```

A computer with no hub that isn't joined to one either, such as one paired only with the phone, has nothing to show its pages. From 0.22 `add` says so there, and `--json` carries `"joined": false`:

```
"Inyon plan" is listed on laptop, under custom-tmux.
This machine doesn't run a hub and isn't joined to one, so the app can't open this page. Add it to a hub (Machines > Add a machine) to show its pages there.
```

If an agent's sandbox can't write to `~/.inyon`, the command says so. Run the same line in your own terminal.

## Docs

```sh
inyon docs                   # every page, with what it covers
inyon docs machines          # one page
inyon docs add machine       # words from its title work too
```

From 0.22. Every page of these docs is inside the program, as the Markdown this site is built from, so it matches the version you have and reads with no browser or network. A topic is a page's name, the start of one (`trouble`), or words from its title or description. `--json` gives the page's `title`, `description`, `url` and `text`, or with no topic the list as `pages`. A topic that matches nothing exits with code 1.

It's meant for coding agents as much as for you: the [inyon skill](skills.md#inyon) tells an agent to read the right page before it guesses.

## Temp files

```sh
inyon temp                               # what's there, per agent, and the biggest items; the same as inyon temp ls
inyon temp files /tmp/claude-1000/-home-me-dev-site/7409eef9-d507-4808-a592-0bfabe896e3a
inyon temp show /tmp/claude-1000/-home-me-dev-site/7409eef9-d507-4808-a592-0bfabe896e3a/scratchpad/notes.md
inyon temp rm ~/.claude/paste-cache/0a49e8c135b0fa0a.txt /tmp/claude-1000/shots
inyon temp clean --older-than 7d
```

From 0.21. `inyon temp` lists the folders coding agents keep temporary files in, with one item per Claude Code session, upload day or loose file. [Agent temp files](temp-files.md#whats-listed) has the folders; transcripts and history are never among them.

```
Agent temp files on laptop: 3.3 GB in 153 items
  Claude Code temp files         3.3 GB    56 items  /tmp/claude-1000
  Claude Code pasted text        1.3 MB    79 items  ~/.claude/paste-cache
```

The biggest items follow, with their size, age and path.

| Command | What it does |
|---|---|
| `inyon temp files <item>` | Every file inside one item, newest first (at most 5,000). `<item>` is its path, or its `id` from `inyon temp ls --json` |
| `inyon temp show <path>` | Prints a text file (the first 256 KB), or says what kind of file it is. With `--json` an image comes as base64, up to `--max` bytes (8 MB) |
| `inyon temp rm <path>…` | Deletes items or files for good and says how much it freed. Exit code 1 if anything stayed |
| `inyon temp clean --older-than <age>` | Deletes every item older than `7d`, `24h`, `30m` or `2w`. It lists them and asks first; `--yes` skips the question, and without a terminal nothing is deleted unless you pass it |

`rm` and `clean` delete only what's inside those folders, never a folder itself, and refuse a path with `..` in it or one that leads out through a symlink. A symlink is deleted as a link. Items changed in the last 30 minutes are left alone by `clean`, since an agent may still be using them, and so are folders it couldn't read all the way through.

## JSON output

`--json` makes every command answer one JSON object, for scripts. From 0.20.3 a command piped into one that stops reading early, such as `inyon status --plain | grep -q hub`, ends quietly instead of with a "Broken pipe" panic. Success looks like `{"v":1,"ok":true,…}` and a failure like `{"v":1,"ok":false,"error":"…"}`, with exit code 1.

`inyon pair --json` can answer more than once, one line each, and the last line is the current one. From 0.20, while the tunnel starts, it first prints `{"v":1,"ok":true,"waiting":"tunnel",…}`. If the tunnel takes longer than 30 seconds, a line with a code and `"waiting"` follows, carrying the addresses that work so far, and a last line has the code with the tunnel's address. If the code runs out before the tunnel comes up, the last line is `{"v":1,"ok":false,"error":"The pairing code expired before the tunnel came up. …","expired":true}`.

```sh
inyon status --json | jq '.phones[].name'
inyon pair --json | jq -r 'select(.waiting == null and .ok) | .link'
```

## Environment

| Variable | Meaning |
|---|---|
| `INYON_TOKEN` | The machine's token when joining a hub |
| `INYON_DOWNLOAD` | Where to fetch releases from, instead of this site's copy of the newest one (`https://inyon.tech/download`) |
| `INYON_DATA_DIR` | Where a hub keeps its records (default `~/.inyon`) |
| `INYON_WEB_URL` | The web page pairing QR codes point to, for your own [browser copy](browser-copy.md) |
| `INYON_THEME` | `light` or `dark`: the terminal's background, instead of asking the terminal (from 0.20.2) |
| `NO_COLOR` | Turn colour off |

Every `TETHER_*` name from before the rename is still read. [Configuration](configuration.md) lists every variable, for computers and hubs, including `INYON_ALLOW_INSECURE` and `INYON_HUB_CERT`. `INYON_ENGINE`, `INYON_LEGACY_ENGINE` and `INYON_HUB_IMPL` are no longer read from 0.23 ([why](install.md#the-bun-engine-removed-in-0-23)).
