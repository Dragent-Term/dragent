#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p dragent_host --bin dragent-host
fixture=$(mktemp -d /private/tmp/dragent-ssh-test.XXXXXX 2>/dev/null || mktemp -d /tmp/dragent-ssh-test.XXXXXX)
sshd_pid=
cleanup() {
  endpoint=$(find "$fixture/home" -name host-v1.json -print -quit 2>/dev/null || true)
  if [ -n "$endpoint" ]; then DRAGENT_HOST_ENDPOINT_FILE="$endpoint" target/debug/dragent-host --dragent-stop-host || true; fi
  if [ -n "$sshd_pid" ]; then kill "$sshd_pid" 2>/dev/null || true; wait "$sshd_pid" 2>/dev/null || true; fi
  # Fixture services are stopped by the acceptance test. Preserve diagnostics
  # after failure; no process or home outside this directory is ever stopped.
  if [ "${test_result:-1}" = 0 ]; then rm -rf "$fixture"; else echo "SSH fixture logs: $fixture"; fi
}
trap cleanup EXIT
mkdir "$fixture/home"
ssh-keygen -q -t ed25519 -N '' -f "$fixture/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$fixture/client_key"
cp "$fixture/client_key.pub" "$fixture/authorized_keys"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
cat > "$fixture/force-command" <<EOF
#!/bin/sh
export HOME='$fixture/home'
export SHELL=/bin/sh
exec /bin/sh -c "\$SSH_ORIGINAL_COMMAND"
EOF
chmod 700 "$fixture/force-command"
cat > "$fixture/sshd_config" <<EOF
Port $port
ListenAddress 127.0.0.1
HostKey $fixture/host_key
PidFile $fixture/sshd.pid
AuthorizedKeysFile $fixture/authorized_keys
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers $(id -un)
AllowTcpForwarding yes
AllowStreamLocalForwarding yes
ForceCommand $fixture/force-command
EOF
cat > "$fixture/ssh_config" <<EOF
Host *
  UserKnownHostsFile $fixture/known_hosts
  StrictHostKeyChecking accept-new
EOF
/usr/sbin/sshd -D -e -f "$fixture/sshd_config" > "$fixture/sshd.log" 2>&1 &
sshd_pid=$!
sleep 1
kill -0 "$sshd_pid"
python3 - "$fixture" "$port" "$(id -un)" <<'PY'
import json,sys
fixture,port,user=sys.argv[1:]
with open(fixture+'/host.json','w') as f:
    json.dump({'host':'127.0.0.1','user':user,'port':int(port),'identity_file':fixture+'/client_key','config_file':fixture+'/ssh_config','forward_agent':False,'authentication':'Key'},f)
PY
export DRAGENT_TEST_SSH_CONFIG="$fixture/host.json"
export DRAGENT_REMOTE_BINARY="$PWD/target/debug/dragent-host"
if [ "${1:-}" = "--serve" ]; then
  echo "SSH fixture ready: $fixture"
  echo "Remote binary: $DRAGENT_REMOTE_BINARY"
  echo "Press Enter to stop the isolated fixture."
  read -r _
  test_result=0
  exit 0
fi
test_result=0
cargo test -p dragent_host --test remote_ssh -- --ignored --nocapture || test_result=$?
exit "$test_result"
