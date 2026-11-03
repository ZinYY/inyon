---
title: Running a hub
nav: Running a hub
description: Install a hub, sign in, reach it from your phone over a private network or HTTPS, pair the app, and keep it updated.
---

A hub is one process on a machine that stays on, such as a home server, a VPS or a desktop that doesn't sleep. It serves the full web app to any browser, reaches your other machines, and sends notifications for all of them. With only the [Android app](android.md) you don't need one.

Hubs run on Linux and macOS. On Windows, run one in WSL.

## Install

```sh
curl -fsSL https://inyon.tech/get.sh | sh -s -- --hub
```

This installs `inyon`, runs `inyon setup --role hub`, and starts the hub as a user service that comes back after a reboot or a crash. It prints the address to open and a **setup code**.

On a computer that already has `inyon`, run `inyon setup` and choose *Make it a hub*, or just:

```sh
inyon hub start
```

### From source

You need [Rust](https://rustup.rs), [Bun](https://bun.sh) 1.3 or later (it builds the web app) and tmux:

```sh
git clone https://github.com/Abhash-Chakraborty/inyon.git
cd inyon
bun install
VITE_INYON_PHONE_CORE=none bun run build   # the web app, which the hub carries inside it
cargo build --release -p inyon-cli
./target/release/inyon hub start
```

That builds the same `inyon` a release ships and starts the hub as a user service running it, with its settings in `~/.inyon/hub.env` as on an installed hub. It hands machines the release of its version from GitHub.

To work on Inyon itself, `bun run dev:hub` (`cargo run -p inyon-cli -- engine hub`) runs a development build in the terminal. It hands machines the `inyon` that `cargo` built into `apps/cli/dist` (or the folder in `INYON_CLI_DIST`), and the release's file for a system with nothing there. Up to 0.22 a hub could run from source on Bun (`bun start`); 0.23 removed the Bun hub, and with it `deploy/inyon-hub.service`.

From 0.19 machines that join a hub take only the `inyon` command, which connects to the hub by itself. From 0.21 the hub runs on the [hub in Rust](#the-hub-in-rust) built into `inyon` too.

### In the cloud

Without a machine that stays on, a hub can run on Railway or Render. Both build the image in this repo's [`deploy/hub`](https://github.com/Abhash-Chakraborty/inyon/tree/main/deploy/hub) folder. It downloads the release's `inyon` for amd64 or arm64, checks it against the release's signed `SHA256SUMS` the way the installer does, and runs the [hub in Rust](#the-hub-in-rust) behind Caddy on the HTTPS address the host gives it, the way [a reverse proxy](#https) is set up below. The container has tmux, git and ssh. The home folder is on a volume, so agents you install there and their sign-ins survive a redeploy. Sessions running in tmux don't, the same as after a reboot.

- **`INYON_PASSWORD` is the password.** The hub uses it from the first boot. To change it, or if you lose it, edit the variable and redeploy. Before the hub starts, the container applies a new value with `inyon hub reset-password`, which signs everyone out. A password changed in **Settings → Account** stays until the variable is edited again, and a value under 8 characters is skipped with a warning in the logs.
- **The host's address works as it is.** The container adds the `up.railway.app` or `onrender.com` name the service gets to `INYON_ALLOWED_HOSTS`. For a custom domain, add its name there yourself.
- **Machines join it** with **Add machine**, as with any hub.
- **Updating means redeploying.** The image carries the release named in `deploy/hub/Dockerfile` when it was built, and `main` moves to each new release. Settings still shows **Update available**, but the program is part of the image and can't be replaced from inside it (it stops with *Permission denied*), so redeploy instead.
- The host's health check is `/healthz`, which answers only once the hub itself does.

### On Railway

Until there's a template from this repo, deploy it from a fork:

1. Fork `Abhash-Chakraborty/inyon` on GitHub. In a Railway project, choose **New → GitHub Repo** and pick your fork.
2. In the service's **Settings**, set **Root Directory** to `deploy/hub`, and the **Healthcheck Path** to `/healthz`.
3. Right-click the service and attach a **Volume** mounted at `/data`.
4. Under **Variables**, add `INYON_PASSWORD` with a long password.
5. Under **Settings → Networking**, generate a domain, then deploy. If the hub's page says *This hub doesn't answer to that address*, redeploy once so the container sees the new domain.

To update, sync your fork on GitHub; Railway deploys the new commit.

There's also a one-click template by [hmseeb](https://github.com/hmseeb/inyon-railway), which this image is based on. It runs its own image, with Inyon 0.20.2 on the older Bun hub. Railway generates its `INYON_PASSWORD`; copy it from the service's **Variables** to sign in.

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/deploy/inyon)

A service made from that template can move to this image by connecting its source to your fork, with the root directory and health check above. It keeps its volume, with the password and records on it.

### On Render

[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/Abhash-Chakraborty/inyon)

The button reads [`render.yaml`](https://github.com/Abhash-Chakraborty/inyon/blob/main/render.yaml) from this repo: a web service built from `deploy/hub`, with a 5 GB disk at `/data` for the home folder and the health check on `/healthz`. Render generates `INYON_PASSWORD`; copy it from the service's **Environment** to sign in.

- A disk needs a paid instance. Render's free instances can't keep files and stop after 15 minutes without traffic, so the Blueprint picks `0.5c-512mb` (512 MB). That holds the hub; to run agents in the container too, choose `1c-2g` or more.
- A service with a disk stops before the new one starts, so a redeploy takes the hub away for a moment.
- Pushes to this repo don't redeploy your hub. To update, choose **Manual Deploy → Deploy latest commit**.

## First sign-in

The hub listens on `127.0.0.1:4777`. Open it, and the first visit asks you to create a password (8 characters or more), using the setup code it printed. Lost the code? `inyon hub code` shows it again. From 0.20.3 the hub prints the code itself only in a terminal; in a log it prints where the code is kept, so another user who can read the log can't use it first.

- Sign-ins last 30 days and are renewed as you use the hub.
- Change the password in **Settings → Account**; that signs out your other browsers.
- Forgot it? `inyon hub reset-password` on the hub's machine sets a new one (it asks without showing what you type) and signs everyone out. A hub running as a service restarts to pick it up.
- Wrong passwords are slowed down and rate-limited, per address and in total.

## Reaching it from your phone

The hub listens only on its own machine until you choose otherwise, because anyone who reaches it can try your password, and it can run commands on every machine you add. Pick one way in:

### A private network

On Tailscale, Headscale, NetBird, ZeroTier or WireGuard, let the hub listen on that network's address. In `inyon setup`, choose the network under *Network*; setup writes it to `~/.inyon/hub.env`. On a Tailscale or NetBird address:

```sh
BIND=100.64.0.5
INYON_ALLOW_HTTP=1
```

`INYON_ALLOW_HTTP=1` allows plain `http://` on that address, which is fine on a VPN that already encrypts. Then open `http://<machine>:4777` from any device on the network. On a ZeroTier or WireGuard address, which machines can't tell apart from a LAN, setup makes a certificate as it does for [the LAN](#the-lan) (from 0.20), and the hub answers on `https://`. Names on Tailscale (`.ts.net`), NetBird and common home networks (`.local`, `.lan`, `.internal`, `.home.arpa`) are accepted automatically.

From 0.20 a machine sends its token over plain `http://` only to a Tailscale or NetBird address (`100.64.0.0/10`, or Tailscale's `fd7a:115c:a1e0::/48`), or to a name that points only there, such as the machine's `.ts.net` name. From 0.20.2 a NetBird name that also has an IPv6 address passes: NetBird picks that address at random per network, in a unique-local range (`fd…`), so it's accepted next to the name's `100.x` address, though never alone. On ZeroTier or WireGuard, whose addresses look like any LAN to the machine, setup gives the hub HTTPS, and the **Add a machine** command carries its certificate's SHA-256. See [Adding machines](machines.md#plain-http).

### HTTPS

HTTPS is needed for the web app installed to a phone's home screen, for notifications in a browser and for pasting images. Any of these gives you it:

- **`tailscale serve`**, where your tailnet has HTTPS: `tailscale serve --bg 4777`, then open `https://<machine>.<tailnet>.ts.net`.
- **`netbird expose`**, for a public HTTPS address through NetBird's proxy.
- **A reverse proxy** such as Caddy in front of `127.0.0.1:4777`. Add your domain to `INYON_ALLOWED_HOSTS`.
- **Its own certificate**: set `TLS_CERT` and `TLS_KEY` to PEM files. Setup makes one for the [LAN](#the-lan).

> **Warning:** Don't open port 4777 to the internet without HTTPS. Over plain HTTP your password crosses the network readable.

### The LAN

Choose *LAN* in setup (`BIND=0.0.0.0`) to answer on every interface of a home network. Anyone on that network can listen in, so from 0.20 the hub serves HTTPS there, never plain http. Setup makes a certificate for the machine's name (`name` and `name.local`), `localhost`, its addresses and the address you told the hub to listen on, saves it as `~/.inyon/tls/hub.crt` and `hub.key` (readable only by you), points `TLS_CERT` and `TLS_KEY` at them and removes an `INYON_ALLOW_HTTP=1` an earlier setup wrote. It prints the certificate's SHA-256 fingerprint:

```
✓ HTTPS, on a certificate made for this machine. The browser warns about it once; check that its SHA-256 fingerprint is
  3F:A1:…:9C
```

No authority vouches for that certificate, so the browser warns the first time. Open the certificate from the warning, compare its SHA-256 fingerprint with the one setup printed, and only then go on. A certificate you set in `TLS_CERT` yourself is kept, and so is setup's own when you run setup again, so the machines added with it keep working. To make a new one, for example after the machine's address changed, delete `~/.inyon/tls` and run setup again; then add the machines again.

The hub puts the certificate's SHA-256 into every **Add a machine** command (from 0.20), and the machine accepts exactly that certificate from the hub's address, and never as an authority for anything else. The Windows command can't check it yet: add a Windows machine through a VPN address, or give the hub a certificate Windows trusts. The Android app doesn't accept a certificate like this yet either, so on the LAN it connects through a VPN address or `tailscale serve`.

Before 0.20, choosing *LAN* served plain http and wrote `INYON_ALLOW_HTTP=1`. Prefer a VPN if the network isn't yours alone.

## Pair the Android app

Once you're signed in, **Open on your phone** shows a QR code and a typed code. It's on the overview until a phone is paired, under Machines, and in **Settings → Paired devices**. In the app, tap **Add a computer**, then **Connect to a hub**, and scan it or type the hub's address and the code.

- Only someone who signed in within the last 10 minutes can show a code, so an old tab asks for the password first.
- A code works once, for 10 minutes; five wrong tries cancel it.
- From 0.20.2 the code's panel also shows the **Hub ID**, and the **Certificate** when the hub serves HTTPS itself. The app shows the same before it pairs; check they match.
- The phone gets a token of its own. **Settings → Paired devices** lists paired phones; **Forget** cuts one off at once.

The phone has to reach the hub's address the same way a browser does. Over plain http that means a Tailscale or NetBird address, or a name that points to one; see [Connect to a hub](android.md#connect-to-a-hub).

## Pair a companion app

From 0.20 an app like Legato can pair too, with **Settings → Paired devices → Pair a companion app** or `inyon hub companion-code` on the hub's computer. It types a six-character code that works once, for 5 minutes, and gets four permissions: the overview, chats, answering a question and sending text. From 0.20.2 **Copy pairing link** puts the address, the code and the certificate's fingerprint in one string, for an app that takes a pasted link. See [Companion apps](companions.md).

## Adding machines

The hub reaches its own machine directly. Add the others from **Machines → Add machine**; see [Adding machines](machines.md).

## Updating

Nothing updates by itself. When a release is out, Settings shows **Update available**.

- **An installed hub** updates with **Update** in Settings, or `inyon update` on its machine. It downloads the release, checks it against the signed `SHA256SUMS`, swaps it in and restarts. If the new version doesn't start, it goes back to the old one.
- **A hub built from source** updates in its checkout: `git pull`, build again, then `inyon hub restart`. A development build (`bun run dev:hub`) says so in Settings in place of **Update**.

After an update, browsers that have the app open offer **Reload**. Machines that joined the hub update from their pages under Machines.

## Running it

```sh
inyon hub restart
inyon hub stop               # stops it and removes the service; its records stay
inyon logs --role hub -f
```

From 0.23.1, while no app on screen has the hub open, it checks its machines every 20 seconds instead of every 4, which also spares the computers it reaches. Agent hooks and tmux still report changes at once, and the hub reads every machine again as soon as an app opens. A browser tab or phone app left in the background counts as not looking after 15 seconds.

The hub keeps its records in `~/.inyon` (or `INYON_DATA_DIR`): the password hash, machines, paired phones, settings, drafts, and the last state of each offline machine. [Configuration](configuration.md) lists every setting.

## Pages

From 0.20 the hub also serves the sites your agents make on its machine, such as Blueprint plans, at `/v/<link>/` on its own port. Pages are listed in `~/.inyon/pages.json`. Each link is signed, opens one page for 12 hours and needs no cookie, so a reverse proxy in front of the hub has to pass `/v/` through as it does everything else. [Security](security.md#pages) says how a page is kept away from your sign-in.

## The new app at /next

From 0.24 the hub also serves an early copy of Inyon's next app, written in Rust, at `/next` on its own address (for example `https://hub.example/next`). It uses the same password and sign-in as the web app, and so far has the sign-in, the Overview, a session's chat and terminal, the Machines screen and, from 0.25, Pages, Settings and Temp files:

- the conversation, with replies, tool calls and code blocks you can copy, and **Earlier messages** as you scroll up
- the message box: Enter sends, a message sent while the agent works waits in its queue, and Esc in an empty box stops the agent
- an agent's question or approval as a card you answer by tapping a choice or pressing its number, with the model, effort and mode line back once it's answered
- the terminal, under **Chat · Terminal** in the session header, as the web app has it (see [The terminal](terminal.md)): the key bar, text size, typing in any language through your keyboard's input method, and a right-click menu (a long press on a phone) with **Copy**, **Paste** and **Select all**. A plain shell, or an agent Inyon doesn't read as chat, opens on its terminal.
- **Machines** and each machine's page, as [Machines](machines.md) describes them: adding a machine with a pairing command or over SSH, **New session**, the **Sessions**, **Setup**, **Config**, **Uploads** and **Integration** tabs, signing an agent in and OpenCode's providers, updating a machine's `inyon`, renaming and removing, **Phone** for the code your phone scans, and [Can't connect?](troubleshooting.md#cant-connect) for a machine that isn't connected or for the hub itself (click **Reconnecting**). Right-click a machine or a session, or press and hold on a phone, for its menu.
- **Pages**, as [Pages](pages.md) describes them: the list by machine and project with its badge, the filter, live progress, the sandboxed viewer with **Plan**, **Mockups**, **Matrix**, **Notes** and the device sizes, **Add a folder**, **Show folder**, **Revoke links** and **Remove** with **Undo**. A machine's page lists its pages under **Pages on this machine**, a reply that names a page shows it as a card under the message, and a session with pages of its own has a **Pages** button in its header. The list follows the hub as pages change, without a refresh. The pages on a phone's own computers are only in the Android app.

- **Settings**, as [Settings](settings.md) describes it for the web app on a computer, with the same sections and the side list from desktop width: the theme and the rest of **Appearance** (kept in the browser with the web app's, so the two agree), the agents with your own **custom agents**, the [companion agent](companion-agent.md), **Live numbers**, uploads, notifications, machines, your other devices, paired phones and [companion apps](companions.md) with their pairing code, the password, **Sign out**, the hub's **Update** and **About**. Right-click a custom agent, a paired phone or a companion app, or press and hold on a phone, for its menu.
- **Temp files**, as [Temp files](temp-files.md) describes them, at `/next/temp` and in a machine's **Temp files** tab.
- the hub's notices as toasts (a session that finished or needs you, a page that moved on, the companion's replies), and the browser's own notification while the tab is hidden, once you allow it. From 0.25 allowing them also signs the browser up for Web Push, as the web app does, so alerts reach it while Inyon is closed, and **Send a test** goes through the push service. On an iPhone, Safari has notifications only for an app added to the Home Screen, and the row says so. **About** says how the terminal is drawn.
- the session header's **⋯** menu, as in the web app: Remote Control, **Compact context**, **Rewind…** (OpenCode's **Undo**, **Redo** and **Share link…**), **Fork conversation**, **Copy session ID**, **Clear conversation**, **Exit to shell** and **End session…**.
- **End session…** in a session's right-click menu, the **Companion** in the sidebar and on the Overview while it runs, *Inyon was updated* once the hub runs newer code, and the same short intro on the first load of a tab.

A few things are still only in the web app: pinning and parking sessions, and in the session header **Session controls…**, **Past conversations…** and renaming. Everything else stays at `/` until the new app can do all of it. A hub you build from source carries it only if you run `apps/app/build.sh` first (it needs Bun and `bun install` for the terminal); without it, `/next` answers "The new app isn't in this build of the hub" with a 404.

## The hub in Rust

From 0.21 the hub runs on the hub built into `inyon`, written in Rust; 0.20 carried it as an opt-in test. It answers the web app, the Android app and joining machines as the Bun hub did, from the same records in `~/.inyon`: your password, phones, machines and Pages carry over both ways. It runs on Linux and macOS.

A hub set up by 0.20 or earlier moves to it with its next `inyon update` or `inyon hub restart`. Up to 0.22, `INYON_HUB_IMPL=bun` put a hub back on the Bun hub; 0.23 removed it, so the line in `hub.env` is no longer read and `inyon` says so (see [The Bun engine, removed in 0.23](install.md#the-bun-engine-removed-in-0-23)). A hub can't run from source on Bun either from 0.23; build `inyon` instead (see [From source](#from-source)).
