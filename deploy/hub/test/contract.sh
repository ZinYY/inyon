#!/bin/bash
# What the image promises Railway, Render and whoever deploys it, checked against real containers:
# the hub answers through Caddy on $PORT, the client address can't be faked, INYON_PASSWORD signs
# in and replaces the password, the hub runs as uid 1000, and the volume keeps everything.
#
#   deploy/hub/test/contract.sh [image]   builds deploy/hub as inyon-hub-contract:test unless given one
#
# CONTRACT_PLATFORM=linux/amd64 runs an image of another system (under emulation, slowly);
# CONTRACT_PORT picks the first of the two local ports it uses (18080).
set -uo pipefail
cd "$(dirname "$0")/.."
IMG=${1:-inyon-hub-contract:test}
[ -n "${1:-}" ] || docker build -q -t "$IMG" . >/dev/null || { echo "build failed"; exit 1; }
PLATFORM=${CONTRACT_PLATFORM:+--platform=$CONTRACT_PLATFORM}

ID=inyon-hub-contract-$$
LOCAL=${CONTRACT_PORT:-18080}
fail=0
ok() { echo "ok   $1"; }
no() { echo "FAIL $1"; fail=1; }
cleanup() {
  docker rm -f "$ID-railway" "$ID-render" >/dev/null 2>&1
  docker volume rm "$ID-railway" "$ID-render" >/dev/null 2>&1
}
trap cleanup EXIT

# container NAME LOCAL_PORT PORT [docker run args...]: started with a volume of the same name, once
# the hub answers through Caddy
NAME= URL= HOST=
start() {
  local name=$1 port=$2 inner=$3; shift 3
  docker run -d $PLATFORM --name "$name" -p "127.0.0.1:$port:$inner" -v "$name:/data" -e PORT="$inner" "$@" "$IMG" >/dev/null || return 1
  wait_up "http://127.0.0.1:$port"
}
wait_up() {
  for _ in $(seq 1 90); do curl -fs -o /dev/null "$1/healthz" && return 0; sleep 1; done
  echo "never came up:"; docker logs "$NAME" 2>&1 | tail -20; return 1
}
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json() { curl -s -H 'Content-Type: application/json' -H "Origin: https://$HOST" "$@"; }
# login PASSWORD [curl args]: the status code
login() { local pw=$1; shift; json -o /dev/null -w '%{http_code}' -X POST --data "{\"password\":\"$pw\"}" "$@" "$URL/api/login"; }
# cookie PASSWORD [curl args]: the session cookie a sign-in sets. It's Secure, so curl won't keep it
# over http: it's carried by hand
cookie() { local pw=$1; shift; json -D - -o /dev/null -X POST --data "{\"password\":\"$pw\"}" "$@" "$URL/api/login" | sed -n 's/^[Ss]et-[Cc]ookie: *\([^;]*\).*/\1/p' | head -1; }
authed() { curl -s -H "Host: $HOST" -H "X-Forwarded-Proto: https" -H "Cookie: $1" "$URL/api/session"; }

# ---------- on Railway ----------
# What its edge sends: X-Forwarded-For "<client>, <edge>", the edge address changing per request,
# and X-Real-IP <client> written over whatever the client sent.
NAME=$ID-railway URL=http://127.0.0.1:$LOCAL HOST=inyon-test.up.railway.app
PW=correct-horse-battery-staple
railway=(-e RAILWAY_SERVICE_ID=svc-test -e RAILWAY_PUBLIC_DOMAIN=$HOST)
edge() { local client=$1; shift; curl -s -H "Host: $HOST" -H "X-Forwarded-Proto: https" -H "X-Forwarded-For: 198.18.$((RANDOM % 250)).$((RANDOM % 250)), 100.64.0.$((RANDOM % 250))" -H "X-Real-IP: $client" "$@"; }

start "$NAME" "$LOCAL" 9090 "${railway[@]}" -e INYON_PASSWORD="$PW" || exit 1

s=$(edge 203.0.113.1 "$URL/api/session")
[[ $s == *'"setup":false'* ]] && ok "the hub answers through Caddy on \$PORT, claimed from INYON_PASSWORD at first boot" || no "hub not claimed: $s"

[ "$(code -H "Host: $HOST" "$URL/api/session")" = 200 ] && ok "Railway's public domain is allowed without INYON_ALLOWED_HOSTS" || no "public domain refused"
[ "$(code -H "Host: evil.example.com" "$URL/api/session")" != 200 ] && ok "an unknown Host is refused" || no "unknown Host answered 200"

h=$(curl -s -D - -H "Host: healthcheck.railway.app" -H "Cookie: inyon_session=x" "$URL/healthz")
[[ $h == *' 200'* && $h == *'"setup":false'* && $h != *[Ss]et-[Cc]ookie* ]] && ok "/healthz is the hub's own answer under the checker's Host, with no cookie" || no "healthz: $h"
[ "$(code -H "Host: anything.example.org" "$URL/healthz")" = 200 ] && ok "/healthz answers under any Host (Render checks a custom domain)" || no "healthz refused a Host"
[ "$(code -H "Host: healthcheck.railway.app" "$URL/api/session")" = 200 ] && ok "a Railway healthcheck on /api/session (an older setup) still passes" || no "healthcheck.railway.app refused"

h=$(edge 203.0.113.1 -D - -o /dev/null -X POST -H 'Content-Type: application/json' -H "Origin: https://$HOST" --data "{\"password\":\"$PW\"}" "$URL/api/login")
grep -qi '^set-cookie:.*secure' <<<"$h" && ok "INYON_PASSWORD signs in, with a Secure cookie behind the edge" || no "login/cookie: $h"
c=$(edge 203.0.113.1 -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -H "Origin: https://$HOST" --data '{"password":"wrong-password"}' "$URL/api/login")
[ "$c" = 401 ] && ok "a wrong password is refused" || no "wrong password got $c"

# The sign-in limit counts X-Real-IP, whatever X-Forwarded-For the client forged
for _ in $(seq 1 8); do
  last=$(edge 203.0.113.7 -X POST -H 'Content-Type: application/json' -H "Origin: https://$HOST" --data '{"password":"wrong-password"}' "$URL/api/login")
done
last=$(edge 203.0.113.7 -X POST -H 'Content-Type: application/json' -H "Origin: https://$HOST" --data '{"password":"wrong-password"}' "$URL/api/login")
other=$(edge 203.0.113.8 -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -H "Origin: https://$HOST" --data '{"password":"wrong-password"}' "$URL/api/login")
[[ $last == *'Too many'* && $other = 401 ]] && ok "a forged X-Forwarded-For counts as the X-Real-IP client" || no "9th try: $last, another client: $other"

u=$(docker exec "$NAME" sh -c 'ps -o uid= -p 1; ps -o uid= -C inyon; ps -o uid= -C caddy' | tr -d ' ' | sort -u | tr '\n' ' ')
[ "$u" = "1000 " ] && ok "the hub, Caddy and the start script run as uid 1000" || no "uids: $u"
docker exec "$NAME" sh -c 'command -v tmux git ssh' >/dev/null && ok "tmux, git, ssh present" || no "tools missing"
lp=$(docker exec -u inyon "$NAME" bash -lc 'echo $PATH')
[[ $lp == *"/data/home/.local/bin"* ]] && ok "login shells keep ~/.local/bin on PATH" || no "login PATH: $lp"
v=$(docker exec "$NAME" inyon --version)
want=$(sed -n 's/^ARG INYON_VERSION=//p' Dockerfile)
[[ $v == *"$want"* ]] && ok "it runs the release named in the Dockerfile ($want)" || no "version: $v, wanted $want"
docker exec "$NAME" sh -c 'test ! -e /data/home/.inyon/bin/inyon-engine && ! pgrep -x inyon-engine' && ok "the hub is the one built into inyon; no Bun engine" || no "the Bun engine was fetched or runs"
p=$(edge 203.0.113.1 "$URL/")
[[ $p == *'<script'* ]] && ok "the web app is served" || no "web app: ${p:0:200}"

# A stop reaches the hub, which saves its drafts and ends before the container does, rather than
# being killed mid-save or after the timeout
docker exec -u inyon "$NAME" rm -f /data/home/.inyon/drafts.json
docker stop -t 20 "$NAME" >/dev/null
e=$(docker inspect -f '{{.State.ExitCode}}' "$NAME")
docker run --rm $PLATFORM -v "$NAME:/data" --entrypoint test "$IMG" -s /data/home/.inyon/drafts.json \
  && [ "$e" != 137 ] && ok "a stop lets the hub save before it ends (exit $e)" || no "not saved on stop, or killed (exit $e)"
docker start "$NAME" >/dev/null; wait_up "$URL" || exit 1
[ "$(login "$PW" -H "Host: $HOST" -H "X-Real-IP: 203.0.113.2")" = 200 ] && ok "the password survives a restart (volume)" || no "after restart login failed"

# Programs an agent drops in ~/.local/bin (first on the image's PATH) never run in the start
# script, which runs as root and then handles the password. The hub still gets that PATH, so what
# it runs itself (cat, head…) may come from there, as uid 1000
docker exec -u inyon "$NAME" sh -c 'mkdir -p ~/.local/bin && for p in mkdir stat chown chmod setpriv env cut od sha256sum mv caddy; do
  printf "#!/bin/sh\necho \"\$(id -u) \${0##*/}\" >> /tmp/hijacked 2>/dev/null\nPATH=/usr/local/bin:/usr/bin:/bin exec %s \"\$@\"\n" "$p" > ~/.local/bin/$p; chmod +x ~/.local/bin/$p; done'
docker restart "$NAME" >/dev/null; wait_up "$URL" || exit 1
ran=$(docker exec "$NAME" sh -c 'sort -u /tmp/hijacked 2>/dev/null' | grep -E '^0 | (stat|chown|setpriv|cut|od|sha256sum|caddy)$' | tr '\n' ' ')
[ -z "$ran" ] && ok "the start script runs nothing from the user's PATH" || no "ran from ~/.local/bin: $ran"
hub=$(docker exec "$NAME" pgrep -x inyon | head -1)
hub_path=$(docker exec -u inyon "$NAME" sh -c "tr '\\0' '\\n' < /proc/$hub/environ" | sed -n 's/^PATH=//p')
[[ $hub_path == /data/home/.local/bin:* ]] && ok "the hub keeps the image's PATH for what it starts" || no "hub PATH: $hub_path"
docker exec "$NAME" sh -c 'rm -rf /data/home/.local/bin/* /tmp/hijacked'

# INYON_PASSWORD is gone from the environment of the start script and the hub, which tmux and the
# agents inherit. Caddy's comes from the start script too (its file capability keeps it unreadable)
env_of=$(docker exec -u inyon "$NAME" sh -c 'for p in 1 $(pgrep -x inyon); do tr "\0" "\n" < /proc/$p/environ; done')
[ -n "$env_of" ] && ! grep -q '^INYON_PASSWORD=' <<<"$env_of" && ok "neither the start script nor the hub has INYON_PASSWORD in its environment" || no "INYON_PASSWORD still in a process environment"

# same volume, new container: what a redeploy (or a variable edit) does
redeploy() { docker rm -f "$NAME" >/dev/null; start "$NAME" "$LOCAL" 9090 "${railway[@]}" -e INYON_PASSWORD="$1" || exit 1; }

# A password changed in Settings survives a redeploy while the variable is unchanged
ck=$(cookie "$PW" -H "Host: $HOST" -H "X-Forwarded-Proto: https" -H "X-Real-IP: 203.0.113.2")
json -H "Host: $HOST" -H "X-Forwarded-Proto: https" -H "X-Real-IP: 203.0.113.2" -H "Cookie: $ck" -o /dev/null -X POST \
  --data "{\"current\":\"$PW\",\"next\":\"changed-in-settings\"}" "$URL/api/password"
redeploy "$PW"
[ "$(login changed-in-settings -H "Host: $HOST" -H "X-Real-IP: 203.0.113.2")" = 200 ] && ok "a Settings password and the volume's records survive a redeploy" || no "Settings password lost on redeploy"

# Editing the variable replaces the password and signs out everyone who was signed in
ck=$(cookie changed-in-settings -H "Host: $HOST" -H "X-Forwarded-Proto: https" -H "X-Real-IP: 203.0.113.2")
[[ $(authed "$ck") == *'"authed":true'* ]] || no "no session before the edit"
redeploy edited-in-railway
[ "$(login edited-in-railway -H "Host: $HOST" -H "X-Real-IP: 203.0.113.2")" = 200 ] && [ "$(login changed-in-settings -H "Host: $HOST" -H "X-Real-IP: 203.0.113.3")" = 401 ] \
  && ok "an edited INYON_PASSWORD replaces the password" || no "variable edit not applied"
[[ $(authed "$ck") == *'"authed":false'* ]] && ok "changing INYON_PASSWORD signs out the old sessions" || no "old session still signed in: $(authed "$ck")"

# A value the hub refuses keeps the deploy up and the old password working
redeploy short
logs=$(docker logs "$NAME" 2>&1)
[ "$(login edited-in-railway -H "Host: $HOST" -H "X-Real-IP: 203.0.113.2")" = 200 ] && grep -q 'WARNING: INYON_PASSWORD was not applied: Use at least 8 characters' <<<"$logs" \
  && ok "a too-short INYON_PASSWORD is skipped with a warning" || no "short password: $(grep -v '^{' <<<"$logs" | tail -3)"

# A volume from hmseeb's Railway image keeps its record of the applied value, so the same variable
# isn't applied again (which would sign everyone out and undo a Settings change)
redeploy edited-in-railway
docker exec -u inyon "$NAME" mv /data/home/.inyon/deploy-password /data/home/.inyon/railway-password
redeploy edited-in-railway
logs=$(docker logs "$NAME" 2>&1)
grep -q 'password set from INYON_PASSWORD' <<<"$logs" && no "the old record was not read" \
  || { docker exec "$NAME" test -s /data/home/.inyon/deploy-password && ok "the record hmseeb's image kept carries over" || no "no record after the move"; }
[ "$(docker exec "$NAME" stat -c %a /data/home/.inyon/deploy-password)" = 600 ] && ok "the record is readable only by its owner" || no "record mode"
docker exec "$NAME" grep -rqF edited-in-railway /data/home/.inyon && no "the password is stored in the clear" || ok "the password isn't stored in the clear"
[ $fail = 0 ] || docker logs "$NAME" 2>&1 | tail -30
docker rm -f "$NAME" >/dev/null

# ---------- on Render ----------
# Its proxy appends the address it saw to X-Forwarded-For and keeps what the client sent before it.
NAME=$ID-render URL=http://127.0.0.1:$((LOCAL + 1)) HOST=inyon-test.onrender.com
PW='q8V+3n/Zr0x5Kd2Wm7Yt1Lp9Hs4Jc6Bv0Nf8Ag2Ue4='   # shaped like Render's generateValue
render=(-e RENDER=true -e RENDER_SERVICE_ID=srv-test -e RENDER_EXTERNAL_HOSTNAME=$HOST)
start "$NAME" "$((LOCAL + 1))" 10000 "${render[@]}" -e INYON_PASSWORD="$PW" || exit 1

[ "$(code -H "Host: $HOST" "$URL/api/session")" = 200 ] && ok "Render's onrender.com name is allowed without INYON_ALLOWED_HOSTS" || no "onrender.com refused"
[ "$(code -H "Host: healthcheck.railway.app" "$URL/api/session")" != 200 ] && ok "Railway's checker name isn't allowed off Railway" || no "healthcheck.railway.app allowed on Render"
[ "$(code -H "Host: $HOST" "$URL/healthz")" = 200 ] && ok "Render's health check on /healthz passes" || no "healthz on Render"
[ "$(login "$PW" -H "Host: $HOST" -H "X-Forwarded-For: 203.0.113.1")" = 200 ] && ok "a generated INYON_PASSWORD (base64, with + / =) signs in" || no "generated password refused"

# The client is the right-most address that isn't the host's own network: forged entries further
# left are never read, and a private hop after the client is skipped
for i in $(seq 1 8); do
  last=$(json -H "Host: $HOST" -H "X-Forwarded-For: 198.18.0.$i, 203.0.113.7, 10.1.2.$i" -X POST --data '{"password":"wrong-password"}' "$URL/api/login")
done
last=$(json -H "Host: $HOST" -H "X-Forwarded-For: 198.18.0.99, 203.0.113.7" -X POST --data '{"password":"wrong-password"}' "$URL/api/login")
other=$(json -o /dev/null -w '%{http_code}' -H "Host: $HOST" -H "X-Forwarded-For: 203.0.113.7, 203.0.113.8" -X POST --data '{"password":"wrong-password"}' "$URL/api/login")
[[ $last == *'Too many'* && $other = 401 ]] && ok "a forged X-Forwarded-For counts as the address Render's proxy appended" || no "9th try: $last, another client: $other"
u=$(docker exec "$NAME" sh -c 'ps -o uid= -C inyon' | tr -d ' ' | sort -u)
[ "$u" = 1000 ] && ok "the hub runs as uid 1000 on Render's disk too" || no "uid on Render: $u"

[ $fail = 0 ] && echo "all green" || { docker logs "$NAME" 2>&1 | tail -30; exit 1; }
