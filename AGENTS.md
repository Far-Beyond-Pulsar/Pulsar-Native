# Repository Guidelines

## Project Structure & Module Organization

- `Cargo.toml` is a Rust workspace; the default app is `crates/engine` (`pulsar_engine`).
- `crates/` contains engine/runtime code (e.g. `engine_backend`, `engine_state`, `engine_fs`, `pulsar_std`).
- `ui-crates/` contains modular UI crates (e.g. `ui_core`, `ui_editor`, `ui_settings`).
- `plugins/` contains editor/plugin crates (e.g. `plugins/blueprint_editor_plugin`).
- `assets/` images + SVG icons, `themes/` theme JSON presets, `scenes/` sample scene JSON.
- `docs/` contains design notes and architecture docs.

## Build, Test, and Development Commands

- `cargo run --release` — run the main app (recommended default; see `README.md`).
- `cargo run` — run in debug.
- `cargo build -p blueprint_editor_plugin` — build a specific plugin crate.
- `cargo test --all` — run all workspace tests (mirrors CI).
- `cargo fmt --all` — format Rust code with `rustfmt`.
- `cargo clippy -- --deny warnings` — lint; keep the workspace warning-free.
- Linux deps: `./script/install-linux-deps` (Ubuntu; required for windowing/WebKit stacks).
- Optional checks used in CI: `typos` (spelling) and `cargo machete` (unused deps).

## Coding Style & Naming Conventions

- Workspace mixes Rust 2021/2024 crates; use `rustfmt` defaults (4-space indentation, trailing commas, etc.).
- Follow workspace Clippy lints (avoid `dbg!`, don’t land `todo!()`/unchecked warnings).
- Naming: `snake_case` for modules/functions, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- UI work: follow native desktop conventions (macOS/Windows patterns); see `CONTRIBUTING.md`.

## Testing Guidelines

- Use Rust’s built-in test harness (`#[test]`).
- Prefer unit tests near code (`mod tests { ... }`) and integration tests under `*/tests/`
  (example: `crates/pulsar_std/tests/registry_test.rs`).
- Name tests by behavior (e.g. `it_loads_registry_from_disk`).

## Commit & Pull Request Guidelines

- Commit messages in this repo are short, imperative summaries (e.g. “Add …”, “Fix …”, “Refactor …”).
- Keep PRs focused: one PR should do one thing; include reproduction/verification steps.
- For UI/visual changes, attach screenshots or short recordings.
- If you used AI assistance, clearly label generated sections and ensure a human review (see `CONTRIBUTING.md`).

## Configuration & Platform Notes

- Toolchain is pinned via `rust-toolchain` (`nightly`; required by Bevy and Rust 2024 edition crates).
- CI lives in `.github/workflows/` and is a good reference for platform dependencies and checks.
