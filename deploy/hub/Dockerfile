# An Inyon hub in a container, for Railway, Render or any host that builds a Dockerfile: tmux, the
# hub built into the release's inyon command on loopback, and Caddy in front of it on $PORT.
# Based on hmseeb's Railway image (see README.md for the credit and its licence).
#
#   docker build -t inyon-hub deploy/hub
#   deploy/hub/test/contract.sh          # what the image promises, against a real container

FROM caddy:2.10 AS caddy

# The release's inyon for this system, trusted only after its SHA256SUMS carries a good minisign
# signature from the release key and the file matches the list. Nothing is pinned by hand, so a
# new release needs only the version below (scripts/deploy-hub.test.ts fails until it matches).
FROM debian:bookworm-slim AS fetch
ARG TARGETARCH
ARG INYON_VERSION=0.24.0
ARG RELEASE_PUBLIC_KEY=RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates minisign \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /release
RUN set -eu; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) file=inyon-linux-x64;; \
      arm64) file=inyon-linux-arm64;; \
      *) echo "No Inyon release for $TARGETARCH"; exit 1;; \
    esac; \
    base="https://github.com/Abhash-Chakraborty/inyon/releases/download/v$INYON_VERSION"; \
    curl -fsSL --retry 3 "$base/SHA256SUMS" -o SHA256SUMS; \
    curl -fsSL --retry 3 "$base/SHA256SUMS.minisig" -o SHA256SUMS.minisig; \
    minisign -V -P "$RELEASE_PUBLIC_KEY" -m SHA256SUMS -x SHA256SUMS.minisig; \
    curl -fsSL --retry 3 "$base/$file" -o "$file"; \
    grep -E "^[0-9a-f]{64}  $file\$" SHA256SUMS > want; \
    [ "$(wc -l < want)" = 1 ] || { echo "$file isn't listed once in SHA256SUMS"; exit 1; }; \
    sha256sum -c want; \
    install -m 755 "$file" /release/inyon; \
    # the signature names no version, so an older signed release can't pass for this one: ask it
    [ "$(/release/inyon --version)" = "inyon $INYON_VERSION" ] || { echo "Got $(/release/inyon --version), not $INYON_VERSION"; exit 1; }

FROM debian:bookworm-slim
# tmux is what Inyon drives; procps for the process tree it reads; git, ssh and curl so the
# in-app "set up this machine" and the agents' own installers work.
RUN apt-get update && apt-get install -y --no-install-recommends \
      tmux procps git openssh-client curl ca-certificates less nano xz-utils unzip \
    && rm -rf /var/lib/apt/lists/*

COPY --from=fetch /release/inyon /usr/local/bin/inyon
COPY --from=caddy /usr/bin/caddy /usr/local/bin/caddy
COPY Caddyfile /etc/inyon/Caddyfile
COPY boot.sh /usr/local/bin/inyon-boot

# Not root: Claude Code refuses --dangerously-skip-permissions as root, and the hub hands out a
# shell as this user. inyon-boot starts as root only to give the volume to it, then drops.
RUN useradd -m -u 1000 -s /bin/bash inyon \
 # tmux panes run login shells, and Debian's /etc/profile resets PATH: keep the agents' installs on it
 && echo 'export PATH="$HOME/.local/bin:$HOME/.inyon/bin:$PATH"' > /etc/profile.d/inyon-path.sh

# The volume goes on /data; the home folder is a folder in it, since hosts mount it root-owned.
# INYON_HUB_IMPL=rust keeps the hub in the inyon program even if a hub.env on the volume, from an
# older image, still asks for the Bun one.
ENV HOME=/data/home \
    INYON_DATA_DIR=/data/home/.inyon \
    INYON_HUB_IMPL=rust \
    PATH=/data/home/.local/bin:/data/home/.inyon/bin:/usr/local/bin:/usr/bin:/bin \
    PORT=8080 \
    LANG=C.UTF-8

EXPOSE 8080
CMD ["/usr/local/bin/inyon-boot"]
