# Private Linux hub releases

Run the `Private Linux Hub` workflow on the desired commit. It tests the readers and
hub, checks the probe with macOS /bin/sh, builds a static Linux x64 binary with the
web app embedded, and publishes a signed GitHub Release. Increase the Cargo workspace
and package.json version for every release. `RELEASE_SIGNING_KEY` is a repository
Actions secret containing the Minisign private key; only the publish job receives it.

The installed hub reads `~/.inyon/update.env` when `inyon update` runs:

```
INYON_GITHUB_REPO=ZinYY/inyon
INYON_DOWNLOAD=https://github.com/ZinYY/inyon/releases/latest/download
INYON_SIGNING_KEY=<base64 public key line>
```

Keep this file private (0600). Optional `https_proxy`, `http_proxy`, `all_proxy`,
`NO_PROXY`, and `no_proxy` entries apply to the update process. Explicit environment
variables take precedence. The file is parsed as data, never evaluated as shell.
The GitHub CLI must be installed and authenticated with access to this repository.
The updater uses `gh api` only for the exact configured GitHub repository; it still
verifies the signed SHA256SUMS and downloaded binary before replacing anything.

The first official-to-private upgrade must download the signed assets with `gh`
and feed them to the official updater via a temporary loopback HTTP server at a
pinned `/releases/download/v<version>` path. After that, plain
`~/.inyon/bin/inyon update` uses the configuration above. The web Settings version
check still refers to the upstream release channel; use the CLI for private updates.

Save an independent backup of the binary and `.inyon` data before deployment. The
updater rolls back on startup failure, but functional verification is separate.
Chat cache reader version 5 rebuilds cached transcripts and summaries automatically.
No Codex session or transcript needs to be deleted or restarted.
