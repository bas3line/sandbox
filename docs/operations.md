# Operations runbook

## Health

- `/healthz`: process and store construction succeeded.
- Worker logs: registration, heartbeat failures, lease failures, and runtime errors.
- PostgreSQL: connection count, transaction latency, storage growth, backups, and replica lag.
- Node freshness: alert before `server.heartbeat_timeout_seconds` is exceeded.

The v0.1 health endpoint does not execute a live database query on every request. Use a separate synthetic create/exec/delete canary for end-to-end health.

## Common incidents

### `no_capacity`

Check healthy node timestamps, advertised available resources, max sandbox count, labels, pressure, and supported isolation tiers. Do not raise the risk threshold as an incident shortcut.

### Assignments repeat

A worker failed to report completion before lease expiry. Create/delete drivers must be idempotent. Exec can be delivered at least once; inspect side effects before retrying manually.

### Restricted egress cannot reach a dependency

The shipped `sandbox-egress` network is intentionally internal. Add the destination to the controlled proxy/mirror path. Do not switch the tenant to open egress without policy approval.

### Tunnel operation fails

Check that the worker advertises tunnel support, the configured edge container is running, the shared route directory is mounted at both paths, and wildcard DNS points at the public edge. The sandbox service must bind `0.0.0.0`. For TLS failures, inspect edge ACME logs and verify the active hostname returns `204` from `/v1/tunnels/authorize?domain=...`; unknown or inactive names must return `404`.

For Cloudflare Tunnel deployments, DNS should resolve to Cloudflare rather than the origin. Check `cloudflared` connector health and verify the published wildcard route targets `http://tunnel-edge:8080`. A nested wildcard needs an Advanced edge certificate; Cloudflare Universal SSL does not cover it, and Total TLS does not issue certificates for Tunnel hostnames. Confirm the public path works before closing ingress, then verify the origin ports are unreachable from outside the management network.

### Worker compromised

1. Drain or network-isolate the host.
2. Rotate the global node token immediately; v0.1 does not have per-node revocation.
3. Identify assignments and tenants placed on the worker from database/audit events.
4. Revoke workload credentials and rebuild the host from a known image.
5. Preserve logs and disks according to the incident policy.

### Controller token exposed

Block edge access, rotate the operator token, search API logs for use, reconcile every sandbox/operation created in the exposure window, and move to short-lived upstream identity before reopening.

## Backup and restore

Back up PostgreSQL with tested point-in-time recovery. The control database restores desired state and audit records, not ephemeral sandbox disks. After restore, stop scheduling until workers and runtime instances are reconciled; v0.1 requires manual reconciliation.

## Capacity

Advertise allocatable resources after host, runtime, image cache, and emergency reserves. Track peak concurrent sandboxes and tunnels, dominant requested resources, image pull latency, create latency, exec duration, output truncation, lease retries, edge certificate/routing failures, and risk-tier demand. Add microVM workers before lowering isolation policy.
