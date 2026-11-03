#!/usr/bin/env bash
# Runs the x86_64 Android builds on the emulator against the runner's sshd (start-sshd.sh), reached
# through adb reverse: 10.0.2.2 needs the emulator's network, which is often not up right after boot.
# The key tests need ssh-keygen, which Android lacks, so they stay on the runner; so does the
# phone-seed test, which edits the runner's authorized_keys.
set -eu
bin=$1
keys=$2
d=/data/local/tmp/s2
adb wait-for-device
adb reverse tcp:2222 tcp:2222
adb reverse tcp:2223 tcp:2223
adb shell "rm -rf $d && mkdir -p $d/sshd"
adb push "$bin"/exec "$bin"/test-sshd "$bin"/test-none_auth $d/ > /dev/null
for k in client_ed25519 client_ed25519_pass client_rsa_pass client_rsa_pem authorized_keys; do adb push "$keys/$k" $d/sshd/ > /dev/null; done
adb shell "chmod 755 $d/exec $d/test-*"
fp=$(ssh-keygen -l -E sha256 -f "$keys/host_ed25519.pub" | awk '{print $2}')
env="INYON_SSH_TEST_HOST=127.0.0.1 INYON_SSH_TEST_PORT=2222 INYON_SSH_TEST_PORT_DEFAULT=2223 INYON_SSH_TEST_USER=$(id -un) INYON_SSH_TEST_DIR=$d/sshd INYON_SSH_TEST_HOSTKEY=$fp"

echo "== exec on the emulator"
adb shell "$d/exec 127.0.0.1 2222 $(id -un) $d/sshd/client_ed25519 'uname -sm; echo from \$(hostname)'"
echo "== tests on the emulator"
adb shell "cd $d && $d/test-none_auth"
adb shell "cd $d && $env $d/test-sshd --skip logs_in_with_the_phone_seed --nocapture"
echo "== device"
adb shell getprop ro.build.version.release
adb shell uname -m
