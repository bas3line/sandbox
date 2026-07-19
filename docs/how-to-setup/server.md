# Set up a Sandbox server

This guide deploys one Sandbox controller, one Docker worker, and PostgreSQL on a dedicated Linux host. It is the smallest durable self-hosted topology and is appropriate for personal, team, or otherwise trusted workloads.

Docker workers are not a hostile multi-tenant VM boundary. Use the external runtime adapter with a VMM-grade driver when the host must isolate mutually hostile users.

## Requirements

- A recent Linux distribution with a dedicated host or VM
- Docker Engine and Docker Compose v2
- Git, OpenSSL, and at least 4 GiB of available memory
- A non-root operator with Docker access, or a root shell for installation
- A domain only if clients must connect from another machine

## 1. Fetch the source

Clone the current main branch, then pin the exact commit in your deployment record. Production operators should review that commit before upgrading it.

```sh
git clone https://github.com/bas3line/sandbox.git
cd sandbox
git checkout main
git rev-parse HEAD
```

## 2. Create runtime secrets

Keep the deployment environment outside the repository. Generate different values for the API, worker, and database credentials.

```sh
sudo install -d -m 0700 /etc/sandbox
sudo install -m 0600 /dev/null /etc/sandbox/runtime.env

api_token=$(openssl rand -hex 32)
node_token=$(openssl rand -hex 32)
database_password=$(openssl rand -hex 32)

sudo sh -c "printf '%s\n' \
  'SANDBOX_API_TOKEN=$api_token' \
  'SANDBOX_NODE_TOKEN=$node_token' \
  'POSTGRES_PASSWORD=$database_password' \
  'SANDBOX_PORT=127.0.0.1:8080' \
  > /etc/sandbox/runtime.env"

unset api_token node_token database_password
```

Do not reuse the API token as the worker token. Never commit `/etc/sandbox/runtime.env`, paste it into an issue, or pass it to a coding agent.

## 3. Start the core stack

```sh
sudo docker compose \
  --env-file /etc/sandbox/runtime.env \
  -f deploy/compose/compose.yaml \
  up --build -d
```

The controller binds to loopback because `SANDBOX_PORT=127.0.0.1:8080`. PostgreSQL remains on the private Compose network, and the worker connects to the controller internally.

## 4. Verify health

```sh
sudo docker compose \
  --env-file /etc/sandbox/runtime.env \
  -f deploy/compose/compose.yaml \
  ps

sudo docker compose \
  --env-file /etc/sandbox/runtime.env \
  -f deploy/compose/compose.yaml \
  exec -T controller sh -lc \
  'SANDBOX_URL=http://127.0.0.1:8080 SANDBOX_TOKEN="$SANDBOX__SERVER__API_TOKEN" sandbox doctor'
```

Require the controller and PostgreSQL to report healthy and the worker to remain running.

## 5. Run a disposable lifecycle test

Install the CLI on the operator machine or run it inside the controller container. Set `SANDBOX_URL` and `SANDBOX_TOKEN` through the operator's secret mechanism first. A complete test must create, wait, execute, and delete.

```sh
sandbox create --tenant smoke --image alpine:3.22 --ttl 300 --network deny
sandbox wait OPERATION_ID --timeout 120
sandbox exec SANDBOX_ID -- uname -a
sandbox delete SANDBOX_ID --wait
```

Save the IDs printed by your deployment; the placeholders above are not literal values.

## 6. Connect remote clients

Do not expose port `8080` directly to the Internet. Put the controller behind an HTTPS reverse proxy or outbound connector, then follow [Set up a client PC](client.md).

For wildcard service URLs and Cloudflare Full (strict), continue with [Set up custom public domains](custom-public-domains.md).

## Operations checklist

- Back up PostgreSQL and test restores.
- Keep `/etc/sandbox/runtime.env` mode `0600`.
- Patch Docker, the host kernel, and pinned container images.
- Monitor controller health, worker heartbeats, disk pressure, and TTL cleanup.
- Keep worker hosts dedicated to workloads that share the same trust boundary.
- Delete disposable sandboxes explicitly; TTL is only a backstop.

See [deployment](../deployment.md), [security](../security.md), and [operations](../operations.md) for larger topologies and failure handling.
