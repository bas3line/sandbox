# CLI

## Connection

```sh
export SANDBOX_URL=https://sandbox.example.com
export SANDBOX_TOKEN='redacted'
sandbox doctor
```

`--json` is global and intended for automation. Tokens are accepted from the environment with values hidden by Clap diagnostics.

## Create

```sh
sandbox create \
  --tenant engineering \
  --image ghcr.io/example/dev@sha256:... \
  --cpu-millis 4000 \
  --memory-mib 8192 \
  --disk-mib 30720 \
  --pids 512 \
  --ttl 3600 \
  --network restricted \
  --isolation auto \
  --sensitivity confidential \
  --untrusted-repo \
  --generated-code \
  --needs-secrets
```

Add repeatable `--expose PORT` or `--expose PORT=SUBDOMAIN` flags to allocate public HTTP/WebSocket URLs during creation. Tunnels must be enabled by the deployment.

Pass an optional detached startup command after `--`. Prefer creating the sandbox first and using `sandbox exec` so operation output is visible.

## Execute

```sh
sandbox exec "$ID" --timeout 900 --cwd /workspace -- cargo test --workspace
```

Arguments after the sandbox ID are passed as argv. Shell operators such as `|`, `&&`, redirects, and variable expansion do not run unless the caller explicitly invokes a shell executable.

Use `--no-wait` to receive an operation ID, then:

```sh
sandbox wait "$OPERATION_ID" --timeout 900
```

## Lifecycle

```sh
sandbox list --tenant engineering
sandbox inspect "$ID"
sandbox delete "$ID" --wait
```

Deletion removes runtime resources. The stopped control-plane record remains for audit.

## Public tunnels

The service must bind `0.0.0.0` inside the sandbox. Then create, inspect, and remove its public route:

```sh
sandbox tunnel create "$ID" --port 3000
sandbox tunnel create "$ID" --port 8080 --subdomain review-42
sandbox tunnel list "$ID"
sandbox tunnel delete "$ID" "$TUNNEL_ID"
```

Tunnel mutations wait by default; pass `--no-wait` to manage the operation separately. Treat every printed URL as public. See [tunnels.md](tunnels.md).

## Agents

```sh
sandbox agent list
sandbox agent run codex --tenant engineering
sandbox agent run opencode --tenant engineering --image registry.example.com/opencode@sha256:...
```

See [agents.md](agents.md) before supplying credentials or custom images.

## Exit behavior

Connection and API errors produce a non-zero CLI exit. A waited remote command exits the local CLI with the bounded remote exit code. JSON output does not include bearer tokens.
