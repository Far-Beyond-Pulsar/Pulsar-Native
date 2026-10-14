# CI

| Workflow | When | What |
|---|---|---|
| `ci.yml` | PR (non-draft), push to `main`, nightly, manual | fmt, workflow lint, clippy + guards + nextest on **Linux**, packaging smoke test, pin checks |
| `ci.yml` (`cross-platform`, smoke on macOS/Windows) | `main`, nightly, `gh workflow run CI -f full=true` | Windows/macOS compile check and packaging smoke test; never per PR |
| `security.yml` | PRs touching manifests; weekly | dependency review, `cargo audit` |
| `release.yml` | push to `main` with a version bump | full release gate and builds |
| `pr-release-guard.yml` | PR | blocks `[FORCE-RELEASE]` commits |

Cost rules: branch pushes do not run CI (the PR does); a new push cancels the
superseded PR run; docs-only changes and draft PRs skip CI; tests run on Linux
only; Windows/macOS minutes are spent on `main` and nightly. Tests that need a
GPU are excluded by `.config/nextest.toml` (`ci` profile). Run a full
cross-platform pass on a branch with `gh workflow run CI --ref <branch> -f full=true`.
