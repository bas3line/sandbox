# AEGIS scheduler

AEGIS is the Adaptive Execution Guard and Isolation Scheduler. It makes isolation selection part of scheduling, so capacity pressure cannot silently downgrade security.

## Phase 1: workload risk

Risk is a saturated score from 0 to 100:

| Signal | Points |
|---|---:|
| Data: public / internal / confidential / restricted | 0 / 10 / 25 / 45 |
| Network: deny / restricted / open | 0 / 10 / 25 |
| Untrusted repository | 20 |
| Executes generated code | 20 |
| Needs secrets | 15 |
| Host mounts | 25 |
| Public/TCP exposure | 15 |
| TTL over 24 hours | 10 |
| Privileged request | 100 and public-API rejection |

`auto` selects a microVM at or above `policy.microvm_risk_threshold`, which defaults to 55. An explicit microVM request is honored. An explicit container request above the threshold is upgraded to microVM, never downgraded.

## Phase 2: hard gates

A node is removed when any condition is true:

- heartbeat is stale, node is draining, or pressure is invalid/above policy;
- required isolation tier is unsupported;
- any requested resource exceeds available capacity;
- sandbox-count limit is reached;
- a required placement label does not match;
- allocation would cross the configured dominant-resource reserve.

Hard gates are not score penalties. An unsafe node cannot win because it is warm or nearby.

## Phase 3: ranking

For each surviving node, AEGIS computes the post-allocation fraction remaining for CPU, RAM, disk, and PID capacity.

```text
dominant_headroom = min(cpu_remaining, memory_remaining, disk_remaining, pids_remaining)
fragmentation      = max(remaining_fractions) - min(remaining_fractions)

score = 1000
      + round(dominant_headroom * 500)
      + warm_image_bonus          # 120
      + preferred_region_bonus    # 80, or 20 when no preference exists
      + packing_bonus             # 70 below 35% dominant headroom
      - round(fragmentation * 220)
      - round(host_pressure * 300)
```

This blends best-fit packing with a reserve floor: it consolidates low-pressure workloads without creating a node that has plenty of one resource but cannot fit realistic work because another resource is exhausted. UUID order provides a deterministic final tie-break.

## Examples

A confidential agent that executes generated code from an untrusted repository and uses a brokered secret scores at least 80 and requires microVM capacity with the default policy.

A trusted internal build with no network scores 10 and can use a hardened container worker.

The Compose developer profile raises the threshold to 101 so ordinary laptops can exercise the system without KVM. Do not copy that override into production.
