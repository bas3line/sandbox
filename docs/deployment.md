# Deployment

## Developer Compose stack

Generate two different tokens and start the stack:

```sh
export SANDBOX_API_TOKEN="$(openssl rand -hex 32)"
export SANDBOX_NODE_TOKEN="$(openssl rand -hex 32)"
docker compose -f deploy/compose/compose.yaml up --build
```

Optional profiles:

```sh
# Start the HTTPS API and public-tunnel edge on ports 80/443.
export SANDBOX_DOMAIN=sandbox.example.com
export SANDBOX_TUNNEL_ENABLED=true
export SANDBOX_TUNNEL_DOMAIN=tunnel.example.com
export SANDBOX_ACME_EMAIL=admin@example.com
docker compose -f deploy/compose/compose.yaml --profile edge up --build

# Start NATS JetStream, then select it for controller events.
export SANDBOX_BUS_KIND=nats
docker compose -f deploy/compose/compose.yaml --profile events up --build
```

For an origin-hidden Cloudflare deployment, use the `cloudflare-edge` overlay. It runs an outbound-only connector and an internal HTTP tunnel edge without publishing ports 80 or 443:

```sh
export SANDBOX_BIND_ADDRESS=127.0.0.1
export SANDBOX_TUNNEL_ENABLED=true
export SANDBOX_TUNNEL_ENTRYPOINT=web
export SANDBOX_TUNNEL_EDGE_TLS=false

docker compose \
  -f deploy/compose/compose.yaml \
  -f deploy/compose/compose.cloudflare.yaml \
  --profile cloudflare-edge up --build -d
```

The connector token is a file-backed Compose secret. Cloudflare route, certificate, token-file, and firewall setup are documented in [tunnels.md](tunnels.md#cloudflare-tunnel-with-a-hidden-origin).

The worker mounts the Docker socket. This is a developer/single-tenant topology. It also raises the AEGIS microVM threshold to 101. Both choices are deliberately visible in the Compose file.

## Production topology

Deploy these trust zones separately:

1. Edge: TLS 1.3, OIDC/mTLS authentication, request limits, WAF, and only the controller API origin. Prefer an outbound connector or restrict origin ingress to the proxy network.
2. Control: at least two `sandboxd --role controller` instances and PostgreSQL with backups/PITR.
3. Worker pools: dedicated hosts grouped by isolation capability, region, sensitivity, and workload class.
4. Egress: explicit artifact mirrors and authenticated HTTP/SOCKS proxies; no direct tenant route to management networks.
5. Observability: JSON logs, PostgreSQL health, node heartbeat alerts, audit/event consumers, and runtime metrics.

Do not expose workers publicly. Allow worker-to-controller API traffic, image/artifact access, approved egress, and centralized telemetry only.

## Reverse proxy and domains

The `edge` Compose profile routes the controller API using `SANDBOX_DOMAIN` and reconciles exact-host HTTP/WebSocket tunnel routes below `SANDBOX_TUNNEL_DOMAIN`. Tunnel routing is disabled unless `SANDBOX_TUNNEL_ENABLED=true`.

When an existing Caddy installation owns ports 80/443, use the Caddy Compose overlay and controller-backed on-demand TLS authorization. See [tunnels.md](tunnels.md) for DNS, TLS, configuration, isolation, and verification. Raw TCP forwarding is not implemented.

When Cloudflare must hide the origin, use the Cloudflare Tunnel overlay instead of pointing a proxied `A` record at the host. A nested wildcard requires an Advanced Cloudflare edge certificate; Universal SSL covers only the apex and first-level subdomains in a full DNS setup.

## Systemd

Units are provided in `deploy/systemd/`. Install binaries and configuration, then:

```sh
install -d -o sandbox -g sandbox /etc/sandbox /var/lib/sandbox
install -m 0640 -o root -g sandbox config/sandbox.example.toml /etc/sandbox/sandbox.toml
install -m 0640 -o root -g sandbox /path/to/sandbox.env /etc/sandbox/sandbox.env
install -m 0644 deploy/systemd/sandboxd-controller.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now sandboxd-controller
```

Use the worker unit only on dedicated execution hosts. Add its user to the runtime-specific group and verify the socket permissions.

## Release installation

Once GitHub Releases contains signed archives and checksum sidecars:

```sh
curl -fsSL https://tools.yshubham.com/sandbox/install.sh | sh
```

The installer supports Linux and macOS on amd64/arm64 and installs `sandbox`, `sandboxd`, and `sandbox-mcp`. It requires a checksum and fails closed. For controlled environments, mirror the release, verify provenance in CI, and set `SANDBOX_INSTALL_BASE_URL` to the approved HTTPS mirror.

Tagged releases also receive signed SLSA provenance through GitHub artifact attestations. Verify a downloaded archive before installation:

```sh
gh attestation verify sandbox_v0.1.0_linux_amd64.tar.gz --repo bas3line/sandbox
```

## PostgreSQL

The controller applies its idempotent initial schema at startup. Production operators should still manage migrations as a reviewed deployment step, create a least-privilege database role, require TLS, and monitor connection saturation and slow queries.

Back up database state for audit and control-plane recovery. Sandbox disks remain runtime-owned and ephemeral.

## Upgrades

1. Back up PostgreSQL.
2. Roll controllers first; keep at least one healthy API instance.
3. Mark a worker draining before replacement (the v0.1 API does not expose this operation; update its node record operationally or stop new scheduling upstream).
4. Upgrade runtime drivers and worker daemon together when the driver protocol changes.
5. Verify create, exec, tunnel create/delete if enabled, TTL deletion, event delivery, and node loss in a staging pool.
