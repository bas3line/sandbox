# HTTP API

The controller serves JSON over HTTP. Put TLS and enterprise identity at the edge. Operator endpoints use the operator bearer token; worker endpoints use the separate node token.

| Method | Path | Caller | Purpose |
|---|---|---|---|
| `GET` | `/healthz`, `/readyz` | public/internal probe | Health and version |
| `POST` | `/v1/sandboxes` | operator | Validate, score, schedule, and enqueue create |
| `GET` | `/v1/sandboxes` | operator | List; optional `tenant` query |
| `GET` | `/v1/sandboxes/{id}` | operator | Inspect |
| `POST` | `/v1/sandboxes/{id}/exec` | operator | Enqueue argv execution |
| `DELETE` | `/v1/sandboxes/{id}` | operator | Enqueue teardown |
| `GET` | `/v1/operations/{id}` | operator | Poll operation |
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

Create, exec, and delete are asynchronous. A successful enqueue returns an operation record; clients poll until `succeeded` or `failed`. Command exit status and bounded output are stored on the operation.

The Rust structs under `crates/core/src/api.rs` and `model.rs` are the canonical v0.1 schema. OpenAPI generation and streaming transports are roadmap items.
