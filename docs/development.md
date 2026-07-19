# Development

## Toolchain

Rust 1.97.1 is pinned in `rust-toolchain.toml`. On a shell with a constrained `PATH`, use:

```sh
env PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" cargo check --workspace
```

`pq-sys` builds its bundled libpq and vendored OpenSSL, so the workspace does not require machine-specific PostgreSQL client headers. A C toolchain, CMake, Make, and Perl are still required for those native builds; the pinned Rust and Docker toolchains provide them.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --profile dist --workspace
```

Validate the repository skill:

```sh
python3 /path/to/skill-creator/scripts/quick_validate.py skills/sandbox-platform
```

## Local controller without Docker

Use the memory store and explicit development authentication bypass:

```sh
SANDBOX__SERVER__ALLOW_UNAUTHENTICATED_DEV=true \
  cargo run --package sandboxd -- --role controller
```

Creation still needs a registered worker. Unit tests exercise AEGIS without one; the Compose stack supplies a Docker worker for integration work.

## Adding an API field

1. Add the domain field with a backward-compatible Serde default where appropriate.
2. Update CLI and MCP schema only if callers should control it.
3. Add validation and policy behavior server-side.
4. Add scheduler/runtime tests.
5. Document whether it is persisted, secret, logged, and authorized.

## Dependency policy

Use stable crate releases compatible with the pinned compiler, commit `Cargo.lock`, run advisory/license checks, and update agent versions separately from platform dependencies. Avoid adding Redis or another daemon unless measured load proves the PostgreSQL/in-memory path inadequate.
