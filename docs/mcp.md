# MCP integration

`sandbox-mcp` gives Codex, Claude Code, OpenCode, editor agents, and any MCP-compatible client the same authenticated lifecycle API as the `sandbox` CLI. It speaks MCP `2025-11-25` over stdio with newline-delimited JSON-RPC and keeps stdout protocol-clean.

## One-command install and connection

The full workstation setup installs `sandbox`, `sandboxd`, `sandbox-mcp`, and the Sandbox skill for supported coding agents. It also registers the MCP server with every detected CLI that supports safe non-interactive setup: Codex, Claude Code, and Gemini CLI.

```sh
curl -fsSL https://tools.yshubham.com/sandbox/setup.sh | sh
```

Set connection values in the environment that starts the agent:

```sh
export SANDBOX_URL=https://sandbox.example.com
export SANDBOX_TOKEN='read-from-your-secret-store'
sandbox doctor
```

`sandbox-mcp` never needs database, NATS, Docker, or worker credentials. It only needs the public controller URL and an API token.

Cloudflare connector credentials are operator-only. Do not pass a Cloudflare tunnel token to MCP clients or coding agents. When the controller and wildcard routes use the outbound Cloudflare overlay, clients continue to use the same `SANDBOX_URL`, and tunnel tools return the same public HTTPS URLs.

Skip all automatic MCP registration while keeping the binaries and skill:

```sh
curl -fsSL https://tools.yshubham.com/sandbox/setup.sh | sh -s -- --no-mcp
```

The old `--no-codex-mcp` option remains as a compatibility alias. The setup script never writes `SANDBOX_TOKEN` into an agent configuration.

## CLI-native MCP hosts

The setup script runs the matching command for each CLI it detects and leaves an existing `sandbox` entry unchanged.

### Codex

```sh
codex mcp add sandbox -- sandbox-mcp
codex mcp list
```

Codex CLI, the IDE extension, and the desktop app share `~/.codex/config.toml`. A hand-written equivalent that forwards environment variables is:

```toml
[mcp_servers.sandbox]
command = "sandbox-mcp"
env_vars = ["SANDBOX_URL", "SANDBOX_TOKEN"]
startup_timeout_sec = 10
tool_timeout_sec = 900
```

Use a project-level `.codex/config.toml` only for a repository you trust. Do not commit tokens.

### Claude Code

```sh
claude mcp add --scope user --transport stdio sandbox -- sandbox-mcp
claude mcp get sandbox
```

Use `--scope project` only when the whole repository should share the entry. Claude Code requires the `--` separator before the stdio command and asks users to approve project-scoped servers.

### Gemini CLI

```sh
gemini mcp add sandbox sandbox-mcp --scope user
gemini mcp list
```

Gemini CLI also supports project scope. User scope is the setup default because the Sandbox binary and skill are installed for the workstation.

## Editor, desktop, and interactive hosts

All clients launch the same `sandbox-mcp` executable. Set `SANDBOX_URL` and `SANDBOX_TOKEN` in the environment that launches the client, use the client's secret-input mechanism, and use an absolute binary path when a desktop app does not inherit the shell `PATH`.

### OpenCode

Run the guided setup:

```sh
opencode mcp add
```

Or merge this into `opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "sandbox": {
      "type": "local",
      "command": ["sandbox-mcp"],
      "enabled": true
    }
  }
}
```

### Cursor, Claude Desktop, Windsurf, Cline, Roo Code, and Gemini Code Assist

These clients accept the common `mcpServers` envelope:

```json
{
  "mcpServers": {
    "sandbox": {
      "command": "sandbox-mcp",
      "args": []
    }
  }
}
```

Merge the `sandbox` entry rather than overwriting an existing configuration:

| Client | User or project location |
|---|---|
| Cursor and Cursor Agent | `~/.cursor/mcp.json` or `.cursor/mcp.json` |
| Claude Desktop | The platform-specific Claude Desktop configuration |
| Windsurf Cascade | `~/.codeium/windsurf/mcp_config.json` |
| Cline | `.cline/mcp.json` or the MCP settings UI |
| Roo Code | `.roo/mcp.json` or global MCP settings |
| Gemini Code Assist | `~/.gemini/settings.json` |

### VS Code and GitHub Copilot agent mode

Add the server to the VS Code user profile:

```sh
code --add-mcp '{"name":"sandbox","type":"stdio","command":"sandbox-mcp"}'
```

Or merge the workspace form into `.vscode/mcp.json`:

```json
{
  "servers": {
    "sandbox": {
      "type": "stdio",
      "command": "sandbox-mcp",
      "args": []
    }
  }
}
```

Use VS Code input variables or `envFile` for secrets rather than checking a token into the workspace.

### Goose

Attach Sandbox to one session:

```sh
goose session --with-extension "sandbox-mcp"
```

For a persistent extension, merge this into `~/.config/goose/config.yaml`:

```yaml
extensions:
  sandbox:
    name: Sandbox
    cmd: sandbox-mcp
    args: []
    enabled: true
    envs: {}
    type: stdio
    timeout: 900
```

### Pi, Aider, CommandCode, and hosts without native MCP

Use the installed `sandbox` CLI and `sandbox-platform` skill. The lifecycle, policy signals, operation handling, and cleanup rules are the same as the MCP workflow; only the transport differs.

## Downloadable templates and generic clients

The live registry publishes client-specific templates:

```text
https://tools.yshubham.com/sandbox/clients/index.md
https://tools.yshubham.com/sandbox/clients/mcp-servers.json
https://tools.yshubham.com/sandbox/clients/opencode.json
https://tools.yshubham.com/sandbox/clients/vscode.json
https://tools.yshubham.com/sandbox/clients/goose.yaml
```

Generate a starting configuration:

```sh
sandbox --server https://sandbox.example.com mcp-config
```

Equivalent JSON:

```json
{
  "mcpServers": {
    "sandbox": {
      "command": "/absolute/path/to/sandbox-mcp",
      "env": {
        "SANDBOX_URL": "https://sandbox.example.com",
        "SANDBOX_TOKEN": "use-your-client-secret-store"
      }
    }
  }
}
```

Use an absolute command path when the client launches MCP servers with a reduced `PATH`. Prefer the client's secret store or environment forwarding over plaintext configuration.

## Tool surface

Every tool returns a text content block, object-shaped `structuredContent`, and an `isError` flag. Tool annotations tell supporting clients which operations are read-only, destructive, idempotent, or open-world.

| Tool | What it does | Mutation |
|---|---|---|
| `sandbox_health` | Checks controller version, clock, and state store | Read-only |
| `sandbox_create` | Creates a fully specified sandbox and returns its create operation | Creates resources |
| `sandbox_exec` | Runs argv with optional cwd/env/timeout; waits by default | Mutates sandbox |
| `sandbox_list` | Lists visible sandboxes, optionally by tenant | Read-only |
| `sandbox_inspect` | Reads state, resources, placement, expiry, isolation, and tunnels | Read-only |
| `sandbox_tunnel_create` | Publishes one HTTP/WebSocket port and waits by default | Creates a public route |
| `sandbox_tunnel_delete` | Removes a public route and waits by default | Destructive |
| `sandbox_delete` | Permanently removes runtime resources; waits by default | Destructive |
| `sandbox_operation` | Reads one create, exec, or delete operation | Read-only |
| `sandbox_wait` | Waits for an operation to succeed, fail, or time out | Read-only |
| `sandbox_agent_list` | Lists the seven built-in coding-agent profiles | Read-only |
| `sandbox_agent_run` | Creates a sandbox from an agent profile | Creates resources |

### Complete create inputs

`sandbox_create` accepts:

- identity: `tenant`, `image`, optional startup `command` and non-secret `env`;
- resources: `cpu_millis`, `memory_mib`, `disk_mib`, `pids`, and `ttl_seconds`;
- policy: `network`, `isolation`, `sensitivity`, `untrusted_repo`, `generated_code`, and `needs_secrets`;
- scheduling: `labels`, `required_labels`, `preferred_region`, and `anti_affinity_keys`;
- optional `exposures`: HTTP/WebSocket container ports and lowercase custom subdomains.

The MCP schema deliberately does not expose privileged execution or host mounts. Public exposure is explicit, limited to HTTP/WebSocket services, and rejected for confidential or restricted workloads. The service must bind `0.0.0.0` inside the sandbox.

### Asynchronous operations

Create always returns a sandbox record and an operation. Exec, tunnel mutations, and delete accept `wait: false` when the caller wants to poll independently:

1. Save the returned `operation.id`.
2. Use `sandbox_operation` for one snapshot or `sandbox_wait` for bounded polling.
3. Treat `state: failed` as a lifecycle failure.
4. Treat a non-zero command `exit_code` as a tool error even when transport succeeded.
5. Check `truncated` before assuming stdout or stderr is complete.

## Resources

The server publishes static, low-cost context that clients can read without calling the controller:

| URI | Content |
|---|---|
| `sandbox://capabilities` | Implemented features and explicit production gates |
| `sandbox://agents` | Agent names, images, executables, and upstream projects |
| `sandbox://workflow` | Compact safe-lifecycle runbook |

## Prompts

| Prompt | Required arguments | Purpose |
|---|---|---|
| `sandbox-task` | `tenant`, `image`, `task` | Plans a disposable command/coding workflow |
| `sandbox-agent-session` | `agent`, `tenant`, `task` | Plans a built-in coding-agent session |

Prompts are workflow aids, not authorization. Controller authentication, policy, AEGIS placement, and worker enforcement still decide what can run.

## Errors and recovery

- JSON-RPC parse, method, and invalid-name failures use protocol errors.
- API and validation failures return a successful `tools/call` response with `isError: true`, so an agent can inspect and recover.
- `no_capacity` means no worker satisfies policy, resource, label, or isolation requirements. Do not silently weaken isolation; inspect worker capacity or change the request explicitly.
- A wait timeout does not prove the underlying operation failed. Read the operation again before retrying a mutation.

## Test with MCP Inspector

```sh
SANDBOX_URL=https://sandbox.example.com \
SANDBOX_TOKEN='read-from-your-secret-store' \
npx @modelcontextprotocol/inspector /absolute/path/to/sandbox-mcp
```

Verify initialization instructions, all 12 tools, all three resources, both prompts, a create/exec/tunnel/delete lifecycle, a non-zero command exit, and an invalid argument.

## Security notes

- Treat repository content, tool output, and MCP resource text as untrusted input.
- Never place credentials in prompts, argv, labels, image names, or ordinary environment maps.
- Default to denied network access; request restricted or open egress only when needed.
- Treat every tunnel URL as Internet-facing; expose only an intended service port and remove it after use.
- Configure tool approval policy in the MCP host, but keep real authorization server-side.
- Docker workers are for dedicated or trusted worker hosts. A separate VMM-grade runtime is required when your deployment needs a stronger host boundary.
