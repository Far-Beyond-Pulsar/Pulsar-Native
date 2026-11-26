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

# Build the editor and every plugin (release): each plugin is its own DLL, built with the editor in one
# cargo invocation (their feature sets must match) and installed where a cargo-built editor looks:
# plugins/editor in the repository root. The shared libraries and std go beside the executable in
# target/release. Run the editor afterwards from the repository root as target/release/pulsar_engine.exe
# (a plain `cargo run` rebuilds it with another feature set, which the plugin DLLs no longer match).
# One folder serves both profiles: the profile built last is the one that is installed.
plugins:
    cargo run -p plugin_builder -- --release --out "{{justfile_directory()}}"

# The same in debug (not measured: the shared libraries may exceed the Windows export limit).
plugins-debug:
    cargo run -p plugin_builder -- --out "{{justfile_directory()}}"

# The same, installing the plugins into another folder (another checkout's root).
plugins-to out:
    cargo run -p plugin_builder -- --release --out "{{out}}"

# Run the engine
run:
    cargo run -p {{project}}

# Build the editor with EVERY installed plugin linked into the executable (release): no plugins/editor
# libraries and no plugin_builder run, one binary. The same plugins the DLL setup loads (plugins/installed
# with its editor registrations, render_bps among them). The shared libraries (wgpui, ui, profiling,
# reflection, SceneDB, Helio) stay shared libraries beside the executable, so each linked plugin uses
# the editor's copy of them. A library of a linked plugin left in plugins/editor by `just plugins` is
# skipped by the loader; a plugin that exists only as a library (hub-installed) still loads.
# Not the default: `just release` / `just plugins` are the DLL setup.
# The executable needs Rust's own std library next to it (every crate links it dynamically, `-C prefer-dynamic`):
# `cargo run` finds it through PATH, a double-clicked or directly launched executable does not, so the recipe
# copies it beside the executable, as plugin_builder does. Beside the executable afterwards: pulsar_engine,
# std-<hash>, the six shared libraries (*_dylib) and DirectML.dll. Run it from the repository root.
[windows]
build-linked:
    cargo build -p {{project}} --release --features link-plugins
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path "{{justfile_directory()}}" "target" }; $sysroot = (rustc --print sysroot); Copy-Item (Join-Path $sysroot "lib/rustlib/*/lib/std-*.dll") (Join-Path $target "release") -Force; Write-Host "Run $target/release/pulsar_engine.exe from {{justfile_directory()}}"

[unix]
build-linked:
    cargo build -p {{project}} --release --features link-plugins
    target="${CARGO_TARGET_DIR:-{{justfile_directory()}}/target}"; cp -f "$(rustc --print sysroot)"/lib/rustlib/*/lib/libstd-*.* "$target/release/" 2>/dev/null || true

# Build (as above) and run the editor with every plugin linked; arguments go to the editor:
#   just run-linked --project-path C:/path/to/project
# Run from the repository root (the editor finds plugins/ and assets/ there).
run-linked *args:
    cargo run -p {{project}} --release --features link-plugins -- {{args}}

# Open an example project from assets/examples (voxel_planet, voxel_moon, voxel_desert, voxel_blocks)
example name="voxel_planet":
    cargo run -p {{project}} --release -- --project-path assets/examples/{{name}}

# ── Test ──────────────────────────────────────────────────────────────────────

# Run all workspace tests
test:
    cargo test --workspace

# Interpreted vs generated Rust: both profiles, since overflow and wrapping
# behaviour is where a debug and a release build could disagree
conformance:
    cargo test -p pulsar_script_conformance
    cargo test -p pulsar_script_conformance --release

# Test a specific crate: just test-crate <name>
test-crate name:
    cargo test -p {{name}}

# ── Lint ──────────────────────────────────────────────────────────────────────

clippy:
    cargo clippy --workspace -- -D warnings

# Layer gate (architecture/layers.toml): core and editor depend on no plugin or
# tool and name no kind beyond architecture/kind-names.txt. Helio's checker, run
# from the submodule; `--update` records fixed violations.
arch *args:
    python crates/renderer/helio/scripts/architecture/check_layers.py --root . {{args}}

# Style ratchet (architecture/STYLE.md, architecture/style-baseline.json): function
# and file length, process comments, println, unwrap, Result<_, String>. Counts
# may only fall; `--update` lowers the baseline, `--report` prints the totals.
style *args:
    python scripts/style-check.py {{args}}

# Pulsar-Native's own crates; submodules format in their own repos (#1375).
fmt:
    scripts/fmt-check.sh --fix

# ── Codegen drift guard (#652) ────────────────────────────────────────────────
# Fast always-on probe: generated blueprint actors compiled against the
# pinned crates inside pulsar_game's own test binary.
ci-drift-probe:
    cargo test -p pulsar_game --lib blueprint_codegen_drift

# Heavy end-to-end check: generate a full game project into a temp dir and
# cargo-check it against current pins (catches manifest/patch-table drift the
# fast probes cannot). First run compiles the whole engine dep tree; later
# runs reuse a shared target dir.
ci-drift-check:
    cargo test -p pulsar_game --test generated_project_compiles -- --ignored --nocapture

# ── Packaging (#926) ──────────────────────────────────────────────────────────
# Package a project as a standalone game: `just package <project> <out>`.
package project out profile="shipping":
    cargo run -p pulsar_package --bin pulsar -- package --project {{project}} --out {{out}} --profile {{profile}}

# Package the smoke-test project, copy it away, run it headless and check
# begin_play, a spawn and an event handler ran (what CI runs).
package-smoke:
    cargo test -p pulsar_package --no-default-features --test smoke -- --nocapture

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

# Where the target directory's disk goes, and what `clean-stale` would remove (read-only).
# Arguments go to scripts/target-disk.ps1 (-TargetDir, -Keep, -ScanDir D:/ ...).
disk *args:
    pwsh -NoLogo -NoProfile -File scripts/target-disk.ps1 -ReportOnly {{args}}

# Prune a target directory to bounded size: keep the newest 3 builds of every crate, drop the rest,
# plus old incremental data, stale plugin_compat copies and idle game target dirs. Dry run by default;
# `just clean-stale --apply` deletes. `just clean-stale -ScanDir D:/ --apply` also removes idle scratch
# target dirs (tb*) next to it. See AGENTS.md, "Disk and target directories".
clean-stale *args:
    pwsh -NoLogo -NoProfile -File scripts/target-disk.ps1 {{args}}

# ── SceneDB inspector ────────────────────────────────────────────────────────

# Checkout of https://github.com/Far-Beyond-Pulsar/SceneDB (override with SCENEDB_DIR)
scenedb_dir := env_var_or_default("SCENEDB_DIR", "../SceneDB")
exe := if os() == "windows" { ".exe" } else { "" }

# Build the engine, then launch it under the SceneDB inspector (live CPU + GPU view)
inspect: build
    cargo run --release --manifest-path {{scenedb_dir}}/crates/scenedb_inspector/Cargo.toml -- target/debug/{{project}}{{exe}}

# Run tsc over the generated TypeScript declarations and sample classes (needs Node)
check-typescript-declarations:
    cargo test -p pulsar_script_ts --test tsc -- --ignored

# ── No git dependency on Pulsar-Native (#1662) ───────────────────────────────
# Fail if any manifest (Helio, a plugin, a vendored crate) takes Pulsar-Native
# as a git source; what CI runs.
check-pulsar-native-git-deps:
    bash scripts/no-pulsar-native-git-deps.sh

# ── Submodule pins ────────────────────────────────────────────────────────────
# Fail unless every submodule pins a commit on its upstream default branch (or
# is listed in .github/submodule-pin-exceptions.txt); what CI runs.
check-submodule-pins:
    bash scripts/submodule-pins.sh check
