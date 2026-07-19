# Configuration

`sandboxd` reads `sandbox.toml` by default, an explicit `--config` path, and environment variables. Environment variables use the `SANDBOX` prefix and double underscores for nesting. Environment values override file values.

```text
SANDBOX__SERVER__BIND=0.0.0.0:8080
SANDBOX__STORE__KIND=postgres
SANDBOX__STORE__DATABASE_URL=postgres://...
SANDBOX__NODE__RUNTIME=external
```

Never commit token or database credential values. A reviewed non-secret example is in [config/sandbox.example.toml](../config/sandbox.example.toml).

## Server

| Key | Default | Meaning |
|---|---:|---|
| `server.bind` | `0.0.0.0:8080` | Controller listen address |
| `server.api_token` | none | Operator bearer token; minimum 32 characters |
| `server.node_token` | none | Worker bearer token; minimum 32 characters and distinct |
| `server.allow_unauthenticated_dev` | `false` | Explicit local-only bypass |
| `server.heartbeat_interval_seconds` | `10` | Worker heartbeat cadence |
| `server.heartbeat_timeout_seconds` | `45` | Stale-node scheduling cutoff |
| `server.assignment_lease_seconds` | `30` | Redelivery lease |
| `server.request_body_limit_bytes` | `1048576` | Maximum request body |

## Storage

| Key | Default | Meaning |
|---|---:|---|
| `store.kind` | `memory` | `memory` or `postgres` |
| `store.database_url` | none | PostgreSQL DSN; required for Postgres |
| `store.max_connections` | `16` | Diesel async pool ceiling |

The memory store disappears on restart and is never an HA option.

## Event bus

| Key | Default | Meaning |
|---|---:|---|
| `bus.kind` | `memory` | `memory` or `nats` |
| `bus.nats_url` | `nats://127.0.0.1:4222` | NATS connection |
| `bus.subject` | `sandbox.events` | Lifecycle subject |

## Policy

| Key | Default | Meaning |
|---|---:|---|
| `policy.max_ttl_seconds` | `86400` | Per-request TTL ceiling |
| `policy.microvm_risk_threshold` | `55` | AEGIS isolation cutoff |
| `policy.max_output_bytes` | `1048576` | Per-stream captured output cap |

Changing the microVM threshold is a security-policy change. Review it like a firewall rule.

## Worker

| Key | Default | Meaning |
|---|---:|---|
| `node.control_plane_url` | `http://127.0.0.1:8080` | Controller URL |
| `node.token` | none | Worker bearer token |
| `node.name` | `sandboxd` | Stable operator-facing name |
| `node.region` / `node.zone` | `local` / `local-a` | Placement locality |
| `node.runtime` | `docker` | `docker` or `external` |
| `node.external_driver` | none | Driver executable path |
| `node.docker_restricted_network` | `sandbox-egress` | Pre-created controlled network |
| `node.resources` | 8 CPU / 16 GiB / 100 GiB / 4096 PIDs | Advertised capacity |
| `node.max_sandboxes` | `100` | Concurrent sandbox ceiling |
| `node.state_dir` | `/var/lib/sandbox` | Stable node ID and future journal |
| `node.labels` | empty | Hard placement labels |

Advertise allocatable capacity after system and runtime reserves, not raw host capacity.

## Public tunnels

Tunnels are disabled by default. Both controller and workers need the same `tunnel.base_domain` and `tunnel.public_scheme`; Docker workers also reconcile the edge route directory and private per-sandbox networks.

```text
SANDBOX__TUNNEL__ENABLED=true
SANDBOX__TUNNEL__BASE_DOMAIN=tunnel.example.com
SANDBOX__TUNNEL__PUBLIC_SCHEME=https
```

See [tunnels.md](tunnels.md) for every key, wildcard DNS, direct Traefik, Caddy on-demand TLS, outbound Cloudflare Tunnel ingress, lifecycle behavior, and troubleshooting. `SANDBOX_PORT=127.0.0.1:8080` limits the optional Compose host port to loopback when a private connector is the only ingress path.

Keep `tunnel.public_scheme = "https"` for normal deployments. Set it to `http` only for the documented fixed-proxied-wildcard compatibility mode, and pair it with an HTTP edge entrypoint and disabled edge TLS so returned URLs match reality.
