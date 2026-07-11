# Pulsar-Native development commands
# Usage: just <command>
# Install just: https://github.com/casey/just

project := "pulsar_engine"

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
    cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys; ms=json.load(sys.stdin)['packages']; [print(m['name'],m['manifest_path']) for m in ms]"

# Show the crate tree for the engine
tree:
    cargo tree -p {{project}}

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
    cargo clean
