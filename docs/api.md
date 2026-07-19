# HTTP API

The controller serves JSON over HTTP. Put TLS and enterprise identity at the edge. Operator endpoints use the operator bearer token; worker endpoints use the separate node token.

| Method | Path | Caller | Purpose |
|---|---|---|---|
| `GET` | `/healthz`, `/readyz` | public/internal probe | Health and version |
| `POST` | `/v1/sandboxes` | operator | Validate, score, schedule, and enqueue create |
| `GET` | `/v1/sandboxes` | operator | List; optional `tenant` query |
| `GET` | `/v1/sandboxes/{id}` | operator | Inspect |
| `POST` | `/v1/sandboxes/{id}/exec` | operator | Enqueue argv execution |
| `POST` | `/v1/sandboxes/{id}/tunnels` | operator | Enqueue public HTTP/WebSocket route creation |
| `DELETE` | `/v1/sandboxes/{id}/tunnels/{tunnel_id}` | operator | Enqueue route removal |
| `DELETE` | `/v1/sandboxes/{id}` | operator | Enqueue teardown |
| `GET` | `/v1/operations/{id}` | operator | Poll operation |
| `GET` | `/v1/tunnels/authorize?domain=...` | public edge | Return `204` only for an active exact hostname |
| `POST` | `/v1/nodes/register` | worker | Upsert capacity/capabilities |
| `POST` | `/v1/nodes/{id}/heartbeat` | worker | Update availability and pressure |
| `GET` | `/v1/nodes/{id}/assignments` | worker | Lease work |
| `POST` | `/v1/assignments/complete` | worker | Report bounded result |

## Errors

```json
{
  "code": "no_capacity",
  "message": "no healthy node satisfies isolation, labels, and resource requirements",
  "request_id": null
}
```

Stable codes currently include `invalid_request`, `unauthorized`, `not_found`, `conflict`, `no_capacity`, and `internal`.

## Semantics

Create, exec, tunnel changes, and delete are asynchronous. A successful enqueue returns an operation record; clients poll until `succeeded` or `failed`. Command exit status and bounded output are stored on the operation.

`POST /v1/sandboxes` accepts optional `spec.exposures` entries containing `container_port`, `protocol: "http"`, optional lowercase `subdomain`, and `authenticated: false`. Post-create tunnel creation accepts the same fields. Every returned `public_url` is Internet-facing. See [tunnels.md](tunnels.md) for supported protocols and policy restrictions.

The Rust structs under `crates/core/src/api.rs` and `model.rs` are the canonical v0.1 schema. OpenAPI generation and streaming transports are roadmap items.
