# Set up custom public domains

This guide configures an HTTPS controller hostname such as `sandbox.example.com` and wildcard sandbox service URLs such as `https://review-42.tunnel.example.com`.

Public tunnel URLs are Internet-facing. Publish only an intended HTTP or WebSocket port, never a database, Docker API, debug console, or credential-bearing admin service.

## Choose the ingress topology

| Topology | Origin exposure | TLS path | Use when |
|---|---|---|---|
| Cloudflare Tunnel | No public origin listener | Cloudflare edge to outbound connector | Recommended when the origin must stay private |
| Proxied `A`/`AAAA` plus Origin CA | Origin ports remain public unless firewalled | Cloudflare edge plus Full (strict) origin TLS | Simple dedicated host with Cloudflare proxying |
| Direct Caddy or Traefik | Public origin | Public CA to the origin | No Cloudflare dependency |

Orange-cloud DNS hides the configured origin from ordinary DNS answers, but it is not an access-control boundary. Use Cloudflare Tunnel or restrict origin ingress after end-to-end verification when direct-IP bypass matters.

## Cloudflare proxied wildcard with Full (strict)

This is the fixed-domain HTTPS mode supported by [`compose.cloudflare-origin.yaml`](https://github.com/bas3line/sandbox/blob/main/deploy/compose/compose.cloudflare-origin.yaml).

### 1. Choose hostnames

```text
Controller: sandbox.example.com
Tunnel base: tunnel.example.com
Tunnel wildcard: *.tunnel.example.com
```

Set only `tunnel.example.com` in Sandbox configuration. Do not include `*.` or a trailing dot.

### 2. Create proxied DNS records

Create orange-cloud records for the controller and wildcard:

```text
sandbox.example.com          A  ORIGIN_IPV4  proxied
*.tunnel.example.com         A  ORIGIN_IPV4  proxied
```

Use `AAAA` only when the origin is intentionally reachable over IPv6. Do not publish worker addresses.

### 3. Issue the Cloudflare edge certificate

Cloudflare Universal SSL covers only one wildcard level in a full zone. `*.example.com` does not cover `*.tunnel.example.com`. Order an Advanced certificate containing both the zone apex and the exact deep wildcard.

In **SSL/TLS → Edge Certificates → Order Advanced Certificate**:

1. Select a supported certificate authority.
2. Include `example.com` and `*.tunnel.example.com`.
3. Select TXT validation.
4. Choose a validity period with automatic renewal.
5. Publish every requested DCV TXT record and wait for status `active`.

Do not continue while the certificate is `pending_validation`. See Cloudflare's [Advanced Certificate Manager documentation](https://developers.cloudflare.com/ssl/edge-certificates/advanced-certificate-manager/).

### 4. Issue an Origin CA certificate

Create a Cloudflare Origin CA certificate for:

```text
tunnel.example.com
*.tunnel.example.com
```

Prefer ECC. For strict private-key locality, generate the key and CSR on the server and submit only the CSR through Cloudflare's Origin CA API. Otherwise, use **SSL/TLS → Origin Server → Create Certificate**, copy the key once, and move it directly into root-readable files.

```sh
sudo install -d -m 0700 /etc/sandbox/origin-tls
sudo install -m 0644 cloudflare-origin.crt /etc/sandbox/origin-tls/tunnel.crt
sudo install -m 0600 cloudflare-origin.key /etc/sandbox/origin-tls/tunnel.key

sudo openssl x509 \
  -in /etc/sandbox/origin-tls/tunnel.crt \
  -noout -subject -issuer -dates -ext subjectAltName
```

The SAN list must contain `*.tunnel.example.com`. Never commit or transmit the private key to a client or coding agent.

Cloudflare Origin CA certificates are meant for the proxied Cloudflare-to-origin connection; browsers do not trust them directly. Keep both DNS records proxied.

### 5. Configure Sandbox and Caddy

Add these non-secret values to the server runtime environment:

```text
SANDBOX_DOMAIN=sandbox.example.com
SANDBOX_PORT=127.0.0.1:8080
SANDBOX_TUNNEL_ENABLED=true
SANDBOX_TUNNEL_DOMAIN=tunnel.example.com
SANDBOX_TUNNEL_SCHEME=https
SANDBOX_TUNNEL_ENTRYPOINT=web
SANDBOX_TUNNEL_EDGE_TLS=false
SANDBOX_ACME_EMAIL=admin@example.com
CLOUDFLARE_ORIGIN_CERT_FILE=/etc/sandbox/origin-tls/tunnel.crt
CLOUDFLARE_ORIGIN_KEY_FILE=/etc/sandbox/origin-tls/tunnel.key
```

The public edge is HTTPS, Caddy terminates verified origin TLS, and the per-sandbox tunnel edge remains private HTTP on the Compose network.

```sh
sudo docker compose \
  --env-file /etc/sandbox/runtime.env \
  -f deploy/compose/compose.yaml \
  -f deploy/compose/compose.caddy.yaml \
  -f deploy/compose/compose.cloudflare-origin.yaml \
  --profile caddy-edge \
  up --build -d
```

Set the Cloudflare zone encryption mode to **Full (strict)**. Do not use Flexible. Full (strict) requires an unexpired origin certificate whose SAN matches the requested hostname and encrypts both network legs. See Cloudflare's [Full (strict) documentation](https://developers.cloudflare.com/ssl/origin-configuration/ssl-modes/full-strict/).

### 6. Verify before exposing workloads

An unused wildcard hostname should complete TLS and reach the tunnel edge as `404`:

```sh
curl -sS -o /dev/null \
  -w 'code=%{http_code} tls=%{ssl_verify_result}\n' \
  https://preflight.tunnel.example.com/
```

Require `code=404` and `tls=0`. A `525` or `526` means the Cloudflare-to-origin TLS path is not ready.

Then run a disposable lifecycle:

```sh
sandbox create --tenant domain-smoke --image python:3.13-alpine --ttl 600 --network deny
sandbox wait OPERATION_ID --timeout 120
sandbox exec SANDBOX_ID -- sh -c \
  'printf domain-ok > /tmp/index.html; cd /tmp; python -m http.server 8000 --bind 0.0.0.0 >/tmp/http.log 2>&1 &'
sandbox tunnel create SANDBOX_ID --port 8000
curl --fail PUBLIC_HTTPS_URL/index.html
sandbox tunnel delete SANDBOX_ID TUNNEL_ID
sandbox delete SANDBOX_ID --wait
```

Use the IDs and URL returned by your controller. After deletion, the public URL must settle to `404`, and the worker must have no route file, per-sandbox tunnel network, or runtime container for the test.

## Cloudflare Tunnel with a hidden origin

For a private origin, create a remotely managed Cloudflare Tunnel and publish these application routes:

| Public hostname | Internal service |
|---|---|
| `sandbox.example.com` | `http://controller:8080` |
| `*.tunnel.example.com` | `http://tunnel-edge:8080` |

Store the connector token only in a mode-`0600` file and start the outbound overlay:

```text
SANDBOX_PORT=127.0.0.1:8080
SANDBOX_TUNNEL_ENABLED=true
SANDBOX_TUNNEL_DOMAIN=tunnel.example.com
SANDBOX_TUNNEL_SCHEME=https
SANDBOX_TUNNEL_ENTRYPOINT=web
SANDBOX_TUNNEL_EDGE_TLS=false
CLOUDFLARE_TUNNEL_TOKEN_FILE=/etc/sandbox/cloudflare-tunnel.token
```

```sh
sudo docker compose \
  --env-file /etc/sandbox/runtime.env \
  -f deploy/compose/compose.yaml \
  -f deploy/compose/compose.cloudflare.yaml \
  --profile cloudflare-edge \
  up --build -d
```

The nested wildcard still needs an active Cloudflare edge certificate. After both routes pass end-to-end tests, close public controller, HTTP, and HTTPS ingress while preserving the management path and outbound connector traffic.

See [Public tunnels](../tunnels.md) for direct Caddy, Traefik, Cloudflare Tunnel, lifecycle behavior, and troubleshooting.
