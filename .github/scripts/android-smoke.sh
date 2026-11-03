#!/usr/bin/env bash
# Installs an APK on the running emulator, starts it, and checks logcat: no crash, nothing R8
# stripped that the app reaches for, both native plugins loaded and the web app started.
#
# Writes its verdict to $OUT/verdict ("pass" or "fail: ..."). No verdict means the emulator
# itself misbehaved, and the workflow may retry.
set -u
apk=$1
out=${OUT:-${RUNNER_TEMP:-/tmp}/smoke}
mkdir -p "$out"
rm -f "$out/verdict"
log="$out/logcat.txt"

verdict() {
  echo "$1" > "$out/verdict"
  echo "verdict: $1"
}

adb wait-for-device
if ! result=$(adb install -r "$apk" 2>&1); then
  echo "$result"
  # a rejected APK is our fault; anything else (device offline, timeouts) is the emulator's
  if grep -qE 'INSTALL_FAILED|INSTALL_PARSE_FAILED|Failure \[' <<< "$result"; then verdict "fail: the APK did not install"; fi
  exit 1
fi

adb logcat -c
adb shell am start -W -n tech.abhashchakraborty.inyon/.MainActivity || exit 1
sleep 25
pid=$(adb shell pidof tech.abhashchakraborty.inyon | tr -d '\r')
adb logcat -d -v threadtime > "$log"
adb exec-out screencap -p > "$out/screen.png" || true

# threadtime lines: date time pid tid level tag: message
mine=$(awk -v p="${pid:-none}" '$3 == p' "$log")
problems=()
if grep -qE 'FATAL EXCEPTION|ANR in tech\.abhashchakraborty\.inyon' "$log" && grep -qE 'Process: tech\.abhashchakraborty\.inyon|ANR in tech\.abhashchakraborty\.inyon' "$log"; then
  problems+=("the app crashed")
fi
[ -n "$pid" ] || problems+=("the app is not running")
if grep -qE 'ClassNotFoundException|NoClassDefFoundError|NoSuchMethodError|NoSuchMethodException|NoSuchFieldError|AbstractMethodError' <<< "$mine"; then
  problems+=("missing classes or methods (R8)")
fi
grep -qE 'InyonDevice *: ready' <<< "$mine" || problems+=("InyonDevice did not load")
grep -qE 'InyonSsh *: ready' <<< "$mine" || problems+=("InyonSsh did not load")
grep -qE 'InyonDevice *: web app started' <<< "$mine" || problems+=("the web app did not start")

# A hub on a private network is plain http, which Android lets an app call only when its manifest
# says so: ask the running app to fetch from a server on the runner (10.0.2.2 from the emulator).
# Then check that no frame but the app's own can reach the native side.
if [ -n "$pid" ]; then
  here=$(cd "$(dirname "$0")" && pwd)
  bun "$here/smoke-server.ts" > /dev/null 2>&1 &
  www_pid=$!
  sleep 2
  curl -s -o /dev/null -w "the test server answers on the runner: %{http_code}\n" http://127.0.0.1:4777/ok
  adb forward tcp:9333 localabstract:webview_devtools_remote_$pid > /dev/null 2>&1
  wsurl=$(curl -s http://127.0.0.1:9333/json | grep -o '"webSocketDebuggerUrl": *"[^"]*"' | head -1 | sed 's/.*"\(ws[^"]*\)"/\1/')
  if [ -n "$wsurl" ]; then
    js() { bun "$here/webview-eval.ts" "$wsurl" "$1" 2>&1; }
    http=$(js 'fetch("http://10.0.2.2:4777/ok", { cache: "no-store" }).then((r) => "http " + r.status).catch((e) => "http failed: " + e.message)')
    echo "plain http from the app: $http"
    [[ "$http" == *"http 200"* ]] || problems+=("the app cannot call a plain-http hub: $http")

    # the web app checks a plain-http hub's name against the addresses it points to, looked up natively
    looked=$(js 'Capacitor.Plugins.InyonDevice.lookUp({ host: "localhost" }).then((r) => JSON.stringify(r.addresses)).catch((e) => "failed: " + e.message)')
    echo "localhost looked up natively: $looked"
    [[ "$looked" == *127.0.0.1* || "$looked" == *::1* ]] || problems+=("InyonDevice.lookUp does not work: $looked")

    # Capacitor's bridge must stay with the app's own top frame. Its cookie and HTTP plugins add
    # JavaScript interfaces Android puts in every frame, so the app removes them; and an agent's
    # page under /v/, in the app's sandboxed frame, must see no native object at all.
    top=$(js '[typeof window.CapacitorCookiesAndroidInterface, typeof window.CapacitorHttpAndroidInterface].join(",")')
    echo "the app's own frame, cookie and HTTP interfaces: $top"
    [[ "$top" == '"undefined,undefined"' ]] || problems+=("Capacitor's cookie or HTTP interface is still in the WebView: $top")
    isolated=$(js 'Capacitor.Plugins.InyonDevice.framesIsolated().then((r) => String(r.isolated)).catch((e) => "failed: " + e.message)')
    echo "frames isolated from the bridge (WebMessageListener): $isolated"
    frames=$(js '(() => {
      const probe = (sandbox) => new Promise((done) => {
        const f = document.createElement("iframe")
        f.setAttribute("sandbox", sandbox)
        f.src = "http://10.0.2.2:4777/v/probe/"
        const on = (e) => {
          if (e.source !== f.contentWindow || !e.data || !e.data.probe) return
          removeEventListener("message", on)
          f.remove()
          done(e.data)
        }
        addEventListener("message", on)
        document.body.appendChild(f)
        setTimeout(() => done({ timeout: true }), 15000)
      })
      // the Pages frame, then the dev server preview (which has same-origin rights on its own site)
      return Promise.all([probe("allow-scripts allow-forms allow-popups allow-downloads"), probe("allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads")]).then((r) => JSON.stringify(r))
    })()')
    echo "what a page in the app's frames can see: $frames"
    if [[ "$frames" != *'probe'* ]] || [[ "$frames" == *timeout* ]]; then
      problems+=("the frame probe got no answer: $frames")
    elif [[ "$frames" =~ (object|function) ]]; then
      problems+=("a page in a frame can reach a native object: $frames")
    fi
  else
    echo "no DevTools socket, so plain http and the frames were not checked"
  fi
  kill "$www_pid" 2> /dev/null || true
fi
echo "---- app log (pid ${pid:-none}) ----"
grep -E 'AndroidRuntime|Inyon|Capacitor|chromium|System.err|Exception|Error' <<< "$mine" | tail -80 || true
grep -A30 'FATAL EXCEPTION' "$log" | head -60 || true

if [ ${#problems[@]} -eq 0 ]; then
  verdict "pass"
else
  verdict "fail: $(IFS=';'; echo "${problems[*]}")"
  exit 1
fi
