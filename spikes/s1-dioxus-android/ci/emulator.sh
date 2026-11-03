#!/usr/bin/env bash
# Runs the spike APK on the emulator and writes what it saw to $OUT: summary.md, logcat, screenshots.
# Fails only when the app crashes or never gets its terminal up.
set -u
apk=$1
own=${2:-}
out=${OUT:-/tmp/s1-out}
here=$(cd "$(dirname "$0")" && pwd)
pkg=tech.abhashchakraborty.inyon.spike
act=$pkg/dev.dioxus.main.MainActivity
mkdir -p "$out"
sum="$out/summary.md"
: > "$sum"
note() { echo "$*" | tee -a "$sum"; }
shot() {
  # the broadcast would also close the notification shade, so that screenshot skips it
  [ "${2:-}" = keep ] || adb shell am broadcast -a android.intent.action.CLOSE_SYSTEM_DIALOGS > /dev/null 2>&1
  adb exec-out screencap -p > "$out/$1.png" 2>/dev/null || true
}
spike_log() { adb logcat -d -s InyonSpike:I; }
wait_log() { # pattern seconds
  for _ in $(seq 1 "$2"); do spike_log | grep -q "$1" && return 0; sleep 1; done
  return 1
}

adb wait-for-device
# a slow emulator often shows "Pixel Launcher isn't responding" over everything; hide such dialogs
adb shell settings put global hide_error_dialogs 1
adb shell am broadcast -a android.intent.action.CLOSE_SYSTEM_DIALOGS > /dev/null
adb install -r "$apk" || { note "install failed"; exit 1; }
adb shell pm grant $pkg android.permission.POST_NOTIFICATIONS || true
note "## Emulator (API 34, x86_64, pixel_6, swiftshader)"
note ""
note "APK: $(stat -c %s "$apk") bytes (x86_64; the arm64 size is in the apk job)"
note ""

# cold starts: force-stop, then start and wait for the terminal to report in
note "| run | am start TotalTime | Rust main | first render | terminal ready |"
note "|---|---|---|---|---|"
for run in 1 2 3 4; do
  adb shell am force-stop $pkg
  sleep 2
  adb logcat -c
  total=$(adb shell am start -W -n $act | tr -d '\r' | awk -F': ' '/TotalTime/ {print $2}')
  if wait_log "terminal ready" 90; then
    l=$(spike_log)
    m=$(grep -o 'main started [0-9]* ms' <<< "$l" | grep -o '[0-9]*' | head -1)
    f=$(grep -o 'first render [0-9]* ms' <<< "$l" | grep -o '[0-9]*' | head -1)
    t=$(grep -o 'terminal ready [0-9]* ms' <<< "$l" | grep -o '[0-9]*' | head -1)
    note "| $run | ${total} ms | ${m} ms | ${f} ms | ${t} ms |"
  else
    note "| $run | ${total} ms | never got ready | | |"
  fi
done
note ""
note "Run 1 is the first start after install; the other columns count from process start (Process.getStartUptimeMillis, read over JNI)."
note ""

pid=$(adb shell pidof $pkg | tr -d '\r')
adb logcat -d -v threadtime > "$out/logcat-start.txt"
if [ -z "$pid" ] || grep -q "FATAL EXCEPTION" "$out/logcat-start.txt"; then
  note "**The app crashed or is not running.**"
  echo fail > "$out/verdict"
  exit 1
fi
spike_log | grep "terminal ready" | tail -1 | sed 's/^.*terminal ready/terminal ready/' | tee -a "$sum"
note ""
sleep 2
shot 01-chat

# DevTools for clicks and reading the page
adb forward tcp:9333 localabstract:webview_devtools_remote_$pid > /dev/null
cdp() { timeout 25 bun "$here/cdp.ts" 9333 "$@"; }
click() { cdp eval "document.querySelector('$1').click()" > /dev/null; }
for _ in $(seq 1 20); do curl -s http://127.0.0.1:9333/json > /dev/null && break; sleep 1; done
note "page metrics: \`$(cdp eval 'JSON.stringify(window.__spike)')\`"
note ""
# where the WebView sits on screen, so a page position becomes an `input tap` position
adb shell uiautomator dump /sdcard/ui.xml > /dev/null 2>&1
origin=$(adb shell cat /sdcard/ui.xml | tr -d '\r' | grep -o 'class="android.webkit.WebView"[^>]*bounds="\[[0-9]*,[0-9]*\]' | grep -o '\[[0-9]*,[0-9]*\]' | head -1 | tr -d '[]' | tr ',' ' ')
origin=${origin:-0 0}
note "WebView top-left on screen: $origin"
tap() { # a real finger tap on the element, through the input system
  local r ox oy
  r=$(cdp rect "$1") || return 1
  read -r ox oy <<< "$origin"
  set -- $r
  echo "tap at $(( ox + $1 )),$(( oy + $2 ))" >> "$out/taps.txt"
  adb shell input tap $(( ox + $1 )) $(( oy + $2 ))
}
focus() { adb shell dumpsys window | tr -d '\r' | grep -m1 -oE 'mCurrentFocus=[^ ]+ [^ ]+ [^ }]+'; }

click '[aria-label=Terminal]'
sleep 5
note "terminal pane visible: $(cdp eval "getComputedStyle(document.querySelector('.term')).visibility")"
shot 02-terminal
# the same screen on xterm's DOM renderer, to tell a WebGL fault from a swiftshader one
cdp eval 'window.__gl && window.__gl.dispose()' > /dev/null
sleep 3
shot 02b-terminal-dom
click '[aria-label=Chat]'
click '[aria-label="Open sidebar"]'
sleep 2
shot 03-sidebar
click '.scrim'
sleep 1

# soft keyboard: a real tap on the composer should bring it up and type through the IME
adb logcat -c
tap '[aria-label=Message]'
sleep 3
shot 04a-after-tap
note "after the tap ($(tail -1 "$out/taps.txt")): window $(focus)"
kb=$(adb shell dumpsys input_method | tr -d '\r' | grep -oE 'mInputShown=(true|false)' | head -1)
note "keyboard after a tap on the composer: ${kb:-unknown}"
note "focused element: $(cdp eval 'document.activeElement.getAttribute("aria-label")')"
if [ "$kb" != "mInputShown=true" ]; then
  # back to the app, focus from script: key events still reach the page, without the keyboard
  adb shell am start -n $act > /dev/null
  sleep 2
  cdp eval "document.querySelector('[aria-label=Message]').focus()" > /dev/null
  note "no keyboard: focused the composer from script instead"
fi
adb shell input text 'hello%sfrom%sthe%skeyboard'
sleep 2
shot 04-keyboard
typed=$(spike_log | grep -o 'composer: .*' | tail -1)
note "Rust saw: \`${typed:-nothing}\`"
tap '[aria-label=Send]'
sleep 1
sent=$(spike_log | grep -o 'sent: .*' | tail -1)
note "after Send: \`${sent:-nothing}\`"
cdp eval 'document.activeElement.blur()' > /dev/null

# typing into the terminal (xterm's own hidden textarea)
adb shell am start -n $act > /dev/null
sleep 2
click '[aria-label=Terminal]'
sleep 3
tap '.term'
sleep 2
adb shell input text 'ls%s-la'
adb shell input keyevent KEYCODE_ENTER
sleep 2
shot 05-terminal-typing
note "terminal input seen by Rust: \`$(spike_log | grep -o 'terminal input: .*' | head -4 | tr '\n' ' ')\`"
cdp eval 'document.activeElement.blur()' > /dev/null

# the keep-alive service, started from Rust over JNI
fgs=$(adb shell dumpsys activity services $pkg | tr -d '\r')
echo "$fgs" > "$out/services.txt"
if grep -q "isForeground=true" <<< "$fgs"; then note "keep-alive service: running in the foreground"; else note "keep-alive service: NOT in the foreground"; fi
note "$(adb logcat -d -s InyonSpike:I | grep -E 'keep-alive' | sed 's/^.*InyonSpike: //' | sort -u | tr '\n' ';')"
adb shell cmd statusbar expand-notifications
sleep 2
shot 06-notification keep
adb shell cmd statusbar collapse

mem=$(adb shell dumpsys meminfo $pkg | tr -d '\r' | grep -E 'TOTAL PSS|TOTAL:' | head -1)
note "memory: $mem"
adb logcat -d -v threadtime > "$out/logcat.txt"
if grep -q "FATAL EXCEPTION" "$out/logcat.txt"; then note "**crashed during the run**"; echo fail > "$out/verdict"; exit 1; fi

# the APK from the app's own Gradle project, installed over the dx one as an upgrade (same id and
# key, higher versionCode)
if [ -f "$own" ]; then
  note ""
  note "### Own Gradle project APK"
  if adb install -r "$own" > /dev/null; then
    note "installed over the dx build: $(adb shell dumpsys package $pkg | tr -d '\r' | grep -m1 -o 'versionCode=[0-9]*')"
    adb shell am force-stop $pkg
    adb logcat -c
    total=$(adb shell am start -W -n $act | tr -d '\r' | awk -F': ' '/TotalTime/ {print $2}')
    if wait_log "terminal ready" 90; then
      note "started: am start ${total} ms, $(spike_log | grep -o 'terminal ready [0-9]* ms' | head -1)"
      sleep 2
      shot 07-own-gradle
      if adb shell dumpsys activity services $pkg | grep -q "isForeground=true"; then note "keep-alive: in the foreground"; else note "keep-alive: not running"; fi
    else
      note "**never got ready**"
    fi
    adb logcat -d -v threadtime > "$out/logcat-own.txt"
    grep -q "FATAL EXCEPTION" "$out/logcat-own.txt" && note "**crashed**"
  else
    note "install failed"
  fi
fi
echo pass > "$out/verdict"
exit 0
