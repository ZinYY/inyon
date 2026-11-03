---
title: Install
description: How to install Inyon with the one-line installers or npm, where it lives on a computer, and how to update or remove it.
---

Every computer that runs agents for you gets one program, `inyon` (written in Rust; about 4 MB on Windows, 10 to 13 MB on Linux and macOS, where from 0.20 it also holds the Rust hub). It sets things up, shows status, keeps everything updated, and from 0.19 does the work behind it too: the link to your phone and the connection to a hub. From 0.21 a [hub](hub.md) runs on `inyon` as well. Before 0.23 a hub could go back to `inyon-engine`, a larger program built with Bun; 0.23 removed it. You only ever type `inyon`.

## The one-line installers

On macOS or Linux:

```sh
curl -fsSL https://inyon.tech/get.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://inyon.tech/get.ps1 | iex
```

To make the computer a [hub](hub.md) instead (Linux and macOS):

```sh
curl -fsSL https://inyon.tech/get.sh | sh -s -- --hub
```

The site's earlier address, inyon.abhashchakraborty.tech, still works and redirects here, so older commands, links and installs keep working.

### What they do

1. Work out the system and processor: Linux or macOS on x64 or arm64, Windows on x64 or arm64. Anything else stops with a message. On Alpine and other musl systems, a hub gets the engine built for musl.
2. Download the release's `SHA256SUMS` and make sure it's the signed one. From 0.20 the copy of each installer this site serves carries the sha256 of the list the site checked against Inyon's release key when it published the release, so it needs no other tool. A copy from anywhere else, or one pointed at another release, checks the list's signature with [minisign](https://jedisct1.github.io/minisign/), and that the list is the one for the version it asked for. Without minisign it stops, unless you [tell it to go on](#without-minisign). Before 0.20 the installers checked the signature only when minisign was installed.
3. Download the `inyon` command for this system, compressed when `gzip` is available, and check it against `SHA256SUMS`, both compressed and unpacked. A mismatch stops the install before anything runs.
4. Put it in `~/.inyon/bin/inyon` (`%USERPROFILE%\.inyon\bin\inyon.exe` on Windows). On macOS it gets an ad-hoc signature so the system will run it.
6. Put `inyon` on your `PATH` (from 0.17.1), so a new terminal finds it by name. See [Put inyon on your PATH](#put-inyon-on-your-path).
7. Run `inyon setup`, which installs the background service and prints the pairing QR code. With `--hub` it runs `inyon setup --role hub`, which prints the hub's address and setup code instead (in 0.19 and 0.20 it also downloaded `inyon-engine`).

Piped from `curl`, setup can't ask questions, so it takes the defaults: a computer that pairs with your phone through a free tunnel and your VPN, started at login. Run `inyon setup` again afterwards to choose differently with the [setup screens](cli.md#setup).

> **Note:** The terminal you installed from doesn't see the new `PATH` yet. Open a new one, or run `export PATH="$HOME/.inyon/bin:$PATH"` first. Until then the command is `~/.inyon/bin/inyon`.

### Installing from somewhere else

The installers download from this site's [/download](/download/), which holds the newest release. Without `gzip`, `get.sh` takes the uncompressed `inyon` from the same release on GitHub instead, checked against the same signed list. Installers from 0.14.0 and earlier download from GitHub.

Both read `INYON_DOWNLOAD`, a folder URL to fetch from instead. Every release stays on GitHub, so point it at one to pin a version:

```sh
curl -fsSL https://inyon.tech/get.sh | INYON_DOWNLOAD=https://github.com/Abhash-Chakraborty/inyon/releases/download/v0.17.0 sh
```

From 0.20 that needs [minisign](https://jedisct1.github.io/minisign/) (`brew install minisign`, `apt install minisign`, `scoop install minisign`), since the list it fetches isn't the one the site checked. The installers check its signature against Inyon's release key, and that it lists the APK of the version asked for: the tag in a GitHub address, the release GitHub calls latest, or the version in the folder's `latest.json`. An older release's list is refused even though it's signed too.

### Without minisign

To install from somewhere else on a computer without minisign, tell the installer to go on without the signature:

```sh
curl -fsSL https://inyon.tech/get.sh | INYON_DOWNLOAD=https://github.com/Abhash-Chakraborty/inyon/releases/download/v0.17.0 sh -s -- --no-signature-check
```

```powershell
$env:INYON_DOWNLOAD = 'https://github.com/Abhash-Chakraborty/inyon/releases/download/v0.17.0'
$env:INYON_NO_SIGNATURE_CHECK = '1'
irm https://inyon.tech/get.ps1 | iex
```

`--no-signature-check` (or `INYON_NO_SIGNATURE_CHECK=1`) only skips a check nothing on the computer can make. Every download is still checked against the list, and with minisign installed a list with a bad signature is refused all the same. `inyon` checks the signature itself on every update from then on.

## With npm

From 0.17.0 the `inyon` command is also on npm, for any system above with Node.js 18 or later:

```sh
npm install -g @abhash-chakraborty/inyon
inyon setup
```

npm installs only the `inyon` command and puts it on your `PATH`. It picks the build for your system from the packages `@abhash-chakraborty/inyon-<platform>-<arch>`, which hold the same files as the release. Nothing runs at install time, so `--ignore-scripts` works. `inyon setup` then copies `inyon` into `~/.inyon/bin` (and for a hub downloads and checks the engine like the installers do), so the background service doesn't depend on npm's folder.

- **Update the command** with `npm install -g @abhash-chakraborty/inyon@latest`. From 0.17.1, `inyon update` run from npm's copy updates `~/.inyon/bin`, which the service runs, and leaves npm's copy to npm.
- **Remove it** with `inyon uninstall` first, then `npm uninstall -g @abhash-chakraborty/inyon`.
- **Not found after installing?** npm's global folder isn't on your `PATH`. `npm prefix -g` prints it: add that folder on Windows, or its `bin` folder elsewhere, to your `PATH`.
- **Check it** with `npm audit signatures` after installing. Every release from 0.17.0 is published by the release workflow on GitHub with a provenance statement, and the [npm page](https://www.npmjs.com/package/@abhash-chakraborty/inyon) links to the run that built it.

## Supported systems

| System | `inyon` | Can be a hub |
|---|---|---|
| Linux x64, arm64 | Yes | Yes |
| Alpine and other musl Linux, x64, arm64 | The same static build | Untested |
| macOS Apple silicon, Intel | Yes | Yes |
| Windows x64 | Yes | No (use WSL) |
| Windows arm64 | Yes | No |

Every computer also needs tmux, or [psmux](https://github.com/marlocarlo/psmux) on Windows. From 0.17.2 `inyon setup` installs it when it's missing, after asking: with winget (or cargo) on Windows, Homebrew on macOS, and your package manager through `sudo` on Linux. `inyon uninstall` offers to remove it again only when setup installed it. The setup tab in the app can install it, and the agents, too. See [Computers](computers.md#windows).

## Put inyon on your PATH

From 0.17.1 the installers do this for you, once, when `~/.inyon/bin` isn't on your `PATH` already:

| Shell | What `get.sh` adds |
|---|---|
| zsh | a line in `~/.zshrc` (in `$ZDOTDIR` when that's set) |
| bash | a line in `~/.bashrc`, and on macOS in `~/.bash_profile` too |
| fish | the file `~/.config/fish/conf.d/inyon.fish` |
| anything else | a line in `~/.profile` |

Each line ends with `# added by Inyon`, and running the installer again doesn't add another. On Windows, `get.ps1` puts `%USERPROFILE%\.inyon\bin` on your user `Path`, and on the window you ran it in; from 0.17.2 `inyon setup` adds it too when it's missing, so a copy that got into `~\.inyon\bin` some other way is found by name as well. `inyon uninstall` takes all of it back out, and nothing else in those files.

To leave your shell's files and `Path` alone, set `INYON_NO_MODIFY_PATH=1` before installing:

```sh
curl -fsSL https://inyon.tech/get.sh | INYON_NO_MODIFY_PATH=1 sh
```

Then add it yourself, or run the command by its full path, `~/.inyon/bin/inyon`. With an installer from 0.17.0 or earlier, add it by hand:

```sh
echo 'export PATH="$HOME/.inyon/bin:$PATH"' >> ~/.zshrc    # or ~/.bashrc
```

```powershell
[Environment]::SetEnvironmentVariable("Path", "$env:USERPROFILE\.inyon\bin;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")
```

npm puts `inyon` on your `PATH` itself, so the installers' line isn't needed there.

## Where things live

| Path | What it holds |
|---|---|
| `~/.inyon/bin/` | `inyon`, `cloudflared` for the tunnel (from 0.20 setup downloads it; before, the first tunnel did), and on Windows the copy described [below](#on-windows) |
| `~/.inyon/node/` | This computer's identity key, paired phones, link settings, hub connection, logs (only you can read it) |
| `~/.inyon/` | A hub's own records: password hash, machines, settings, `hub.env`, and from 0.20 `tls/`, the certificate setup makes for a hub on the LAN |

`INYON_DATA_DIR` moves the hub's records elsewhere. The [Configuration](configuration.md) page lists every setting.

## The background service

Setup runs Inyon as a service of your own user, so it starts on its own and comes back after a crash. It never needs `sudo`.

| System | How it runs | Starts at | After a crash |
|---|---|---|---|
| Linux | systemd user unit `inyon-link`, `inyon-node` or `inyon-hub` | Boot (lingering is turned on) | Restarts after 5 s |
| macOS | LaunchAgent `tech.abhashchakraborty.inyon.<role>` | Login | Restarts |
| Windows | The Inyon app, opened at sign-in by the scheduled task "Inyon link" or "Inyon node" | Login | The app starts it again |
| Linux without user systemd | A background process, plus an `@reboot` cron line | Boot, when cron exists | No |

The role is `link` for a computer paired with your phone, `node` for one that joined a hub, and `hub` for a hub. `inyon service status` shows what's installed and running, and `inyon logs` shows its output.

### On Windows

From 0.20.3 Inyon on Windows is an app. Setup adds **Inyon** to the Start menu and to **Settings → Apps → Installed apps**, and both open `inyon-app.exe`, a copy of `inyon.exe` marked as a program without a console. Nothing Inyon does opens a terminal window: not at sign-in, not the window's buttons, and not the programs it starts (the engine, `cloudflared`, psmux, PowerShell).

The app runs what Inyon keeps going in the background: the link to your phone (and its tunnel) or the connection to a hub. They're its own child processes, in a Windows job, so they end when the app ends, even if it crashes. The scheduled task only opens the app when you sign in; it starts nothing itself. Their output goes to `%USERPROFILE%\.inyon\node\link.log` (`node.log` for a computer that joined a hub), and the app's own to `app.log` beside them.

The Inyon icon is in the notification area exactly while something runs. Click it to show or hide the Inyon window, and right-click it for its menu; [The Inyon app on Windows](windows-app.md) describes both.

The `inyon` command works with the app instead of starting copies of its own. `inyon status` says what runs in the background and that the Inyon app runs it, `inyon stop` stops all of it, and `inyon service install` or `inyon update` ask the running app to start or restart a job. A computer set up by 0.20.2 or earlier moves to the app by itself the next time its old engine starts or it updates: the tasks are registered again to open the app, and the app takes over the running engine.

Before 0.20.3 the task started `inyon-engine.exe` itself, the window's **Change**, **Show all** and **Details** opened a terminal, and the tray icon belonged to the engine, so it could be missing while the link ran. If Windows hides the icon under the arrow beside the clock, drag it onto the taskbar to keep it in view.

Everything stays under `%USERPROFILE%\.inyon`, also when a Git or MSYS shell sets `HOME` to another folder. `inyon-engine.exe` stays as the windowless copy that runs the link and the hub connection, so the firewall rule "Inyon link" and older commands keep the name they know.

### The Inyon window

From 0.21 the window has a page for each part of Inyon on this computer: the phone link and how phones reach it, Phones, a hub's web app, Pages, Temp files, Activity and Settings. See [The Inyon app on Windows](windows-app.md). From 0.20.1 to 0.20.5 it was a smaller window of a fixed size with the pairing code, the reach, the phones and this computer's service.

Setup lists Inyon under **Settings → Apps → Installed apps** (from 0.17.2), with its version and the kite icon, and `inyon update` keeps the version current. An update from 0.20.2 or 0.20.3 leaves the old version's entry there; from 0.20.4 the Inyon app puts the right one back within a few minutes of starting. From 0.20.3 **Uninstall** there asks to confirm, then removes Inyon without opening a terminal and says when it's done; before, it ran `inyon uninstall --purge` in a console window that stayed open until you pressed Enter.

## Updating

```sh
inyon update
```

It reads the newest version from this site's `latest.json` and checks that the release's `SHA256SUMS` carries a good signature from Inyon's release key and lists that version's APK. Then it downloads the new `inyon`, checks it against that list, and only then swaps it in. From 0.23 it never downloads `inyon-engine`, and an update removes the one an older version left (on Windows it's replaced by the copy of `inyon.exe`). A computer whose hub still hands out an `inyon` from before 0.19 keeps the engine, or gets the hub's, because that `inyon` still runs on it. Every running role restarts on the new version. On Windows the Inyon app itself starts again on its new version first, then runs the roles again; an update by 0.20.3 or 0.20.4 restarted only the roles and left the old app running until the next sign-in, and from 0.20.5 the new link or hub connection asks that old app to start its new version. If the new version doesn't stay up for a few seconds, the old files go back and the roles restart on them.

Versions after 0.14.0 update from this site; 0.14.0 and earlier from GitHub. A computer that joined a hub updates from the hub instead, so it always matches the hub's version. From 0.20 it checks those files against the release's signed `SHA256SUMS`, which the hub passes on, and changes nothing when the hub has none (a hub from before 0.20): update the hub first. The hub can update it for you from its page under Machines. See [Updating hubs and machines](hub.md#updating).

`inyon update --background` does the same without keeping your terminal open, which is how the app and the hub start an update remotely.

If you installed the command with npm, update that copy with npm too. See [With npm](#with-npm).

## Uninstalling

```sh
inyon uninstall            # remove the services and the programs, keep pairings
inyon uninstall --purge    # also delete ~/.inyon: pairings, keys, a hub's records
inyon uninstall --yes      # don't ask anything
```

From 0.17.1 both also take `inyon` back off your `PATH`: the lines marked `# added by Inyon`, Inyon's fish file, or the entry on the Windows user `Path`.

From 0.17.2, when setup installed tmux (psmux on Windows) for you, uninstall asks whether to remove it too; the answer is no unless you say yes, and `--yes` keeps it. tmux that was there before Inyon is never touched. On Windows it also removes the entry under Settings → Apps and, from 0.20.3, the Start menu shortcut.

On Windows from 0.17.1, uninstalling first stops everything that runs from `~/.inyon/bin` (the Inyon app, the engine and the tunnel), since Windows can't delete a program that's running. `inyon.exe` itself goes a moment after the command ends. If a file still can't be removed, it says which one, and from 0.17.2 a file in use under `~/.inyon` (an agent's hook writing the event log, say) stops only itself from going, not the rest. It also removes the firewall rule "Inyon link" when setup added one, which Windows asks an administrator to allow.

From 0.17, `--purge` first gives Claude Code and Antigravity their own status line back and drops Inyon's hooks from their settings, so they never call scripts that are gone. It also removes folders older versions left: `~/.tether`, `~/.tether-node` and `~/.cache/inyon-ssh`.

To stop being part of a hub but keep Inyon installed, use `inyon disconnect`. To unpair one phone, use `inyon forget <phone>`.

## Coming from tether-node

Inyon was called Tether before 0.7.0, and its computer program was `tether-node`. Run the installer again, because those versions updated from a release address that no longer exists. Nothing else needs redoing: its files move from `~/.tether-node` to `~/.inyon/node` (leaving a link behind) the first time the new program runs, and `inyon` moves the old service to its new name the next time it updates or sets the machine up. Settings named `TETHER_*` are still read as `INYON_*`.

Before 0.13, `inyon` was the Bun program itself. It became the small Rust command, and the Bun program became `inyon-engine`, which from 0.19 only a hub used, and from 0.21 only a hub put back on Bun. Two things changed for people who type it:

- Plain `inyon` now shows a [status dashboard](cli.md#status). Setup is `inyon setup`.
- `inyon install` and `inyon uninstall <role>` became `inyon service install` and `inyon service uninstall`. The old forms still work.

## The Bun engine, removed in 0.23

Up to 0.22, `INYON_LEGACY_ENGINE=1` ran a computer on the Bun engine as 0.18 did, and `INYON_HUB_IMPL=bun` put a hub back on the Bun hub. 0.23 removed both: `inyon` runs the link, the hub connection and the hub itself. Neither switch is read any more, whether in your shell, a service or `~/.inyon/hub.env`. While one is still set, `inyon service install`, `inyon hub start` and `inyon hub restart` say so, and `inyon doctor` lists it:

```sh
Note: INYON_LEGACY_ENGINE is no longer read: the Bun engine and hub were removed in 0.23, and inyon runs its own.
```

The notes 0.22 kept for them (`~/.inyon/node/legacy-engine` and `~/.inyon/node/hub-impl`) go the first time one of these runs, and the next update removes a Bun engine left in `~/.inyon/bin`. An `inyon` from 0.12 to 0.22 still downloads `inyon-engine` as it updates to 0.23: that file is now a copy of the new `inyon`, which answers as the engine until the link or the hub connection next starts and removes it (on Windows it stays as the windowless copy). If something that worked on 0.22 doesn't now, please [report it](https://github.com/Abhash-Chakraborty/inyon/issues), and [pin 0.22.5](#installing-from-somewhere-else) until it's fixed.
