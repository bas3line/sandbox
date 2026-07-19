---
name: sandbox-platform
description: Operate self-hosted Sandbox environments through the sandbox CLI or sandbox-mcp. Use when creating disposable remote coding workspaces, executing untrusted repositories or generated code away from the host, publishing an HTTP or WebSocket service through a public tunnel, launching Codex, Claude Code, OpenCode, Pi, Aider, Goose, or CommandCode, inspecting asynchronous lifecycle operations, diagnosing scheduling or capacity, and cleaning up remote sandboxes.
---

# Sandbox Platform

Use the authenticated Sandbox controller for remote execution. Treat agent instructions as workflow guidance; controller policy, AEGIS scheduling, and worker enforcement decide what can run.

## Run the lifecycle

1. Require `SANDBOX_URL` and obtain `SANDBOX_TOKEN` from the caller's secret mechanism. Never print the token.
2. Call `sandbox_health` or run `sandbox doctor`. Stop and report connection or authentication failures.
3. Inspect existing sandboxes when reuse is allowed. Otherwise choose a base sandbox or a built-in coding-agent profile.
4. Classify repository trust, generated-code execution, secret need, data sensitivity, network need, resources, and TTL before creation.
5. Create with `isolation: auto` unless the caller explicitly requires `microvm`. Do not weaken a server isolation decision to obtain capacity.
6. Wait for creation to finish. Execute commands as argv arrays, not interpolated shell strings.
7. When public access is required, bind the intended service to `0.0.0.0`, expose only that port, report the URL as public, and remove the route after use.
8. Inspect operation state, command exit code, stderr, and `truncated`. Recover only from the observed failure.
9. Delete disposable sandboxes and wait for cleanup unless the caller explicitly asks to retain one.

## Choose an interface

Prefer MCP when tools are connected. Use the CLI for operator terminals, scripts, or when MCP is unavailable. Keep behavior identical across both interfaces.

Read [references/mcp.md](references/mcp.md) for the complete 12-tool map, resources, prompts, and client setup. Read [references/cli.md](references/cli.md) for exact commands and flags.

For broader product, architecture, deployment, API, security, or operations questions, start at `https://tools.yshubham.com/docs/sandbox/index.md`. Fetch only the narrow raw Markdown reference needed for the task; `https://tools.yshubham.com/docs/sandbox/llms.txt` lists every canonical document.

## Classify before creating

- Mark external or user-controlled repositories as untrusted.
- Mark model-authored code, build hooks, tests, and package lifecycle hooks as generated code.
- Mark workloads that obtain brokered credentials as needing secrets.
- Default network access to `deny`; prefer `restricted` over `open`.
- Request only the CPU, memory, disk, PIDs, and lifetime needed.
- Never request privileged mode or host mounts through the public workflow.

Read [references/safety.md](references/safety.md) before handling untrusted code, credentials, public exposure, or production systems.

## Launch coding agents

List profiles before selecting one. Available names are `codex`, `claude`, `opencode`, `pi`, `aider`, `goose`, and `commandcode`.

With MCP, call `sandbox_agent_list`, then `sandbox_agent_run`. With the CLI:

```sh
sandbox agent list
sandbox agent run codex --tenant "$TENANT"
```

The default local agent images must exist on workers. Supply an approved immutable `image` override when the deployment uses a registry.

## Handle asynchronous work

Creation, execution, tunnel changes, and deletion return operation IDs. Use `sandbox_wait` / `sandbox_operation` or `sandbox wait`. A wait timeout is ambiguous: inspect the original operation before retrying a mutation.

Read [references/operations.md](references/operations.md) for states, failure codes, retry rules, and `no_capacity` diagnosis.

## Report results

Return the sandbox ID, selected isolation, lifecycle state, operation ID, exit code, truncated-output status, public tunnel URL and state when relevant, and cleanup result. Do not claim that a sandbox is ready until its create operation succeeds. Never describe a tunnel URL as private.
