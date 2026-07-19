# MCP tool and context map

## Setup

```sh
curl -fsSL https://tools.yshubham.com/sandbox/setup.sh | sh
export SANDBOX_URL=https://sandbox.example.com
export SANDBOX_TOKEN='read-from-your-secret-store'
```

The setup script installs the skill for supported agents and registers `sandbox-mcp` with detected Codex, Claude Code, and Gemini CLIs. Manual commands are:

```sh
codex mcp add sandbox -- sandbox-mcp
claude mcp add --scope user --transport stdio sandbox -- sandbox-mcp
gemini mcp add sandbox sandbox-mcp --scope user
opencode mcp add
code --add-mcp '{"name":"sandbox","type":"stdio","command":"sandbox-mcp"}'
goose session --with-extension "sandbox-mcp"
```

Cursor, Claude Desktop, Windsurf, Cline, Roo Code, and Gemini Code Assist use the shared `mcpServers` JSON envelope. OpenCode, VS Code, and Goose have native formats. Fetch verified templates and config locations from `https://tools.yshubham.com/sandbox/clients/index.md`.

Pi, Aider, CommandCode, and hosts without native MCP use the `sandbox` CLI with this skill. `sandbox-mcp` is a local stdio bridge. It connects to the public controller API and does not need worker, database, Docker, or NATS access. Never place `SANDBOX_TOKEN` in a committed client config.

## Tools

| Tool | Use |
|---|---|
| `sandbox_health` | Verify controller reachability and version |
| `sandbox_create` | Create with resources, policy signals, labels, and placement |
| `sandbox_exec` | Execute argv; wait by default or return an operation |
| `sandbox_list` | List visible sandboxes, optionally by tenant |
| `sandbox_inspect` | Read one sandbox, selected isolation, and tunnels |
| `sandbox_tunnel_create` | Publish one HTTP/WebSocket port and wait by default |
| `sandbox_tunnel_delete` | Remove a public route and wait by default |
| `sandbox_delete` | Remove runtime resources; wait by default |
| `sandbox_operation` | Read one asynchronous operation snapshot |
| `sandbox_wait` | Poll an operation with a bounded timeout |
| `sandbox_agent_list` | Discover built-in coding-agent profiles |
| `sandbox_agent_run` | Create from a coding-agent profile |

For `sandbox_create`, supply `tenant` and `image`. Optional fields cover startup `command`, non-secret `env`, CPU, memory, disk, PIDs, TTL, network, isolation, sensitivity, risk signals, labels, required worker labels, preferred region, anti-affinity keys, and HTTP/WebSocket `exposures`.

For `sandbox_exec`, supply `sandbox_id` and `argv`. Optional fields are `cwd`, non-secret `env`, `timeout_seconds`, and `wait`.

For `sandbox_tunnel_create`, supply `sandbox_id` and `container_port`; optionally request a lowercase `subdomain`. The service must listen on `0.0.0.0`. Treat the returned URL as Internet-facing and delete it when no longer required.

## Resources

- Read `sandbox://capabilities` before claiming a deployment feature exists.
- Read `sandbox://agents` to discover profile defaults without a controller call.
- Read `sandbox://workflow` for the compact lifecycle runbook.

## Prompts

- `sandbox-task` accepts `tenant`, `image`, `task`, and optional `network`.
- `sandbox-agent-session` accepts `agent`, `tenant`, and `task`.

Prompts guide workflow only. They do not grant authorization.

## Result handling

Inspect `isError` before consuming `structuredContent`. A non-zero remote exit is a tool error. Check the nested operation `state`, `error`, `output.exit_code`, `stdout`, `stderr`, and `truncated` fields. A protocol-success response does not mean the remote command succeeded.
