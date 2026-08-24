set shell := ["bash", "-euo", "pipefail", "-c"]

# List the canonical engineering recipes without running verification.
default:
    @just --list

# Type-check the frontend and run the complete Vitest suite.
frontend:
    pnpm typecheck
    pnpm test

# Check repository formatting without rewriting files.
format:
    cargo fmt --all -- --check

# Run one fully-qualified test from the canonical Rust integration target.
rust-test selector:
    selector={{ quote(selector) }}; if [[ ! "$selector" =~ ^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)+$ ]]; then printf 'rust-test requires one fully-qualified test name\n' >&2; exit 2; fi
    selector={{ quote(selector) }}; matches="$(CARGO_NET_OFFLINE=true cargo test --workspace --locked --test all -- --list | awk -v selector="$selector" '$0 == selector ": test" { count += 1 } END { print count + 0 }')"; if ! [[ "$matches" -eq 1 ]]; then printf 'rust-test selector matched %s tests; expected exactly 1: %s\n' "$matches" "$selector" >&2; exit 2; fi
    selector={{ quote(selector) }}; CARGO_NET_OFFLINE=true cargo test --workspace --locked --test all "$selector" -- --exact

# Run the current typed-client and Rust Serde wire characterizations.
wire:
    pnpm exec vitest run src/skillyardClient.test.ts
    domain_tests="$(CARGO_NET_OFFLINE=true cargo test --workspace --locked --lib domain::tests:: -- --list | awk '$0 ~ /^domain::tests::.*: test$/ { count += 1 } END { print count + 0 }')"; if ! [[ "$domain_tests" -gt 0 ]]; then printf 'wire found no domain::tests:: Serde characterizations\n' >&2; exit 2; fi
    CARGO_NET_OFFLINE=true cargo test --workspace --locked --lib domain::tests::
    @printf '%s\n' '{"guard":"generated-wire-drift","status":"not_attached","owner":"Train B"}'

# Verify the released migration prefix and upgrade the v1.0.1 snapshot through the application seam.
migration:
    node scripts/check-released-migrations.mjs
    just rust-test migration_contract::v1_0_1_snapshot_upgrades_restarts_and_reads_core_state_through_application
    @printf '%s\n' '{"guard":"released-migration-prefix","status":"passed","owner":"A4"}'

# Run the complete offline verification slice without building the application.
slice: format frontend _engineering-guards _rust-all _clippy wire migration

# Run the complete slice and build the production macOS application.
stage: slice
    CARGO_NET_OFFLINE=true pnpm tauri build --bundles app

# Inspect Cargo artifacts without creating, rewriting, or deleting them.
target-report output="":
    @output={{ quote(output) }}; if [[ -n "$output" ]]; then node scripts/report-cargo-target.mjs "$output"; else node scripts/report-cargo-target.mjs; fi

# Run the one ignored Codex discovery contract on a prepared Darwin host.
mac-contract:
    test "$(uname -s)" = "Darwin" || { printf 'mac-contract requires Darwin\n' >&2; exit 2; }
    CARGO_NET_OFFLINE=true cargo test --workspace --locked --test codex_mount_contract current_codex_discovers_global_and_project_directory_symlinks -- --exact --ignored --nocapture

# Build stage evidence, then stop for separately authorized release gates.
release: stage
    @printf '%s\n' '{"status":"manual_gates_required","gates":["tart","MAC-CONTRACT","manual_product_paths","authorized_real_provider"],"provider_execution":"not_automatic","publish":"not_automatic"}'
    @exit 3

_engineering-guards:
    node --test scripts/report-cargo-target.test.mjs scripts/engineering-commands.test.mjs scripts/check-released-migrations.test.mjs

_rust-all:
    CARGO_NET_OFFLINE=true cargo test --workspace --locked

_clippy:
    CARGO_NET_OFFLINE=true cargo clippy --workspace --all-targets --locked -- -D warnings
