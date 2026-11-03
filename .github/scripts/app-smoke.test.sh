#!/usr/bin/env bash
# Checks app-smoke.sh's log and sample handling without an emulator: adb is a stand-in that
# answers from files.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0
check() { if eval "$2"; then echo "ok: $1"; else echo "FAILED: $1"; fail=1; fi; }

# adb shell cat /proc/<pid>/stat: a process the stand-in knows, or nothing (gone)
adb() {
  case "$*" in
    "shell cat /proc/"*"/stat") local p=${3#/proc/}; p=${p%/stat}; [ -f "$tmp/stat-$p" ] && cat "$tmp/stat-$p" ;;
    *) return 0 ;;
  esac
}

OUT=$tmp/out APP_SMOKE_LIB=1 . "$here/app-smoke.sh"

# utime 7 + stime 3 after the name
echo '41 (main) S 1 1 0 0 -1 0 0 0 0 0 7 3 0 0' > "$tmp/stat-41"
echo '42 (renderer) S 1 1 0 0 -1 0 0 0 0 0 20 5 0 0' > "$tmp/stat-42"
check "ticks adds user and system time" '[ "$(ticks 41)" = 10 ]'
check "sum adds every process" '[ "$(sum ticks 41 42)" = 35 ]'
check "sum gives nothing when one process is gone" '[ -z "$(sum ticks 41 43)" ]'

cat > "$tmp/own.txt" <<LOG
11-02 10:00:00.000  900  900 E AndroidRuntime: FATAL EXCEPTION: main
11-02 10:00:00.000  900  900 E AndroidRuntime: Process: tech.abhashchakraborty.inyon, PID: 900
LOG
cat > "$tmp/other.txt" <<LOG
11-02 10:00:00.000  800  800 E AndroidRuntime: FATAL EXCEPTION: main
11-02 10:00:00.000  800  800 E AndroidRuntime: Process: com.example.other, PID: 800
11-02 10:00:01.000  500  500 I ActivityManager: Force stopping tech.abhashchakraborty.inyon appid=10190
LOG
cat > "$tmp/native.txt" <<LOG
11-02 10:00:00.000  901  901 F DEBUG   : pid: 900, tid: 950, name: tokio  >>> tech.abhashchakraborty.inyon <<<
LOG
cat > "$tmp/anr.txt" <<LOG
11-02 10:00:00.000  500  520 E ActivityManager: ANR in tech.abhashchakraborty.inyon (tech.abhashchakraborty.inyon/dev.dioxus.main.MainActivity)
LOG
check "the app's own exception is a crash" 'crashed "$tmp/own.txt"'
check "another app's exception and a force stop are not" '! crashed "$tmp/other.txt"'
check "a native crash in the app is a crash" 'crashed "$tmp/native.txt"'
check "an ANR is a crash" 'crashed "$tmp/anr.txt"'

exit $fail
