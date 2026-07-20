#!/bin/sh
set -eu

: "${SANDBOX_URL:?set SANDBOX_URL to the controller}"
: "${SANDBOX_TOKEN:?set SANDBOX_TOKEN from your secret store}"

cli=${SANDBOX_CLI:-sandbox}
tenant=${SANDBOX_SMOKE_TENANT:-sandbox-cli-smoke}
suffix=$(date +%s)-$$
subdomain="cli-smoke-$suffix"
sandbox_id=

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf '%s\n' "missing required command: $1" >&2
    exit 1
  }
}

run_cli() {
  "$cli" --server "$SANDBOX_URL" "$@"
}

cleanup() {
  if [ -n "$sandbox_id" ]; then
    run_cli delete "$sandbox_id" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_command "$cli"
require_command curl
require_command jq

run_cli --json doctor | jq -e '.status == "ok"' >/dev/null
run_cli --json agent list | jq -e 'length == 7' >/dev/null
run_cli mcp-config | jq -e '.mcpServers.sandbox.command == "sandbox-mcp"' >/dev/null

create=$(run_cli --json create \
  --tenant "$tenant" \
  --image python:3.13-alpine \
  --cpu-millis 500 \
  --memory-mib 256 \
  --disk-mib 256 \
  --pids 64 \
  --ttl 600 \
  --network restricted \
  --expose "3000=$subdomain" \
  --no-wait \
  -- python -m http.server 3000 --bind 0.0.0.0)
sandbox_id=$(printf '%s\n' "$create" | jq -er '.sandbox.id')
create_operation=$(printf '%s\n' "$create" | jq -er '.operation.id')
run_cli --json wait "$create_operation" --timeout 120 | jq -e '.state == "succeeded"' >/dev/null

inspect=$(run_cli --json inspect "$sandbox_id")
printf '%s\n' "$inspect" | jq -e '.state == "running"' >/dev/null
public_url=$(printf '%s\n' "$inspect" | jq -er '.tunnels[0].public_url')
curl --fail --silent --show-error --max-time 20 "$public_url" >/dev/null
run_cli --json tunnel list "$sandbox_id" | jq -e 'length == 1 and .[0].state == "active"' >/dev/null

first_tunnel=$(printf '%s\n' "$inspect" | jq -er '.tunnels[0].id')
tunnel_delete=$(run_cli --json tunnel delete "$sandbox_id" "$first_tunnel" --no-wait)
tunnel_delete_operation=$(printf '%s\n' "$tunnel_delete" | jq -er '.operation.id')
run_cli --json wait "$tunnel_delete_operation" --timeout 120 | jq -e '.state == "succeeded"' >/dev/null

tunnel_create=$(run_cli --json tunnel create "$sandbox_id" --port 3000)
second_tunnel=$(printf '%s\n' "$tunnel_create" | jq -er '.tunnel.id')
second_url=$(printf '%s\n' "$tunnel_create" | jq -er '.tunnel.public_url')
curl --fail --silent --show-error --max-time 20 "$second_url" >/dev/null
run_cli --json tunnel delete "$sandbox_id" "$second_tunnel" | jq -e '.state == "succeeded"' >/dev/null

run_cli --json exec "$sandbox_id" --cwd /workspace --env MESSAGE=ready -- \
  /bin/sh -c 'printf "%s\n" "$MESSAGE" > smoke.txt && cat smoke.txt' \
  | jq -e '.output.exit_code == 0 and .output.stdout == "ready\n"' >/dev/null

async_exec=$(run_cli --json exec "$sandbox_id" --no-wait -- /bin/sh -c 'printf "async-ok\n"')
async_operation=$(printf '%s\n' "$async_exec" | jq -er '.id')
run_cli --json wait "$async_operation" --timeout 120 \
  | jq -e '.output.exit_code == 0 and .output.stdout == "async-ok\n"' >/dev/null

set +e
failed_exec=$(run_cli --json exec "$sandbox_id" -- /bin/sh -c 'printf "expected failure\n" >&2; exit 7')
failed_status=$?
set -e
[ "$failed_status" -eq 7 ]
printf '%s\n' "$failed_exec" \
  | jq -e '.state == "failed" and .output.exit_code == 7' >/dev/null

run_cli --json list --tenant "$tenant" | jq -e --arg id "$sandbox_id" 'any(.id == $id)' >/dev/null

delete=$(run_cli --json delete "$sandbox_id" --no-wait)
delete_operation=$(printf '%s\n' "$delete" | jq -er '.id')
run_cli --json wait "$delete_operation" --timeout 120 | jq -e '.state == "succeeded"' >/dev/null
sandbox_id=

run_cli --json list --tenant "$tenant" | jq -e 'length == 0' >/dev/null
run_cli --json list --tenant "$tenant" --all | jq -e 'any(.state == "stopped")' >/dev/null

printf '%s\n' "Sandbox CLI smoke test passed against $SANDBOX_URL"
