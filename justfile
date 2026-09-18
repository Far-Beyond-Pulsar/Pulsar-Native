# Pulsar-Native development commands
# Usage: just <command>
# Install just: https://github.com/casey/just

project := "pulsar_engine"

# Windows has no `sh`; every recipe here is a plain command line, so PowerShell
# runs them unchanged and no recipe needs a platform-specific variant.
set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-Command"]

# ── Build ────────────────────────────────────────────────────────────────────

# Build the engine (default)
build:
    cargo build -p {{project}}

# Check compilation without producing binaries
check:
    cargo check

# Build release
release:
    cargo build -p {{project}} --release

# Run the engine
run:
    cargo run -p {{project}}

# ── Test ──────────────────────────────────────────────────────────────────────

# Run all workspace tests
test:
    cargo test --workspace

# Test a specific crate: just test-crate <name>
test-crate name:
    cargo test -p {{name}}

# ── Lint ──────────────────────────────────────────────────────────────────────

clippy:
    cargo clippy --workspace -- -D warnings

fmt:
    cargo fmt --all

# ── Codegen drift guard (#652) ────────────────────────────────────────────────
# Fast always-on probes: PBGC-generated blueprint actors compiled against the
# pinned crates inside pulsar_game's own test binary.
ci-drift-probe:
    cargo test -p pbgc
    cargo test -p pulsar_game --lib blueprint_codegen_drift

# Heavy end-to-end check: generate a full game project into a temp dir and
# cargo-check it against current pins (catches manifest/patch-table drift the
# fast probes cannot). First run compiles the whole engine dep tree; later
# runs reuse a shared target dir.
ci-drift-check:
    cargo test -p pulsar_game --test generated_project_compiles -- --ignored --nocapture

# ── Submodules ───────────────────────────────────────────────────────────────

# Init all submodules
submodule-init:
    git submodule update --init --recursive

# Pull latest for all submodules
submodule-update:
    git submodule update --remote --recursive

# Status of all submodules
submodule-status:
    git submodule status

# ── Vendored deps ────────────────────────────────────────────────────────────

# Update a vendored submodule to latest and fix up Cargo.toml if needed
# Usage: just vendor-pull <path>
vendor-pull path:
    git submodule update --remote {{path}}

# ── Info ──────────────────────────────────────────────────────────────────────

# Show all workspace members
members:
    cargo tree --workspace --depth 0

# Show the crate tree for the engine
tree:
    cargo tree -p {{project}}

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
    cargo clean

# ── SceneDB inspector ────────────────────────────────────────────────────────

# Checkout of https://github.com/Far-Beyond-Pulsar/SceneDB (override with SCENEDB_DIR)
scenedb_dir := env_var_or_default("SCENEDB_DIR", "../SceneDB")
exe := if os() == "windows" { ".exe" } else { "" }

# Build the engine, then launch it under the SceneDB inspector (live CPU + GPU view)
inspect: build
    cargo run --release --manifest-path {{scenedb_dir}}/crates/scenedb_inspector/Cargo.toml -- target/debug/{{project}}{{exe}}
