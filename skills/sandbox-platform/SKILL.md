---
name: sandbox-platform
description: Operate and configure self-hosted Sandbox environments through the sandbox CLI or sandbox-mcp. Use when creating disposable remote coding workspaces, executing untrusted repositories or generated code away from the host, sharing a local frontend or API at a temporary public URL, publishing a managed-sandbox HTTP or WebSocket service, configuring custom wildcard domains or Cloudflare HTTPS ingress, launching Codex, Claude Code, OpenCode, Pi, Aider, Goose, or CommandCode, inspecting asynchronous lifecycle operations, diagnosing scheduling or capacity, and cleaning up remote sandboxes.
---

# Sandbox Platform

Use the authenticated Sandbox controller for remote execution. Treat agent instructions as workflow guidance; controller policy, AEGIS scheduling, and worker enforcement decide what can run.

## Run the lifecycle

1. Require a controller configured with `sandbox config set-server URL`, `SANDBOX_URL`, or `--server`, and obtain `SANDBOX_TOKEN` from the caller's secret mechanism. Never print or persist the token.
2. Call `sandbox_health` or run `sandbox doctor`. Stop and report connection or authentication failures.
3. Inspect existing sandboxes when reuse is allowed. Otherwise choose a base sandbox or a built-in coding-agent profile.
4. Classify repository trust, generated-code execution, secret need, data sensitivity, network need, resources, and TTL before creation.
5. Create with `isolation: auto` unless the caller explicitly requires `microvm`. Do not weaken a server isolation decision to obtain capacity.
6. Wait for creation to finish. Execute commands as argv arrays, not interpolated shell strings.
7. When sharing a service on the caller's machine, use `sandbox http PORT` and keep it attached until sharing should stop. For a service inside a managed sandbox, bind to `0.0.0.0`, expose only that port with `sandbox tunnel`, use the returned URL exactly, and remove the route after use. Report every returned URL as public.
8. Inspect operation state, command exit code, stderr, and `truncated`. Recover only from the observed failure.
9. Delete disposable sandboxes and wait for cleanup unless the caller explicitly asks to retain one.

## Choose an interface

Prefer MCP when tools are connected. Use the CLI for operator terminals, scripts, or when MCP is unavailable. Keep behavior identical across both interfaces.

Read [references/mcp.md](references/mcp.md) for the complete 12-tool map, resources, prompts, and client setup. Read [references/cli.md](references/cli.md) for exact commands and flags.

Read [references/public-domains.md](references/public-domains.md) when configuring wildcard DNS, Cloudflare Advanced certificates, Origin CA, Full (strict), direct Caddy/Traefik, or an origin-hidden Cloudflare Tunnel. Domain and TLS settings belong to the server deployment; MCP clients cannot choose or rewrite them.

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
sandbox agent run opencode --tenant "$TENANT" -- --version
```

Running without agent arguments provisions an agent-ready sandbox. Arguments after `--` use the observable exec path. The default local agent images must exist on workers. Supply an approved immutable `image` override when the deployment uses a registry. CommandCode always requires an image override.

## Handle asynchronous work

Creation, execution, tunnel changes, and deletion return operation IDs. CLI mutations wait by default; use `--no-wait` only when the caller will track the ID with `sandbox wait`. MCP callers use `sandbox_wait` / `sandbox_operation`. A wait timeout is ambiguous: inspect the original operation before retrying a mutation.

Read [references/operations.md](references/operations.md) for states, failure codes, retry rules, and `no_capacity` diagnosis.

## Report results

Return the sandbox ID, selected isolation, lifecycle state, operation ID, exit code, truncated-output status, public URL and provider or managed-tunnel state when relevant, and cleanup result. Do not claim that a sandbox is ready until its create operation succeeds. Never describe a public URL as private.
