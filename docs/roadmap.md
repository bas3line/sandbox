# Roadmap and production gates

## Identity and tenancy

- Native OIDC/JWT validation, tenant claims, service accounts, and per-action RBAC.
- Node-specific mTLS identity, automatic rotation, and revocation.
- Quotas, budgets, admission webhooks, and approval policies.

## Runtime and networking

- Reference Firecracker jailer driver with snapshot pools and vsock agent.
- gVisor/Kata drivers and capability probes.
- Identity-aware tunnel authentication, raw TCP leases, and SSH certificates.
- Domain-aware egress proxy, DNS policy, image signatures, SBOM admission, and malware scanning.

## Execution

- Interactive PTY/WebSocket streams, reconnect, resize, and session recording.
- Persistent node journal for exactly-once-visible exec behavior.
- Runtime reconciliation, draining API, prewarming, snapshots, and checkpoint/restore.
- Repository service, patch export, artifact cache, and secret-reference API.

## Control plane

- PostgreSQL transactions and transactional outbox.
- OpenAPI, pagination, idempotency keys, audit query API, and fine-grained rate limits.
- Prometheus/OpenTelemetry metrics and SLO dashboards.
- Kubernetes operator and Helm chart.

No roadmap item should be represented as an implemented security property until its enforcement and bypass tests land.
