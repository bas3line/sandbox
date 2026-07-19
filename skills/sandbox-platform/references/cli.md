# CLI command map

## Connection

```sh
sandbox config set-server https://sandbox.example.com
export SANDBOX_TOKEN='read-from-your-secret-store'
sandbox doctor
sandbox config show
```

The config file stores only the controller URL. `--server` and `SANDBOX_URL` override it. Global options are `--server`, `--token`, and `--json`; put them before the final `--` argument delimiter. Prefer an environment or secret store for the token so it does not enter shell history.

## Create

```sh
sandbox create \
  --tenant "$TENANT" \
  --image "$IMAGE" \
  --cpu-millis 2000 \
  --memory-mib 4096 \
  --disk-mib 20480 \
  --pids 512 \
  --ttl 3600 \
  --network restricted \
  --isolation auto \
  --sensitivity confidential \
  --untrusted-repo \
  --generated-code \
  --needs-secrets \
  --expose 3000 \
  --label team=platform
```

An optional startup command follows `--`. Create waits until ready by default. Use `--no-wait` only when the caller will save the operation ID and call `sandbox wait`.

## Inspect and list

```sh
sandbox list --tenant "$TENANT"
sandbox list --tenant "$TENANT" --all
sandbox inspect "$SANDBOX_ID"
sandbox --json inspect "$SANDBOX_ID"
```

Normal list output excludes stopped and failed audit records. Use `--all` when those records are needed.

## Execute

```sh
sandbox exec "$SANDBOX_ID" --cwd /workspace --timeout 900 -- cargo test --workspace
sandbox exec "$SANDBOX_ID" --env RUST_BACKTRACE=1 -- cargo test
```

Arguments after `--` remain an argv vector. Use `--no-wait` only when the caller will track the printed operation ID.

## Operations and cleanup

```sh
sandbox wait "$OPERATION_ID" --timeout 900
sandbox delete "$SANDBOX_ID"
```

Deletion waits and reports completion by default. Use `--no-wait` only when tracking the operation separately.

## Public sharing

Share a service running on the agent's current machine:

```sh
sandbox http 3000
```

The command checks both local IPv4 and IPv6, connects to the hosted Sandbox WebSocket relay, prints a temporary URL on the deployment's `*.tunnel.yshubham.com` wildcard, and stays attached until Ctrl-C. It does not require `SANDBOX_URL`. `SANDBOX_HTTP_RELAY` or `--relay` selects a self-hosted relay. Treat the URL as public and never expose credentials, private data, or admin interfaces.

For a service inside a managed sandbox, make it listen on `0.0.0.0`, then use the controller-managed tunnel commands:

```sh
sandbox tunnel create "$SANDBOX_ID" --port 3000
sandbox tunnel list "$SANDBOX_ID"
sandbox tunnel delete "$SANDBOX_ID" "$TUNNEL_ID"
```

Use `sandbox tunnel create SANDBOX_ID --port PORT --subdomain review-42` only when the caller needs a stable human-readable label. Managed tunnel availability remains capability-gated by the server.

## Coding agents

```sh
sandbox agent list
sandbox agent run codex --tenant "$TENANT"
sandbox agent run opencode --tenant "$TENANT" -- --version
sandbox agent run opencode --tenant "$TENANT" \
  --image registry.example.com/agents/opencode@sha256:... -- --version
```

Without agent-specific arguments, the command provisions an agent-ready sandbox. Arguments after `--` run through the observable exec path. Local images must exist on every eligible worker; CommandCode always requires `--image`. Agent profiles default to restricted egress, so pass `--network open` only when external model access is required.

## MCP configuration

```sh
sandbox --server https://sandbox.example.com mcp-config
```

Treat the printed JSON as a starting point. Put tokens in the MCP client's secret store or environment-forwarding configuration.
