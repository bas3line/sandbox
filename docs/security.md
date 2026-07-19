# Security model

## Trust boundaries

Sandbox assumes the controller, PostgreSQL, worker daemon, runtime driver, host kernel/hypervisor, and enterprise identity proxy are trusted. Sandbox code, repositories, package hooks, generated commands, and model output are untrusted.

Agent prompts, `SKILL.md`, MCP descriptions, and CLI defaults are behavioral guidance. They are not security boundaries. Enforcement lives in API validation, controller policy, node eligibility, runtime configuration, host networking, and credentials that prevent bypass.

## Supported isolation claims

### Docker adapter

The built-in adapter hardens containers, but containers share the host kernel. Mounting `/var/run/docker.sock` gives the worker process host-equivalent authority. Use it only on a dedicated worker machine or a trusted single-tenant developer installation. Never run the controller on that host merely for convenience in a hostile multi-tenant environment.

### External driver

Use a VMM-grade driver for hostile tenants or high-risk agents. Firecracker should run through its jailer on a hardened KVM host. Kata or gVisor may fit organizations already standardized on containerd/Kubernetes. The driver must enforce resources, networking, immutable base images, teardown, and idempotency.

## Authentication in v0.1

The controller has separate static bearer tokens for operators and workers, requires at least 32 characters, rejects identical values, and compares them in constant time. This is enough for a private pilot behind TLS, not a complete enterprise identity plane.

For production, place the controller behind an OIDC/mTLS gateway that maps identity and tenant claims, rotate short-lived credentials, and block direct reachability to the controller. Per-tenant RBAC and node-specific credentials are roadmap items and must be implemented or enforced upstream before a hostile multi-tenant rollout.

## Command and secret handling

- Exec accepts argv, not a shell string.
- The Docker adapter invokes the OCI runtime without a shell for exec.
- Output is bounded and may be truncated.
- The public API rejects privileged workloads.
- Plaintext secret fields are intentionally absent. Do not put credentials in argv, labels, image references, or ordinary environment values.
- A secret-capable deployment should resolve opaque references inside the trusted runtime driver through workload identity.

## Network policy

`deny` attaches no network. `restricted` attaches the configured worker network; the shipped Compose network is internal and therefore has no external egress. To allow selected destinations, attach a controlled egress proxy and enforce destinations outside the sandbox. `open` uses the Docker bridge and should be denied for sensitive tenants by organization policy.

Public tunnels are an independent ingress choice. The Docker worker creates one internal network per tunneled sandbox and attaches only that sandbox and the edge. Routes use exact hostnames and are removed with the tunnel or sandbox. URLs are unauthenticated and Internet-facing; confidential/restricted workloads, raw TCP, and requested tunnel authentication are rejected. Put an identity-aware proxy in front if your deployment needs user authentication.

## Host hardening checklist

- Dedicated worker hosts with minimal packages and automatic security updates.
- KVM/IOMMU and a VMM jailer for microVM pools.
- Rootless runtime where compatible; no controller credentials on workers.
- Default-deny ingress/egress and separate management, artifact, and tenant networks.
- Immutable, digest-pinned images with signatures and SBOM verification.
- cgroup v2 limits, seccomp, AppArmor/SELinux, and audit logging.
- Short-lived node identity, disk encryption, secure boot, and centralized log export.
- Alerting for stale nodes, repeated lease retries, policy rejection spikes, and unexpected open egress.

## Known gaps before hostile production

- Static tokens instead of native OIDC/RBAC.
- At-least-once exec delivery without a persistent node journal.
- No interactive PTY/session recording.
- No built-in secret broker, image signature verifier, or domain egress proxy.
- PostgreSQL writes are not yet wrapped in one transaction/outbox.
- Worker startup does not yet reconcile pre-existing runtime instances.
- Public HTTP/WebSocket tunnels do not include built-in user authentication or abuse/rate limiting.

Treat these as explicit engineering gates, not documentation footnotes.

## Reporting vulnerabilities

Do not open a public issue for a suspected vulnerability. Follow [SECURITY.md](../SECURITY.md).
