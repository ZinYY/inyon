<p align="center"><img src="apps/web/public/icon.svg" width="88" height="88" alt=""></p>

<h1 align="center">Inyon</h1>

<p align="center">
  <b>Your coding agents, from your phone.</b><br>
  Watch, answer and steer Claude Code, Codex, Gemini CLI, OpenCode, GitHub Copilot<br>
  and Antigravity CLI running in tmux on your own computers. Each keeps its own account.
</p>

<p align="center">
  <a href="https://github.com/Abhash-Chakraborty/inyon/releases/latest"><picture><source media="(prefers-color-scheme: dark)" srcset="https://inyon.tech/badge/release-dark.svg"><img alt="Latest release" src="https://inyon.tech/badge/release-light.svg"></picture></a>
  <a href="https://github.com/Abhash-Chakraborty/inyon/actions/workflows/ci.yml"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/ci/Abhash-Chakraborty/inyon.svg?workflow=ci.yml&variant=secondary&mode=dark"><img alt="CI status" src="https://shieldcn.dev/github/ci/Abhash-Chakraborty/inyon.svg?workflow=ci.yml&variant=secondary&mode=light"></picture></a>
  <a href="#quick-start"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/badge/runs%20on-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows%20%C2%B7%20Android.svg?variant=secondary&mode=dark"><img alt="Runs on Linux, macOS, Windows and Android" src="https://shieldcn.dev/badge/runs%20on-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows%20%C2%B7%20Android.svg?variant=secondary&mode=light"></picture></a>
  <a href="LICENSE"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/license/Abhash-Chakraborty/inyon.svg?variant=secondary&mode=dark"><img alt="MIT licence" src="https://shieldcn.dev/github/license/Abhash-Chakraborty/inyon.svg?variant=secondary&mode=light"></picture></a>
</p>

<p align="center">
  <a href="https://inyon.tech/">Download</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="https://inyon.tech/docs/">Docs</a> ·
  <a href="https://github.com/Abhash-Chakraborty/inyon/releases">Releases</a>
</p>

<p align="center">
  <a href="https://www.producthunt.com/products/inyon?utm_source=badge-featured&amp;utm_medium=badge&amp;utm_campaign=badge-inyon"><picture><source media="(prefers-color-scheme: dark)" srcset="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1282413&amp;theme=dark"><img src="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1282413&amp;theme=light" width="250" height="54" alt="Inyon on Product Hunt"></picture></a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/hero-dark.webp">
  <img src="docs/hero-light.webp" alt="Inyon on a desktop, a tablet and a phone: the overview of every agent session across four machines, a Claude Code session read as a chat, and Codex asking to run the tests">
</picture>

Walk away from your laptop while an agent works. When it asks something, answer from your phone
with a tap. Sit back down and carry on in the same terminal, because nothing changed at the
keyboard.

## See it work

Codex asks before it runs the tests. Answer from the overview, then watch the results come in.

<img src="docs/answer.webp" alt="On a desktop browser, the overview zooms in on a Codex session asking to run bun test src/auth. The cursor clicks Yes, run it, opens the session as a chat, and the reply says all 31 auth tests pass.">

On a phone, read the change an agent made, watch the tests pass and send the next instruction.

<img src="docs/phone.webp" alt="On a phone, the overview scrolls to a Claude Code session fixing a flaky checkout test. Tapping it opens the chat, the edit expands into a diff, the 50-run test suite finishes with no failures, and a reply asking for a regression test is typed and sent.">

On a tablet, sessions sit in a tree by machine. Ending one says exactly what closes.

<img src="docs/tablet.webp" alt="On a tablet, the sidebar tree of machines and sessions is zoomed in, then an OpenCode session's menu opens End session. The confirmation sheet says the agent exits and its pane closes, and a Session ended toast appears.">

## Why I built it

I run agents on a laptop and two servers, each signed in to a different account. Built-in remote
features follow one account at a time, so switching machines meant switching accounts.

Inyon never signs in anywhere. It drives the agent CLI already running in tmux, so every machine
keeps its own account, subscription or API key.

## What you can do

- Read each session as a chat: replies, tool calls, diffs, plans and to-dos. The real terminal is
  one tap away.
- Answer permission requests, questions and menus with buttons, straight from the overview.
- See the model, context, cost and lines changed, plus the 5-hour and weekly plan limits.
- Switch model, effort and permission mode, or compact, clear and rewind.
- Follow queued messages, subagents and background shells, each shell with a real Stop.
- Type with `/` commands, `@` file search and attached images. Drafts mirror into the terminal.
- Set up a new machine from the app: it installs tmux and the agents, then opens sign-in.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/phones-dark.webp">
  <img src="docs/phones-light.webp" alt="Three phones: the overview with a Codex prompt waiting for an answer, a Claude Code session as a chat with its to-do list, and the Machines page with four machines, one offline">
</picture>

Inside a session the app takes on that agent's colours, logo and reply font. Everywhere else it
stays ink on silver, in light and dark.

## How it works

<img src="docs/how-it-works-anim.webp" alt="The phone or browser talks to one hub. The hub reads its own machine directly, laptops connect out to it by running inyon, and servers are reached over SSH. The Android app can do the same without a hub.">

Inyon reads tmux, the process tree and each agent's own session files, and types answers back with
`tmux send-keys`. The agents never know it's there. There are three ways to connect a machine:

| Way | What runs on the machine | How you connect |
|---|---|---|
| Direct | Nothing but tmux | The app or a hub connects over SSH |
| Paired | `inyon`, as a small user service | Scan its QR code in the Android app |
| Hub | A hub on one machine that stays on; others join it | Open the hub in any browser |

Paired machines reach the phone through a free Cloudflare tunnel or your VPN, end-to-end
encrypted with Noise, so nothing needs an open port or an account.

## Agent support

| Agent | Chat view | Live status | Prompts |
|---|---|---|---|
| Claude Code | Full | Full, with the integration | Full |
| Codex | Full | Context, limits, credits | Numbered prompts |
| Gemini CLI | Beta | Context | Numbered prompts |
| OpenCode | Beta | Context, cost | Through its API |
| GitHub Copilot | Beta | Context and activity | Native CLI prompts |
| Antigravity CLI | Yes | Plan quotas | Yes |
| Cursor, Grok, Pi, Amp, goose, Hermes, Kimi, Kiro, Qwen, Qoder, CodeBuddy | Terminal only | Working or waiting, from the screen | In the terminal |

[Agents](https://inyon.tech/docs/agents/) in the docs has the details for each.

## Why not Paseo, Happy or Claude Remote Control?

They're good tools, and each needs the agent started its own way: Paseo starts it from its
daemon, Happy when you run `happy` instead of `claude`, and Remote Control only from inside
`claude` itself. Inyon works the other way round. It picks up the agents already running in your tmux panes,
including the one you started an hour ago over SSH, and you can still start new ones from it.

| | Inyon | Paseo | Happy | Claude Remote Control |
|---|---|---|---|---|
| An agent already running in tmux | Any pane | No | No | Only by typing `/remote-control` in it |
| Needs on a server | tmux and your agents, over SSH | Its daemon | The `happy` CLI or app | Claude Code |
| Phone to computer | Encrypted end to end over a free tunnel or your VPN, or your own hub | Encrypted relay, or direct | Encrypted relay, or one you host | Anthropic's servers |
| Agents | Six as a chat, eleven more in the terminal | Seven | Several | Claude Code |
| Account | None | None | A Happy key pair | A Claude plan, no API keys |

What you give up: Paseo and Happy have iPhone apps and Inyon doesn't yet. Inyon also reads each
agent's transcript and screen, so a new agent release can break a reader until Inyon catches up.
Checked against their docs in October 2026.

## Quick start

**With the Android app**, nothing is hosted anywhere:

1. Install the APK from the [download page](https://inyon.tech/).
2. On each computer, run one line. It prints a QR code.

   ```sh
   curl -fsSL https://inyon.tech/get.sh | sh      # macOS, Linux
   irm https://inyon.tech/get.ps1 | iex           # Windows, PowerShell
   ```

   Or with npm: `npm install -g @abhash-chakraborty/inyon`, then `inyon setup`.

3. In the app, tap *Add a computer* and scan it.

From 0.19 that line installs one program, `inyon`, written in Rust: about 4 MB on Windows and
10 to 13 MB on Linux and macOS, where from 0.21 it also runs the hub. It draws the setup,
the status screen and the pairing code, and runs the link to your phone. A 0.11 or 0.12 install
crosses over on its next update, and an older one needs the line above again.

**From a browser**, run a hub on a machine that stays on, then open the address it prints:

```sh
curl -fsSL https://inyon.tech/get.sh | sh -s -- --hub
```

The hub listens only on localhost until you choose a network. [Running a hub](https://inyon.tech/docs/hub/) covers
the rest, and [SECURITY.md](SECURITY.md) explains why it matters. Without a machine that stays on,
a hub can run [on Railway or Render](https://inyon.tech/docs/hub/#in-the-cloud).

## Docs

The full documentation is at **[inyon.tech/docs](https://inyon.tech/docs/)**, with search. A few
places to start:

- [Quick start](https://inyon.tech/docs/quick-start/) and [choosing a setup](https://inyon.tech/docs/setups/)
- [Android app](https://inyon.tech/docs/android/): pairing, private networks, servers over SSH
- [Running a hub](https://inyon.tech/docs/hub/) and [adding machines](https://inyon.tech/docs/machines/)
- [The inyon command](https://inyon.tech/docs/cli/): every command and flag
- [Troubleshooting](https://inyon.tech/docs/troubleshooting/)
- [Security](https://inyon.tech/docs/security/) ([SECURITY.md](SECURITY.md) for reporting a vulnerability)
- [Contributing](CONTRIBUTING.md): development setup, tests, adding an agent

## Status

Inyon is young and moving fast. Linux, macOS and Windows (through psmux) work today. The Gemini,
OpenCode and Copilot readers are in beta, and the iOS app isn't built yet.

The name comes from 인연 (*inyeon*), the Korean word for a bond between people. 연 on its own also
means kite, which is the mark.

## Support

Inyon is free and MIT licensed. If it saves you a walk back to your desk and you want to chip in,
[GitHub Sponsors](https://github.com/sponsors/Abhash-Chakraborty) is the place. Bug reports and
pull requests help just as much.

## License

MIT. See [LICENSE](LICENSE). Agent names and logos belong to their owners; see
[NOTICE.md](NOTICE.md).
