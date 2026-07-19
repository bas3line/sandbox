# Operations, retries, and capacity

## States

Operations move through `pending`, `running`, and one terminal state: `succeeded` or `failed`. Creation, execution, and deletion may all be asynchronous.

Do not treat a sandbox record returned by create as ready until the associated operation succeeds. Do not treat an exec operation as successful until its state succeeds and its command exit code is zero.

## Waiting

Use `sandbox_wait` or `sandbox wait` with a timeout appropriate to image pull, startup, and command duration. A caller-side timeout does not cancel the server operation and does not prove failure. Inspect the same operation ID before retrying.

## Retry rules

- Retry read-only health, list, inspect, and operation reads after transient transport failures.
- Inspect a timed-out mutation before repeating it.
- Do not automatically retry a failed command that may have external side effects.
- Do not weaken network, sensitivity, resource, or isolation inputs automatically.
- Use a new sandbox when runtime state is uncertain or contaminated.

## `no_capacity`

`no_capacity` means no healthy worker matches all policy and scheduling constraints. Check:

1. Required isolation tier and whether a compatible runtime is configured.
2. Free CPU, memory, disk, and PID capacity.
3. Required worker labels and preferred region.
4. Worker heartbeat freshness, drain state, and pressure.
5. Image availability and registry access.

Report the limiting constraint. Ask the caller before changing requirements.

## Output failures

- Non-zero `exit_code`: report stderr and the exact argv; fix the command or workspace state.
- `truncated: true`: state that output is incomplete; narrow the command or retrieve logs through deployment-specific observability.
- Operation `failed` with no output: report the operation error and inspect sandbox state before continuing.
- Lost connection during exec: inspect the operation ID if one was returned; otherwise inspect the sandbox before rerunning.
