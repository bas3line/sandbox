# External runtime driver protocol

An external driver is an executable selected with:

```toml
[node]
runtime = "external"
external_driver = "/usr/local/libexec/sandbox-firecracker"
```

The worker invokes the executable with one action argument, writes one JSON request to stdin, and reads one JSON response from stdout. Logs belong on stderr.

## Envelope

```json
{
  "protocol_version": "sandbox.runtime.v1",
  "sandbox_id": "019f...",
  "payload": {}
}
```

Success response:

```json
{
  "success": true,
  "result": {},
  "error": null
}
```

Failure response:

```json
{
  "success": false,
  "result": null,
  "error": "bounded operator-safe message"
}
```

## Actions

### `probe`

Return capabilities:

```json
{
  "success": true,
  "result": {
    "name": "firecracker-jailer",
    "version": "1.15.1",
    "tiers": ["microvm"]
  }
}
```

### `create`

The payload contains `spec` and the selected `isolation`. Creation must be idempotent for a sandbox ID, enforce every resource and network limit, and return only after the runtime is ready.

### `exec`

The payload is a `CommandSpec`. Return `exit_code`, `stdout`, `stderr`, and `truncated`. Do not invoke a shell unless argv explicitly names one.

### `delete`

Destroy compute, storage, network, credentials, and routing. Repeating deletion for an absent sandbox should succeed.

## Driver requirements

- Authenticate artifacts and pin kernel/rootfs/image digests.
- Use the Firecracker jailer or an equivalent outer sandbox.
- Create per-sandbox cgroups, namespaces, network identity, and storage.
- Resolve secret references inside the driver trust boundary.
- Enforce bounded responses and never write tenant output to protocol stderr.
- Persist an idempotency journal before executing side effects.
- Expose enough internal telemetry to reconcile orphaned instances after worker restart.
