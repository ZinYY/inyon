#!/usr/bin/env bash
# Screenshots of the native screens, taken after android-smoke.sh on the same emulator: the QR
# scanner (camera permission prompt, viewfinder, idle hint), a photo input's camera prompt with
# its gallery fallback, and the app's notifications with their buttons. It drives the app through
# the WebView's DevTools socket, which debuggable (google_apis) emulator images open for any app.
#
# Best effort: it never decides the smoke verdict. Results go to $OUT/screens.
set -u
here=$(cd "$(dirname "$0")" && pwd)
out=${OUT:-${RUNNER_TEMP:-/tmp}/smoke}/screens
mkdir -p "$out"
pkg=tech.abhashchakraborty.inyon

shot() {
  sleep "${2:-2}"
  adb exec-out screencap -p > "$out/$1.png" && echo "screenshot: $1"
}

# taps the first (or, with "last", the last) view whose attributes match an ERE
tap_where() {
  adb shell uiautomator dump /sdcard/ui.xml > /dev/null 2>&1
  local b pick=head
  [ "${2:-}" = last ] && pick=tail
  b=$(adb shell cat /sdcard/ui.xml | tr '>' '\n' | grep -E "$1" | grep -o 'bounds="[^"]*"' | $pick -1 | tr -c '0-9\n' ' ')
  if [ -z "$b" ]; then
    echo "nothing on screen matches $1"
    return 1
  fi
  set -- $b
  adb shell input tap $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))
}

# taps a view by its resource id, e.g. the permission dialog's buttons
tap_id() {
  tap_where "resource-id=\"$1\""
}

js() {
  bun "$here/webview-eval.ts" "$ws" "$1"
}

pid=$(adb shell pidof $pkg | tr -d '\r')
[ -n "$pid" ] || { echo "the app is not running"; exit 1; }
adb forward tcp:9333 localabstract:webview_devtools_remote_$pid > /dev/null || { echo "no DevTools socket"; exit 1; }
ws=$(curl -s http://127.0.0.1:9333/json | grep -o '"webSocketDebuggerUrl": *"[^"]*"' | head -1 | sed 's/.*"\(ws[^"]*\)"/\1/')
[ -n "$ws" ] || { echo "no page to drive"; exit 1; }
echo "driving $ws"
js 'location.href'
# the page's CSP is in force and the theme script it allows still ran
js '[document.querySelector("meta[http-equiv=Content-Security-Policy]") ? "csp on" : "no csp", "theme " + (document.documentElement.dataset.theme || "unset"), typeof Capacitor.Plugins.InyonDevice].join(", ")'

# 1. a photo input with capture: the camera prompt, refused, opens the gallery picker instead
js '(() => { const i = document.createElement("input"); i.type = "file"; i.accept = "image/*"; i.setAttribute("capture", "environment"); i.onchange = () => (window.__picked = i.files.length); document.body.appendChild(i); i.click(); return "clicked" })()'
shot photo-camera-prompt 3
tap_id com.android.permissioncontroller:id/permission_deny_button
shot photo-gallery-fallback 4
adb shell dumpsys activity activities | grep -E 'topResumedActivity|ResumedActivity:' | head -2
adb shell input keyevent KEYCODE_BACK
sleep 2

# 2. the scanner: asks for the camera as it opens, then the viewfinder, then the idle hint
js 'void (window.__scan = Capacitor.Plugins.InyonDevice.scanQr({ haptics: true }).then((r) => (window.__scan = r)))'
shot scanner-camera-prompt 3
tap_id com.android.permissioncontroller:id/permission_allow_one_time_button \
  || tap_id com.android.permissioncontroller:id/permission_allow_foreground_only_button
shot scanner 5
shot scanner-idle 12
adb logcat -d -s InyonScanner:* | tail -20
adb shell input keyevent KEYCODE_BACK
sleep 2
echo "scanQr resolved: $(js 'window.__scan')"

# 3. the same photo input with the camera allowed goes to the camera app
js 'document.querySelector("input[capture]").click(), "clicked"'
shot photo-camera-app 4
adb shell input keyevent KEYCODE_BACK
sleep 2

# 4. notifications: an agent waiting (Open, Mark read), one done (Open), the keep-alive (Stop, Open)
adb shell pm grant $pkg android.permission.POST_NOTIFICATIONS
js 'Capacitor.Plugins.InyonDevice.startKeepAlive({ title: "Inyon is connected", text: "Keeping your sessions live so alerts arrive right away" })'
js 'Capacitor.Plugins.InyonDevice.postNotification({ title: "claude is done", body: "Refactored the session list", href: "/", tag: "box:%2", kind: "done" })'
js 'Capacitor.Plugins.InyonDevice.postNotification({ title: "claude needs you", body: "Allow the edit to src/App.tsx?", href: "/", tag: "box:%1", kind: "waiting" })'
sleep 2
adb shell cmd statusbar expand-notifications
shot notifications 3
adb shell dumpsys notification --noredact | grep -E 'NotificationRecord\(.*pkg=tech\.abhashchakraborty\.inyon|  \[[0-9]\] "' | cut -c1-160
adb shell cmd statusbar collapse

# 5. Stop on the keep-alive turns it off in the web app. It is a silent notification, shown
# minimised, so clear the agent ones and open it up first
js 'Promise.all([Capacitor.Plugins.InyonDevice.clearNotification({ tag: "box:%1" }), Capacitor.Plugins.InyonDevice.clearNotification({ tag: "box:%2" })]).then(() => "cleared")'
js 'new Promise((done) => { Capacitor.Plugins.InyonDevice.addListener("keepAliveStopped", (e) => done(e.reason)); setTimeout(() => done("no event"), 28000) })' > "$out/keepalive-event.txt" &
waiter=$!
sleep 1
adb shell cmd statusbar expand-notifications
sleep 2
stop_visible() {
  adb shell uiautomator dump /sdcard/ui.xml > /dev/null 2>&1
  adb shell cat /sdcard/ui.xml > "$out/shade.xml"
  grep -qE 'text="(Stop|STOP)"' "$out/shade.xml"
}
for target in 'text="Inyon is connected"' 'text="Inyon"' 'resource-id="android:id/expand_button"'; do
  stop_visible && break
  tap_where "$target" last > /dev/null && sleep 2
done
if stop_visible; then
  shot keep-alive-buttons 0
  tap_where 'text="(Stop|STOP)"' && echo "tapped Stop"
else
  # still not on screen: send the button's broadcast directly (the receiver isn't exported, so as root)
  echo "Stop not reachable on screen; sending its broadcast instead"
  adb root > /dev/null && adb wait-for-device
  adb shell am broadcast -n $pkg/.device.NotificationActions -a tech.abhashchakraborty.inyon.action.STOP_KEEP_ALIVE
fi
wait $waiter
echo "keepAliveStopped: $(cat "$out/keepalive-event.txt")"
adb shell cmd statusbar collapse
shot after-stop 1
adb forward tcp:9333 localabstract:webview_devtools_remote_$pid > /dev/null
echo "localStorage inyon.keepAlive: $(js 'localStorage.getItem("inyon.keepAlive")')"
echo "keep-alive notifications left: $(adb shell dumpsys notification | grep -c 'id=7001 ')"

