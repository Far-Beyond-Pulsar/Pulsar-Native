# Bump Dependency Chain

Get a change in Graphy or Plugin_Blueprints into Pulsar-Native.

Both are submodules of Pulsar-Native: Graphy at `crates/third-party/graphy`, Plugin_Blueprints at
`plugins/vendor/blueprint_editor`. Pulsar-Native uses them by path (the root `Cargo.toml` has a
path dependency and a `[patch]` entry for Graphy, and `crates/core/engine` /
`crates/core/pulsar_package` depend on the plugin by path). So bumping one means moving the
submodule pin, not editing a `rev =` in Pulsar-Native.

## The rule

Merge the dependency's PR first, then re-pin the submodule to a commit on the dependency's default
branch, then merge the Pulsar-Native PR. CI's "submodule pins are on upstream main" check
(`just check-submodule-pins`) fails otherwise; `.github/submodule-pin-exceptions.txt` lists the
rare allowed exceptions.

## After a Graphy change merges

```bash
git -C crates/third-party/graphy fetch origin main
git -C crates/third-party/graphy checkout origin/main
git add crates/third-party/graphy
```

The Blueprint plugin also names Graphy by git rev, in `plugins/vendor/blueprint_editor/Cargo.toml`
and `plugins/vendor/blueprint_editor/compiler/Cargo.toml`. Inside Pulsar-Native the `[patch]`
redirects them to the submodule, so they only matter for the plugin's own builds; when you change
them, change both to the same rev in a Plugin_Blueprints PR.

## After a Plugin_Blueprints change merges

```bash
git -C plugins/vendor/blueprint_editor fetch origin main
git -C plugins/vendor/blueprint_editor checkout origin/main
git add plugins/vendor/blueprint_editor
```

## After a Pulsar-Native change a plugin needs

Nothing to bump: plugins use Pulsar crates through `workspace = true` and build
inside this workspace. No manifest may take Pulsar-Native as a git source
(`just check-pulsar-native-git-deps`, #1662).

## Bumping SceneDB

SceneDB (`pulsar_scenedb`, `pulsar_scenedb_derive`, `scenedb_dylib`) is the submodule
`crates/external/scenedb`, and the workspace uses it by **path** (root `Cargo.toml`
`[workspace.dependencies]`, `[patch."https://github.com/Far-Beyond-Pulsar/SceneDB"]` and
`tools/workspace_hack/Cargo.toml`), as it does Profiling and Reflection. So the pin is the submodule's
commit. Two places still name a git rev, because **Helio** (a submodule with its own manifest) depends on
SceneDB by git: `crates/renderer/helio/Cargo.toml` (both crates) and
`crates/renderer/helio/crates/helio_dylib/Cargo.toml` (`scenedb_dylib`). The root patch redirects
that git source to the path, so those revs must equal the submodule's commit (Helio is a submodule, so
**commit it first**: Helio PR merged to Helio main, then re-pin the submodule, then the Pulsar-Native PR).

A generated game gets the same SceneDB: `engine_backend/build.rs` bakes the workspace dependency
(absolute path) and the whole `[patch]` table into every game manifest, so a game resolves SceneDB
from the engine checkout and not from a git checkout of its own. Nothing in the game tooling names a
SceneDB rev.

The old and new commits cannot coexist, so a SceneDB bump and the code it needs go in one
Pulsar-Native PR. Plugins carry no SceneDB pin. Merge order: SceneDB PR to SceneDB main (take the sha
after any squash) → Helio rev bump merged and re-pinned → move the `crates/external/scenedb`
submodule → Pulsar-Native PR.

```bash
git -C crates/external/scenedb rev-parse HEAD
grep -n 'rev = ' crates/renderer/helio/Cargo.toml crates/renderer/helio/crates/helio_dylib/Cargo.toml | grep -i scenedb   # same sha
```

(`.agents/COMPONENT_LIFECYCLE_PLAN.md`, "As built (P5b)", is the worked example: SceneDB's
`Actor`/`ActorRegistry` removal.)

## Bumping Pulsar-Profiling

The `profiling` crate's one home is the public Pulsar-Profiling repository (#1662). Pulsar-Native
has it as the submodule `crates/external/pulsar-profiling`, and the root `Cargo.toml`'s
`[patch."https://github.com/Far-Beyond-Pulsar/Pulsar-Profiling"]` points every copy at that
submodule. Helio pins it by `rev =` in `crates/renderer/helio/Cargo.toml`, in its dependency and in
its own `[patch]` (same sha in both); SceneDB pins its own rev, which the patches override.

Merge order: Pulsar-Profiling PR to its main → Helio rev bump merged and re-pinned → submodule
re-pinned and the Pulsar-Native PR merged.

```bash
grep -rn 'Pulsar-Profiling' --include=Cargo.toml Cargo.toml crates/renderer/helio/Cargo.toml
git -C crates/external/pulsar-profiling rev-parse HEAD
```

---

## Rules

1. **Take SHAs from git, never from memory** (`git rev-parse HEAD`, `git -C <submodule> rev-parse HEAD`).
2. **Don't remove the root `[patch]` sections.** They keep one copy of each shared crate in the graph.
3. **Order matters:** dependency PR merged → submodule re-pinned → Pulsar-Native PR merged.

## Verifying a rev exists

```bash
git -C crates/third-party/graphy cat-file -t <SHA>   # should print "commit"
```

## Common error: duplicate crate versions

```
note: there are multiple different versions of crate `graphy` in the dependency graph
```

See `/debug-compile-errors`. Start with:
```bash
grep -rn 'rev =' --include=Cargo.toml plugins/vendor
cargo tree -i graphy -e normal
```
