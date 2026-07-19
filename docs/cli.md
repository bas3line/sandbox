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

## Share a local service

```sh
sandbox http 3000
sandbox http 4321 --provider cloudflare
```

`sandbox http PORT` checks that the port is listening on local IPv4 or IPv6, starts a temporary public tunnel, prints its HTTPS URL, and stays attached until Ctrl-C. It does not contact `SANDBOX_URL` and does not require a running Sandbox controller.

The default `--provider auto` prefers an installed `cloudflared`; if it is unavailable, it uses the system SSH client with [localhost.run](https://localhost.run/). Override it with `--provider cloudflare` or `--provider localhost-run`, or set `SANDBOX_HTTP_PROVIDER`. [Cloudflare Quick Tunnels](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/) are intended for development and testing. The localhost.run path is a third-party SSH relay. Treat either URL as public and do not share admin panels, credentials, or private data.

## Public tunnels from managed sandboxes

A service inside a managed sandbox must bind `0.0.0.0`. Then create, inspect, and remove its controller-managed route:

```sh
sandbox tunnel create "$ID" --port 3000
sandbox tunnel create "$ID" --port 8080 --subdomain review-42
sandbox tunnel list "$ID"
sandbox tunnel delete "$ID" "$TUNNEL_ID"
```

Controller-managed tunnel mutations wait by default; pass `--no-wait` to manage the operation separately. The server enforces sensitivity, worker capability, tunnel count, and protocol policy. Use `--subdomain` only for a stable label. See [tunnels.md](tunnels.md).

## Agents

```sh
sandbox agent list
sandbox agent run codex --tenant engineering
sandbox agent run opencode --tenant engineering --image registry.example.com/opencode@sha256:...
```

See [agents.md](agents.md) before supplying credentials or custom images.

## Exit behavior

Connection and API errors produce a non-zero CLI exit. A waited remote command exits the local CLI with the bounded remote exit code. JSON output does not include bearer tokens.
