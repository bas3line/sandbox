# CLI

## Connection

```sh
sandbox config set-server https://sandbox.example.com
export SANDBOX_TOKEN='redacted'
sandbox doctor
sandbox config show
```

The saved configuration contains only the controller URL. Keep the token in your shell, service manager, or secret store; the CLI never writes it to the config file. `--server` and `SANDBOX_URL` override the saved URL, in that order.

`--server`, `--token`, and `--json` are global options and work before or after a subcommand. Put them before the final `--` that separates agent or exec arguments. `--json` is intended for automation. Token values are hidden by Clap diagnostics.

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

Create waits for the sandbox to become ready and prints its ID, state, selected node, isolation, and expiry. Pass `--no-wait` only when a caller will track the returned operation ID. Add repeatable `--expose PORT` or `--expose PORT=SUBDOMAIN` flags to allocate public HTTP/WebSocket URLs during creation. Tunnels must be enabled by the deployment.

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
sandbox list --tenant engineering --all
sandbox inspect "$ID"
sandbox delete "$ID"
```

Normal list output shows active records. `--all` includes stopped and failed audit records. Deletion waits for cleanup by default; `--no-wait` returns the operation ID immediately. Runtime resources, including the sandbox workspace volume, are removed while the stopped control-plane record remains for audit.

## Share a local service

```sh
sandbox http 3000
sandbox http 4321 --subdomain design-review
SANDBOX_HTTP_RELAY=https://relay.tunnel.example.com sandbox http 8080
```

`sandbox http PORT` checks that the port is listening on local IPv4 or IPv6, connects to the hosted Sandbox relay over an outbound WebSocket, prints the temporary HTTPS URL, and stays attached until Ctrl-C. The default relay is `https://relay.tunnel.yshubham.com`; `SANDBOX_HTTP_RELAY` or `--relay` selects a self-hosted deployment. `SANDBOX_TOKEN` is sent when the selected relay requires operator authentication.

The relay supports ordinary HTTP plus WebSocket upgrades such as Vite HMR. It detects whether the app is listening on IPv4 (`127.0.0.1`) or IPv6 (`::1`) and forwards to that exact loopback address. It deliberately does not preserve the public `Host` or `Origin`, so a development server never has to trust a random hostname. The route is exact-host, expires at the server TTL, and is removed immediately when the CLI disconnects. The URL is unauthenticated and public; do not share admin panels, credentials, or private data.

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
sandbox agent run opencode --tenant engineering -- --version
sandbox agent run opencode --tenant engineering \
  --image registry.example.com/opencode@sha256:... -- --version
```

Running a profile without agent arguments provisions an agent-ready sandbox and prints the next `sandbox exec` command. Arguments after `--` run through the observable exec path, so stdout, stderr, timeout, and exit status are preserved. The agent sandbox remains available until deletion or TTL expiry. Use `--network open` only when the agent must contact an external model API; the default is restricted egress.

Codex, Claude Code, OpenCode, and Pi use local worker images. Build them on every eligible worker with `./scripts/build-agent-image.sh NAME`, or pass an immutable registry image. CommandCode always requires `--image` because no public default image is assumed.

See [agents.md](agents.md) before supplying credentials or custom images.

## Exit behavior

Connection and API errors produce a non-zero CLI exit. A waited remote command exits the local CLI with the bounded remote exit code. JSON output does not include bearer tokens.

## Deployment smoke test

Run the destructive, self-cleaning command-surface check against a test controller before a release:

```sh
SANDBOX_URL=https://sandbox.example.com \
SANDBOX_TOKEN='read-from-your-secret-store' \
./scripts/smoke-test-cli.sh
```

Set `SANDBOX_CLI=/path/to/sandbox` to test an unpacked binary. The script uses the isolated `sandbox-cli-smoke` tenant, creates a short-lived Python sandbox, verifies synchronous and asynchronous operations, preserves a deliberate exit code `7`, checks both tunnel workflows over public HTTPS, deletes the sandbox, and confirms that only its stopped audit record remains.
