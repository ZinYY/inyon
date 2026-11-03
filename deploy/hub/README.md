# Inyon hub in a container

The image Railway and Render build for a hub in the cloud: tmux, the release's `inyon` (with the hub built in) on `127.0.0.1:4777`, and Caddy in front of it on `$PORT`. How to deploy it is in the docs: [Running a hub](https://inyon.tech/docs/hub/#in-the-cloud).

| File | What it does |
|---|---|
| `Dockerfile` | Downloads `inyon` for amd64 or arm64 from the release in `INYON_VERSION`, after checking the release's `SHA256SUMS` against its minisign signature and the file against the list |
| `boot.sh` | Gives the volume to uid 1000, applies `INYON_PASSWORD`, starts the hub and then Caddy |
| `Caddyfile` | `/healthz` for the host's health check, and the one client address the hub counts sign-ins against |
| `test/contract.sh` | Builds the image and checks it against real containers, as Railway and Render would run it |

`render.yaml` at the repository root is the Render Blueprint for it. `scripts/deploy-hub.test.ts` fails when `INYON_VERSION` differs from the release in `package.json`, so a release moves it along with the rest.

```sh
docker build -t inyon-hub deploy/hub
deploy/hub/test/contract.sh
```

## Credit

This is based on [hmseeb/inyon-railway](https://github.com/hmseeb/inyon-railway) by Haseeb Azhar, who worked out the non-root user, the volume handling, applying `INYON_PASSWORD` with `reset-password`, and how Railway's edge forwards the client's address. It's used under the MIT licence:

```
MIT License

Copyright (c) 2026 Haseeb Azhar (hmseeb)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
