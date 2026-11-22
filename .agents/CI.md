# CI

| Workflow | When | What |
|---|---|---|
| `ci.yml` | PR (non-draft), push to `main`, nightly, manual | fmt (Pulsar-Native's own crates, `scripts/fmt-check.sh`), workflow lint, clippy + guards + nextest on **Linux**, packaging smoke test, pin checks |
| `ci.yml` (`cross-platform`, smoke on macOS/Windows) | `main`, nightly, `gh workflow run CI -f full=true` | Windows/macOS compile check and packaging smoke test; never per PR |
| `security.yml` | PRs touching manifests; weekly | dependency review (needs the repo's Dependency graph enabled), `cargo audit` |
| `release.yml` | push to `main` with a version bump | full release gate and builds |
| `pr-release-guard.yml` | PR | blocks `[FORCE-RELEASE]` commits |
| `plugin-release.yml` | called by each plugin repo's `release.yml` (push to its `main` with a version bump, or manual) | builds the plugin root with `--features dll` for linux x86_64, windows x86_64/i686/arm64 and macos x86_64/arm64 and attaches the libraries and `sdk-versions.json` to the tag `v<version>`; written once here, called from all 34 plugin repositories |

Cost rules: branch pushes do not run CI (the PR does); a new push cancels the
superseded PR run; docs-only changes and draft PRs skip CI; tests run on Linux
only; Windows/macOS minutes are spent on `main` and nightly. Tests that need a
GPU are excluded by `.config/nextest.toml` (`ci` profile). Run a full
cross-platform pass on a branch with `gh workflow run CI --ref <branch> -f full=true`.

The Linux lane also runs `cargo run -p todo_check`. It scans Git-tracked
source and configuration files in this repository (not submodules) for TODO
comment markers without an issue reference, such as `#123` or a GitHub issue
URL. Existing unreferenced TODOs are reported too; they must be linked to
issues before this check can pass.

## Shared library releases

The shared UI, reflection, profiling, SceneDB and renderer libraries (`.agents/PLUGIN_SYSTEM.md`, "Shared
libraries") are released from the repository that owns each (`sdk-release.yml` there: on a version bump of its
`*_dylib` crate, six platforms, the library, its import library, the crate metadata consumers compile against
and a manifest of toolchain, flags and source revisions). Plugin releases do not build them for release; they
record the latest release of each in `sdk-versions.json`.

Known gap: Cargo compiles every crate in the graph, including the ones inside a released library, so a plugin
build still compiles the shared libraries' crates locally (cached) and only the release assets are published
once. Skipping that compile needs a `RUSTC_WRAPPER` that serves the released metadata; it is not built yet.
The shared-library mechanism is verified on windows x86_64 only: the other five platforms' cargo config
(`prefer-dynamic`, rpath) and the native-library exports are untested until their runners pass.
