# Inyon

<a href="https://www.producthunt.com/products/inyon?utm_source=badge-featured&amp;utm_medium=badge&amp;utm_campaign=badge-inyon"><img src="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1282413&amp;theme=light" width="250" height="54" alt="Inyon on Product Hunt"></a>

Your coding agents keep running in tmux on your machines. Inyon lets you watch them, answer them
and steer them from your phone or any browser. It works with Claude Code, Codex, Gemini CLI,
OpenCode, GitHub Copilot CLI and Antigravity CLI.

```sh
npm install -g @abhash-chakraborty/inyon
inyon setup
```

`inyon setup` copies `inyon` into `~/.inyon/bin`, installs the background service and prints a
QR code to pair the [Android app](https://inyon.tech/download/). From 0.21 a hub runs on `inyon`
too; only a hub put back on Bun downloads the engine, checked against the release's signed
`SHA256SUMS`.

## What this package installs

Only the `inyon` command, 4 to 13 MB depending on the system. npm picks the build for your system from
`@abhash-chakraborty/inyon-<platform>-<arch>` (Linux, macOS and Windows, x64 and arm64). Nothing
runs at install time, so `--ignore-scripts` works. Every version is published from the release
workflow on GitHub with provenance, and the programs are the ones in that release's signed
`SHA256SUMS`.

To update the command, run `npm install -g @abhash-chakraborty/inyon@latest`. From 0.17.1,
`inyon update` updates the copy in `~/.inyon/bin` that the background service runs.

Docs: https://inyon.tech/docs/install/ · Source: https://github.com/Abhash-Chakraborty/inyon · MIT
