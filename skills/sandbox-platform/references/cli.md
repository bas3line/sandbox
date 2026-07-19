# CLI command map

## Connection

```sh
export SANDBOX_URL=https://sandbox.example.com
export SANDBOX_TOKEN='read-from-your-secret-store'
sandbox doctor
```

Global options are `--server`, `--token`, and `--json`. Prefer environment variables so secrets do not enter shell history.

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

An optional startup command follows `--`. Save both the sandbox ID and create operation ID. Wait for the operation before executing.

## Inspect and list

```sh
sandbox list --tenant "$TENANT"
sandbox inspect "$SANDBOX_ID"
sandbox --json inspect "$SANDBOX_ID"
```

## Execute

```sh
sandbox exec "$SANDBOX_ID" --cwd /workspace --timeout 900 -- cargo test --workspace
sandbox exec "$SANDBOX_ID" --env RUST_BACKTRACE=1 -- cargo test
```

Arguments after `--` remain an argv vector. Use `--no-wait` only when the caller will track the printed operation ID.

## Operations and cleanup

```sh
sandbox wait "$OPERATION_ID" --timeout 900
sandbox delete "$SANDBOX_ID" --wait
```

## Public tunnels

Make the intended HTTP/WebSocket service listen on `0.0.0.0`, then:

```sh
sandbox tunnel create "$SANDBOX_ID" --port 3000
sandbox tunnel list "$SANDBOX_ID"
sandbox tunnel delete "$SANDBOX_ID" "$TUNNEL_ID"
```

Use `--subdomain review-42` only when the caller needs a stable human-readable label. Every returned URL is public. Do not expose admin interfaces or services containing credentials.

## Coding agents

```sh
sandbox agent list
sandbox agent run codex --tenant "$TENANT"
sandbox agent run opencode --tenant "$TENANT" --image registry.example.com/agents/opencode@sha256:...
```

Agent-specific arguments follow `--`.

## MCP configuration

```sh
sandbox --server https://sandbox.example.com mcp-config
```

Treat the printed JSON as a starting point. Put tokens in the MCP client's secret store or environment-forwarding configuration.
