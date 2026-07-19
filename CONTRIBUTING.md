# Contributing

Sandbox welcomes focused changes that preserve its trust boundaries and small default footprint.

1. Open an issue or design note for protocol, schema, isolation, or authentication changes.
2. Keep user-facing binaries limited to `sandbox`, `sandboxd`, and `sandbox-mcp` unless a new process boundary is justified.
3. Put security decisions in the controller/runtime, not only in CLI validation or prompt text.
4. Add tests for policy bypasses, failure recovery, and boundary values.
5. Run formatting, Clippy with warnings denied, all tests, and the skill validator.
6. Update docs and the feature-status table without presenting roadmap work as implemented.

Use conventional commits when practical. Never commit tokens, agent transcripts, production output, database dumps, private images, or credentials.

All contributions are licensed under Apache-2.0.
