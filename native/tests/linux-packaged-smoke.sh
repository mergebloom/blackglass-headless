#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo 'usage: linux-packaged-smoke.sh SERVER_BINARY BGH_BINARY' >&2
  exit 2
fi
server="$1"
client="$2"
test_dir=$(mktemp -d /tmp/blackglass-native-linux.XXXXXX)
server_pid=
service_pid=
cleanup() {
  if [ -n "$service_pid" ]; then
    kill -TERM "$service_pid" 2>/dev/null || true
    wait "$service_pid" 2>/dev/null || true
  fi
  if [ -n "$server_pid" ]; then
    kill -INT "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -r -- "$test_dir"
}
trap cleanup EXIT

control_port=54881
data_port=54882
origin="http://127.0.0.1:$control_port"
if curl --silent --max-time 1 "$origin/health" >/dev/null 2>&1; then
  echo 'test control port is already occupied' >&2
  exit 2
fi
export SELFHOST_BIND_HOST=127.0.0.1
export SELFHOST_CONTROL_PORT="$control_port"
export SELFHOST_DATA_PORT="$data_port"
export SELFHOST_DATA_HOST="127.0.0.1:$data_port"
export SELFHOST_DATABASE="$test_dir/server.sqlite"
export SELFHOST_STAGING_DIR="$test_dir/uploads"
export SELFHOST_ALLOWED_ORIGINS=app://obsidian.md

printf 'synthetic-account-password\n' | "$server" user create "$SELFHOST_DATABASE" native-linux@example.test 'Native Linux Smoke'
start_server() {
  "$server" serve > "$test_dir/server.log" 2>&1 &
  server_pid=$!
  count=0
  until curl --silent --fail --max-time 1 "$origin/health" >/dev/null; do
    count=$((count + 1))
    if [ "$count" -ge 100 ]; then
      echo 'server failed to start' >&2
      exit 1
    fi
    sleep 0.1
  done
}
start_server

profile_a="$test_dir/profile-a.json"
profile_b="$test_dir/profile-b.json"
mkdir "$test_dir/vault-a" "$test_dir/vault-b"
"$client" --profile "$profile_a" configure --server "$origin"
"$client" --profile "$profile_b" configure --server "$origin"
printf 'synthetic-account-password\n' | "$client" --profile "$profile_a" login --email native-linux@example.test --password-stdin
printf 'synthetic-account-password\n' | "$client" --profile "$profile_b" login --email native-linux@example.test --password-stdin
created=$(printf 'synthetic-vault-password\n' | "$client" --profile "$profile_a" vault create --name Linux-Smoke --password-stdin)
vault_id=$(printf '%s\n' "$created" | sed -n 's/.*(\([^)]*\)).*/\1/p')
if [ -z "$vault_id" ]; then echo 'vault creation did not return an ID' >&2; exit 1; fi
printf 'synthetic-vault-password\n' | "$client" --profile "$profile_a" vault connect --id "$vault_id" --path "$test_dir/vault-a" --password-stdin
printf '# From Linux A\n' | "$client" --profile "$profile_a" note write a.md
"$client" --profile "$profile_a" sync once
printf 'synthetic-vault-password\n' | "$client" --profile "$profile_b" vault connect --id "$vault_id" --path "$test_dir/vault-b" --password-stdin
"$client" --profile "$profile_b" sync once
cmp "$test_dir/vault-a/a.md" "$test_dir/vault-b/a.md"

printf '# From Linux B\n' | "$client" --profile "$profile_b" note write b.md
"$client" --profile "$profile_b" sync once
"$client" --profile "$profile_a" sync once
cmp "$test_dir/vault-a/b.md" "$test_dir/vault-b/b.md"

rm -- "$test_dir/vault-a/a.md"
"$client" --profile "$profile_a" sync once
"$client" --profile "$profile_b" sync once
test ! -e "$test_dir/vault-b/a.md"

"$server" backup "$SELFHOST_DATABASE" "$test_dir/backup.sqlite"
"$server" verify "$test_dir/backup.sqlite"
kill -INT "$server_pid"
wait "$server_pid"
server_pid=
start_server
"$client" --profile "$profile_a" sync once
"$client" --profile "$profile_b" sync once
cmp "$test_dir/vault-a/b.md" "$test_dir/vault-b/b.md"

"$client" --profile "$profile_a" service run --interval-seconds 1 > "$test_dir/service.log" 2>&1 &
service_pid=$!
count=0
until [ -S "$test_dir/profile-a.sock" ]; do
  kill -0 "$service_pid" 2>/dev/null || { echo 'service exited before socket readiness' >&2; exit 1; }
  count=$((count + 1))
  if [ "$count" -ge 100 ]; then echo 'service socket did not become ready' >&2; exit 1; fi
  sleep 0.1
done
count=0
until printf '# Background Linux Sync\n' | "$client" --profile "$profile_a" note write background.md >/dev/null 2>&1; do
  count=$((count + 1))
  if [ "$count" -ge 100 ]; then echo 'service did not accept local note write' >&2; exit 1; fi
  sleep 0.1
done
count=0
until "$client" --profile "$profile_b" sync once >/dev/null 2>&1 && [ -e "$test_dir/vault-b/background.md" ]; do
  count=$((count + 1))
  if [ "$count" -ge 100 ]; then echo 'background note did not reach second profile' >&2; exit 1; fi
  sleep 0.1
done
cmp "$test_dir/vault-a/background.md" "$test_dir/vault-b/background.md"
awk '/^VmRSS:/ {print "idle service " $2 " " $3}' "/proc/$service_pid/status"
kill -TERM "$service_pid"
wait "$service_pid"
service_pid=
test ! -e "$test_dir/profile-a.sock"

if grep -aFq '# From Linux B' "$SELFHOST_DATABASE"; then
  echo 'server database contains plaintext canary' >&2
  exit 1
fi
echo 'packaged Linux client Sync smoke passed'
