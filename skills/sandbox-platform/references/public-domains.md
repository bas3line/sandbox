# Public domains and TLS

Sandbox persists the operator-configured scheme and wildcard base domain in every tunnel URL. Clients must use the returned URL exactly. Never change `http://` to `https://` based on an assumption.

## Choose the topology

- Prefer Cloudflare Tunnel when the origin must not accept public ingress.
- Use proxied `A`/`AAAA` records plus Cloudflare Origin CA when a public origin is acceptable. Keep the zone on Full (strict).
- Use direct Caddy or Traefik when Cloudflare is not required.

Orange-cloud DNS obscures ordinary DNS answers but does not block a known origin address. Cloudflare Tunnel or verified origin firewall rules provide that boundary.

## Configure a nested wildcard

For service URLs such as `https://preview.tunnel.example.com`, configure Sandbox with `tunnel.example.com`, without `*.` or a trailing dot.

A full Cloudflare zone's Universal SSL certificate does not cover this nested wildcard. Order an Advanced certificate containing `*.tunnel.example.com`, finish DCV, and require `active` status before testing HTTPS.

For a proxied origin:

1. Issue a Cloudflare Origin CA certificate containing `*.tunnel.example.com`.
2. Keep its private key root-readable on the server and outside Git.
3. Run the base Compose file, Caddy overlay, and `compose.cloudflare-origin.yaml`.
4. Set `SANDBOX_TUNNEL_SCHEME=https`, `SANDBOX_TUNNEL_ENTRYPOINT=web`, and `SANDBOX_TUNNEL_EDGE_TLS=false`.
5. Set Cloudflare encryption to Full (strict). Never downgrade to Flexible.
6. Require an unused wildcard hostname to return `404` with successful TLS, then test create, publish, fetch, unpublish, and delete.

For a hidden origin, route both the controller hostname and wildcard hostname through the outbound `compose.cloudflare.yaml` connector. The connector token and Origin CA private key are deployment credentials; never request them through MCP or place them inside a sandbox.

See `docs/how-to-setup/custom-public-domains.md` or the canonical [custom-domain guide](https://docs.yshubham.com/v2/sandbox/custom-domains) for the complete commands and verification steps.
