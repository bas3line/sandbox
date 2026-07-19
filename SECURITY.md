# Security policy

## Supported versions

Until the first stable release, only the current `main` branch receives security fixes.

## Report a vulnerability

Use GitHub Private Vulnerability Reporting for `bas3line/sandbox`. Do not open a public issue, attach exploit logs to a public discussion, or test against infrastructure you do not own.

Include the affected commit/version, runtime backend, deployment topology, reproduction steps, impact, and any suggested containment. Remove credentials, tenant data, and production addresses from the report.

The maintainers should acknowledge a report within three business days, coordinate a fix and advisory privately, and credit the reporter unless anonymity is requested.

## Scope

Controller authorization bypasses, worker impersonation, assignment forgery, cross-sandbox access, runtime escape, secret leakage, policy downgrade, installer/release-chain compromise, and denial of service that defeats configured limits are in scope.

The documented limitations in `docs/security.md` are not themselves vulnerabilities, but an implementation that claims or enforces behavior differently from that document may be.
