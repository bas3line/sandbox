.PHONY: build check test lint fmt compose-up compose-down migrate

build:
	cargo build --workspace

check:
	cargo check --workspace --all-features

test:
	cargo test --workspace --all-features

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all -- --check

compose-up:
	docker compose -f deploy/compose/compose.yaml up -d --build

compose-down:
	docker compose -f deploy/compose/compose.yaml down

migrate:
	diesel migration run --migration-dir crates/storage/migrations
