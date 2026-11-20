# Contributing to Pulsar-Native

Community, discussions and the wider contribution process:
[Community](https://github.com/Far-Beyond-Pulsar/Community).

This file covers what you need in the tree itself.

## Get the code

Plugins, Helio and several dependencies are git submodules, and cargo cannot
load the workspace without them:

```
git clone --recursive <repo-url>
# or, in an existing clone:
just submodule-init
```

Install [just](https://github.com/casey/just). The Rust toolchain is pinned in
`rust-toolchain.toml`. On Linux, `scripts/install-linux-deps` installs the
system packages (written for Ubuntu 24.04).

## The `just` gates

| Command | What it does |
|---|---|
| `just check` | `cargo check` |
| `just build` | build `pulsar_engine` |
| `just test` | `cargo test --workspace` |
| `just clippy` | clippy over the workspace with `-D warnings` |
| `just arch` | layer gate: core and editor depend on no plugin or tool and name no kind beyond `architecture/kind-names.txt` |
| `just check-submodule-pins` | every submodule is pinned to a commit on its upstream main |
| `just fmt` | format Pulsar-Native's own crates (`scripts/fmt-check.sh --fix`) |

Run `just --list` for the rest. CI also runs `scripts/fmt-check.sh` (rustfmt)
and `todo_check` (TODOs need an issue reference).

## Submodule merge order

Merge dependency-first. A Pulsar-Native PR must never pin a dependency
commit that is not on that dependency's main:

1. merge the dependency PR into its repository's main;
2. re-pin the submodule here to that main commit;
3. merge the Pulsar-Native PR.

Details: "Submodule pins" in `.agents/CRATES.md`.

## Where things live

- `AGENTS.md`: project overview and index of the docs.
- `.agents/`: design and as-built docs per subsystem (some files are plans).
  `.agents/CRATES.md` is the crate layout, `.agents/CI.md` the CI rules.
- `crates/README.md`: map of `crates/`. `architecture/`: layer gate data.
- `docs/`: standalone notes. `diagrams/`: drawio sources.

## `scripts/` and `tools/`

- `scripts/`: shell scripts for CI and setup. `fmt-check.sh` (rustfmt over
  non-submodule crates), `submodule-pins.sh` (the pin check),
  `no-pulsar-native-git-deps.sh` (no manifest may take a git dependency on
  this repo), `install-linux-deps`, `sync-revs` (update `rev` pins in
  `Cargo.toml` to submodule HEADs) and `bootstrap` (deprecated).
- `tools/`: developer utilities that are not part of the build. Rust tools
  (`todo_check`, the CI TODO check; `set-logo`, run as `cargo set-logo
  <png>`), and PowerShell/shell helpers: `pre-release-check.*` (mirrors the
  release pipeline locally), `bump-plugins.ps1`, `dep_analysis.ps1`,
  `check_helio_scene_api_boundary.ps1`, `setup/init_mac.sh`.

## Configuration files

The tracked `.env` holds only the public GitHub OAuth client ID; see
`.env.example`. Do not add secrets to it.

## Conventions (enforced as a ratchet)

From `.agents/READABILITY_REPORT.md`. `just style` measures them and fails when
a file gets worse than `architecture/style-baseline.json` (shrink-only, like
`just arch`); fix existing violations and run `just style --update` to lock the
gain in. The rules, the limits and how to fix each are in
`architecture/STYLE.md`.

- functions at most 100 lines, files at most 600 (tests 1500); `lib.rs` and
  `mod.rs` should hold only `mod` lines, docs and `pub use` (not gated yet);
- no issue or phase references in comments (`Phase`, `P3`, `#1234`); that
  context belongs in commits and `.agents/` plans. A TODO is
  `TODO(#NNNN): what and why`;
- no `println!` in libraries (use `tracing`); no `Result<_, String>` in
  public APIs (use `pulsar_error`);
- no bare `.unwrap()` in non-test code: use `?` or `expect("why this cannot fail")`;
- prefer `foo.rs` plus `foo/` over `foo/mod.rs` for new modules.
