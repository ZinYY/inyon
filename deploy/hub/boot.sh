#!/bin/bash
# The container's start: the password, then the hub on loopback, and only then Caddy on $PORT, so a
# passing health check means the hub itself answers (not just a proxy in front of nothing).
set -euo pipefail

: "${PORT:=8080}"
HUB_PORT=4777

# The image's PATH starts with folders the inyon user, and every agent it runs, can write to. This
# script runs as root first and then handles INYON_PASSWORD, so it finds programs in the system's
# folders only. The hub gets the image's PATH back, for the shells and agents it starts.
export INYON_USER_PATH=${INYON_USER_PATH:-$PATH}
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# Hosts mount the volume root-owned (with a lost+found), so HOME is a folder inside it, given to
# the inyon user before this script runs again as that user.
if [ "$(id -u)" = 0 ]; then
  mkdir -p "$HOME"
  [ "$(stat -c %u "$HOME")" = 1000 ] || chown -R 1000:1000 "$HOME"
  chmod 700 "$HOME"
  exec setpriv --reuid=1000 --regid=1000 --init-groups -- "$0" "$@"
fi

export SHELL=/bin/bash USER=inyon
mkdir -p "$INYON_DATA_DIR"

# INYON_PASSWORD becomes the hub's password whenever the variable changes: applied before the hub
# starts, with the hub's own reset-password (which signs everyone out). A change made in Settings
# stays until the variable is edited again. Only a salted hash of the last applied value is kept.
applied="$INYON_DATA_DIR/deploy-password"
# what hmseeb's Railway image kept, so moving a volume over doesn't count as a new value
[ -e "$applied" ] || [ ! -e "$INYON_DATA_DIR/railway-password" ] || mv "$INYON_DATA_DIR/railway-password" "$applied"
if [ -n "${INYON_PASSWORD:-}" ]; then
  salt=$(cut -d' ' -f1 "$applied" 2>/dev/null || true)
  [ -n "$salt" ] || salt=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')
  sum=$(printf '%s%s' "$salt" "$INYON_PASSWORD" | sha256sum | cut -c1-64)
  if [ "$(cat "$applied" 2>/dev/null || true)" != "$salt $sum" ]; then
    # on stdin, so the password never shows in the process list
    if out=$(printf '%s\n' "$INYON_PASSWORD" | inyon hub reset-password 2>&1); then
      (umask 077; echo "$salt $sum" > "$applied")
      echo "==> password set from INYON_PASSWORD"
    else
      # keep the deploy healthy; the hub keeps the password it had
      out=${out#New Inyon password: }
      echo "WARNING: INYON_PASSWORD was not applied: ${out#$'\n'}" >&2
    fi
  fi
fi

# Then once more without it, so the hub, Caddy and everything the hub starts (tmux, agents) never
# have the password in their environment, nor this process's /proc/1/environ
if [ -n "${INYON_PASSWORD+set}" ]; then
  exec env -u INYON_PASSWORD "$0" "$@"
fi

git config --global init.defaultBranch main >/dev/null 2>&1 || true

# The hub refuses any Host it wasn't told about (its DNS rebinding guard). The address the host
# gave the service counts as its own; a custom domain goes in INYON_ALLOWED_HOSTS. The health
# check is /healthz, which answers under any Host, but a Railway service set up for hmseeb's image
# checks /api/session as healthcheck.railway.app, so that name stays allowed on Railway.
railway_check=${RAILWAY_SERVICE_ID:+healthcheck.railway.app}
for name in "${RAILWAY_PUBLIC_DOMAIN:-}" "${RENDER_EXTERNAL_HOSTNAME:-}" "$railway_check"; do
  if [ -n "$name" ]; then INYON_ALLOWED_HOSTS="${INYON_ALLOWED_HOSTS:+$INYON_ALLOWED_HOSTS,}$name"; fi
done
export INYON_ALLOWED_HOSTS="${INYON_ALLOWED_HOSTS:-}"

# Which address is the client, for the hub's per-address sign-in limit. Railway's edge sends
# X-Forwarded-For as "<client>, <edge>" with an edge address that changes per request, and always
# overwrites X-Real-IP with the client, whatever the client sent (measured on Railway). Elsewhere
# (Render, plain Docker) Caddy reads X-Forwarded-For from the right past private addresses, which
# a client can't fake (see the Caddyfile).
if [ -n "${RAILWAY_SERVICE_ID:-}" ]; then
  export INYON_CLIENT_IP='{http.request.header.X-Real-IP}'
else
  export INYON_CLIENT_IP='{client_ip}'
fi

pids=()
# a stop (a redeploy, docker stop) reaches both; the hub saves its records before it ends
trap 'kill -TERM "${pids[@]}" 2>/dev/null || true' TERM INT

# PORT and BIND are the hub's own names: scoped to the hub, so Caddy keeps the host's PORT.
PORT=$HUB_PORT BIND=127.0.0.1 PATH=$INYON_USER_PATH /usr/local/bin/inyon hub &
pids+=($!)

session() { curl -fsS -m 3 "http://127.0.0.1:$HUB_PORT/api/session" 2>/dev/null; }
for _ in $(seq 1 120); do session >/dev/null && break; sleep 0.5; done
session >/dev/null || { echo "FATAL: hub did not answer on 127.0.0.1:$HUB_PORT within 60s" >&2; exit 1; }

caddy run --config /etc/inyon/Caddyfile --adapter caddyfile &
pids+=($!)

# Either process ending ends the container; the host's restart policy brings both back. The
# other one gets to finish too: the container ending first would kill the hub mid-save.
status=0
wait -n "${pids[@]}" || status=$?
kill -TERM "${pids[@]}" 2>/dev/null || true
for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
exit "$status"
