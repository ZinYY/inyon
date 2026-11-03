#!/usr/bin/env bash
# Starts two throwaway sshd's as the current user on 127.0.0.1 for the integration tests:
#   $PORT     MaxSessions 32, so one connection can carry 12+ sessions
#   $PORT+1   MaxSessions 10 (OpenSSH's default), to show where SshCore needs a second connection
# Writes the INYON_SSH_TEST_* variables to $GITHUB_ENV when set, else prints them for `export`.
# Stop them with: kill $(cat "$DIR"/sshd-*.pid)
set -euo pipefail
dir=${1:?usage: start-sshd.sh <dir> [port]}
port=${2:-2222}
mkdir -p "$dir"
chmod 700 "$dir"
rm -f "$dir"/host_* "$dir"/client_*

ssh-keygen -q -t ed25519 -N '' -f "$dir/host_ed25519"
ssh-keygen -q -t ed25519 -N '' -C plain -f "$dir/client_ed25519"
ssh-keygen -q -t ed25519 -N 'pass phrase' -C ed-pass -f "$dir/client_ed25519_pass"
ssh-keygen -q -t rsa -b 3072 -N 'hunter2' -C rsa-pass -f "$dir/client_rsa_pass"
ssh-keygen -q -t rsa -b 2048 -m PEM -N '' -C rsa-pem -f "$dir/client_rsa_pem"
cat "$dir"/client_*.pub > "$dir/authorized_keys"
chmod 600 "$dir/authorized_keys"

sshd=/usr/sbin/sshd
start() {
  local p=$1 max=$2 cfg="$dir/sshd-$1.conf"
  cat > "$cfg" <<EOF
Port $p
ListenAddress 127.0.0.1
HostKey $dir/host_ed25519
PidFile $dir/sshd-$p.pid
AuthorizedKeysFile $dir/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
MaxSessions $max
MaxStartups 100
EOF
  # OpenSSH 9.8+ throttles sources that connect without logging in, as the probe test does
  if "$sshd" -t -f "$cfg" -o PerSourcePenalties=no 2>/dev/null; then echo "PerSourcePenalties no" >> "$cfg"; fi
  "$sshd" -f "$cfg" -E "$dir/sshd-$p.log"
}
start "$port" 32
start "$((port + 1))" 10

for p in "$port" "$((port + 1))"; do
  for _ in $(seq 1 20); do
    ssh -F /dev/null -i "$dir/client_ed25519" -p "$p" -o BatchMode=yes -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null "$(id -un)@127.0.0.1" true 2>/dev/null && break
    sleep 0.5
  done
done

fp=$(ssh-keygen -l -E sha256 -f "$dir/host_ed25519.pub" | awk '{print $2}')
vars="INYON_SSH_TEST_HOST=127.0.0.1
INYON_SSH_TEST_PORT=$port
INYON_SSH_TEST_PORT_DEFAULT=$((port + 1))
INYON_SSH_TEST_USER=$(id -un)
INYON_SSH_TEST_DIR=$dir
INYON_SSH_TEST_HOSTKEY=$fp"
if [ -n "${GITHUB_ENV:-}" ]; then echo "$vars" >> "$GITHUB_ENV"; else echo "$vars"; fi
