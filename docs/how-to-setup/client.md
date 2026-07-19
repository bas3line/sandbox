# Set up a client PC

The workstation needs only `sandbox`, `sandbox-mcp`, the Sandbox skill, the controller URL, and an API token. It does not need Docker, PostgreSQL, NATS, worker credentials, or Cloudflare credentials.

## 1. Install everything

The one-command setup installs the binaries, installs the cross-agent skill, and registers `sandbox-mcp` with detected Codex, Claude Code, and Gemini CLIs. It also publishes templates for OpenCode, Cursor, VS Code, Goose, Claude Desktop, Windsurf, Cline, Roo Code, and generic MCP clients.

```sh
curl -fsSL https://tools.yshubham.com/sandbox/setup.sh | sh
```

Use the binary-only installer when no local agent integration is wanted:

```sh
curl -fsSL https://tools.yshubham.com/sandbox/install.sh | sh
```

## 2. Configure the connection

Obtain the public HTTPS controller URL and an operator API token from the server administrator. Keep the token in the operating system or agent secret store.

For one shell session:

```sh
export SANDBOX_URL=https://sandbox.example.com
read -r -s -p 'Sandbox token: ' SANDBOX_TOKEN
printf '\n'
export SANDBOX_TOKEN
```

Do not add the token to shell history, a repository `.env`, MCP JSON, prompts, labels, or sandbox environment maps.

## 3. Verify the CLI

```sh
sandbox doctor
sandbox agent list
```

`sandbox doctor` must report the expected controller version and tunnel configuration before an agent creates resources.

To share a service already running on the workstation, no managed sandbox is needed:

```sh
sandbox http 4321
```

The public URL uses the hosted `*.tunnel.yshubham.com` wildcard by default. A self-hosted installation sets `SANDBOX_HTTP_RELAY=https://relay.tunnel.example.com`. Ctrl-C revokes the route.

## 4. Register MCP clients

All native clients launch the same local stdio process. Existing entries should point to the absolute `sandbox-mcp` path when desktop applications do not inherit the shell `PATH`. A local wrapper that reads the controller URL and token from the operating-system secret store is also valid and avoids plaintext agent configuration.

```sh
# Codex
codex mcp add sandbox -- sandbox-mcp

# Claude Code
claude mcp add --scope user --transport stdio sandbox -- sandbox-mcp

# Gemini CLI
gemini mcp add sandbox sandbox-mcp --scope user

# OpenCode guided setup
opencode mcp add

# VS Code / Copilot agent mode
code --add-mcp '{"name":"sandbox","type":"stdio","command":"sandbox-mcp"}'

# Goose, one session
goose session --with-extension sandbox-mcp
```

Cursor, Claude Desktop, Windsurf, Cline, Roo Code, and Gemini Code Assist use the shared `mcpServers` JSON envelope. Pi, Aider, and CommandCode use the `sandbox` CLI plus the installed `sandbox-platform` skill when their host does not provide native MCP.

Use the verified templates at `https://tools.yshubham.com/sandbox/clients/index.md`. Merge only the `sandbox` entry; never overwrite unrelated MCP servers.

## 5. Verify MCP behavior

Restart the agent after changing its MCP configuration. Then ask it to read `sandbox://capabilities` and call `sandbox_health`.

For a protocol-level check:

```sh
SANDBOX_URL=https://sandbox.example.com \
SANDBOX_TOKEN='read-from-your-secret-store' \
npx @modelcontextprotocol/inspector "$(command -v sandbox-mcp)"
```

The MCP server exposes 12 lifecycle tools, three resources, and two workflow prompts. Public tunnel tools return the deployment-configured URL. Use that URL exactly; never rewrite `http://` to `https://` on the client.

## 6. Run and clean up a client test

```sh
sandbox create --tenant workstation-smoke --image alpine:3.22 --ttl 300 --network deny
sandbox wait OPERATION_ID --timeout 120
sandbox exec SANDBOX_ID -- printf 'sandbox-client-ok\n'
sandbox delete SANDBOX_ID --wait
```

Confirm the delete operation succeeds. A stopped audit record may remain in the controller store even after runtime containers, routes, and networks are gone.

See the full [MCP client matrix](../mcp.md) and [CLI reference](../cli.md).
