# Game project generation: audit and plan

**Status: Phase 0 — audit and design. No code changes.** Every claim about
current code below was read for this document, with `file:line` against
`main` at `0f5d8b3b4`. The decisions in section 4 are confirmed by the user.

The goal: one owner for every file the engine generates in a game project; a
model of the project computed from declared inputs; deterministic output
applied with a record of what was written; generation that runs whenever its
inputs change, not only when someone remembers to call it; and projects that
build on any machine.

---

## 0. Findings (read these first)

1. **Five writers, no owner.** These files are written by five pieces of code
   with different rules:

   | Writer | Writes | Rule |
   |---|---|---|
   | `engine_backend/src/services/core_project_builder.rs` (1563 lines, 58 revisions) | `Cargo.toml`, `src/{main,lib,plugins,engine_main}.rs`, `src/classes/mod.rs`, `Pulsar/level.json`, starter `scripts/<x>/` | marker comments, line-based TOML editing |
   | `blueprint_editor/.../compilation/compiler.rs:357-450` (`compile_to_class_directory`) | `<class>/events/{events,mod}.rs`, `<class>/mod.rs`, **the class directory's parent `mod.rs`** | overwrite, or append |
   | `blueprint_editor/.../variables/operations.rs:152` (`generate_vars_module`) | `<class>/vars/mod.rs` | overwrite |
   | `pulsar_script_codegen/src/actor.rs:310` (`class_files`) | `src/classes/<ident>/{mod.rs,events/*}` (a **different layout** from the editor's: no `vars`, snake_case ident instead of the raw class name) | used only by the drift test |
   | Project templates (`Template-Blank`) | committed copies of generated files | frozen at whatever engine version committed them |

2. **The Rust a Blueprint compiles to is not compiled for most classes.** A
   class is `src/classes/<Name>/` *or* a `<Name>.class/` folder anywhere
   (`class_dirs.rs:3-7`); the content browser creates the latter. Its Rust goes
   into that folder, outside `src/`, so no crate includes it. The compiler then
   appends `pub mod <Name>;` to `class_path.parent()/mod.rs`
   (`compiler.rs:431-447`), which for `<project>/Door.class` is
   `<project>/mod.rs`, a stray file. The runtime runs the VM module
   (`events/.build/module.json`); the Rust is dead output that can still break
   `cargo build` when it is under `src/classes`.

3. **Generated files carry absolute paths to one engine checkout.**
   `engine_backend/build.rs` bakes the workspace's dependency specs with
   `path`s made absolute at **engine compile time** (`build.rs:345-358`), and
   the scripts crate scaffold writes `pulsar_game = { path = "<abs>" }` from
   `env!("CARGO_MANIFEST_DIR")` (`core_project_builder.rs:837-878`) into a file
   the user owns and the builder never touches again. The Blank template has
   it committed:
   `Template-Blank/scripts/test_scripts/Cargo.toml` →
   `pulsar_game = { path = "C:/Users/redst/OneDrive/Documents/GitHub/Pulsar-Native/..." }`.
   Clone the project to another machine, or move the engine, and it does not
   build. `remove_missing_local_patch_entries` (`:407`) works around the
   symptom by deleting patch entries whose path vanished.

4. **Ownership is guessed from comments.** The root `Cargo.toml` is
   regenerated only if it contains one of two marker strings (`:498-504`);
   otherwise it is "user-owned" and edited line by line
   (`ensure_generated_dependencies`, `merge_baked_patches`, `with_plugin_block`
   — three hand-written TOML line editors). `lib.rs` and `engine_main.rs` have
   their own opt-out markers (`:1112-1122`, `:1148`), but `engine_main.rs` is
   rewritten whether or not the marker is present (`:1267`). A user who removes
   a marker freezes the file at that engine version forever.

5. **Generation runs only on three paths, and not on the one that changes it
   most.** `ensure_core_bootstrap` is called by the Build window (an optional,
   user-toggled step: `ui_build/src/runner/plan.rs:248`), by Play
   (`ui_level_editor/src/ui/panel/pie.rs:265`) and by `pulsar package`
   (`pulsar_package/src/lib.rs:405`). Not when the project opens, not when a
   plugin is enabled in `Pulsar.toml`, not when a scripts crate is added, not
   when the engine is upgraded. Plain `cargo build` in the project builds
   whatever was last written.

6. **Nothing is deleted.** Generation only writes. A removed class keeps its
   `events.rs`; the only cleanup is the `classes/mod.rs` rescan (`:613-640`),
   which the Blueprint compiler then appends to (finding 1), so the two fight.

7. **Output is not deterministic.** `events/mod.rs` embeds
   `chrono::Local::now()` (`compiler.rs:396-405`): every compile changes it, so
   the PIE fast path (`pie.rs:392`, newest `.rs`/`.toml` mtime) rebuilds.

8. **Fresh projects resolve fresh crates.** The generated project has no
   lockfile, so cargo resolves today's newest semver-compatible versions, not
   the engine's. Hence `pin_compatible_dependencies` (`:781`: string-replace
   pins for `find-msvc-tools` and `libc`), added after CI broke.

9. **Identity is read three ways.** Cargo's package name comes from
   `ProjectSettings` `project.name` (`:464`), `main.rs` and the scripts crate
   from the folder name (`:645`, `:826`), the PIE fast path by scanning lines
   for the first `name =` (`pie.rs:431`, which matches `[[bin]] name` or a
   dependency key if the order changes), the packager by `[[bin]]` ending in
   `_game` (`pulsar_package/src/lib.rs:495-510`).

10. **Legacy left in.** `Pulsar/level.json` `prefabs` spawn
    `classes::X::new()` native actors (`:1155-1199`), from before the script VM
    and placed class instances; every template ships it empty.
    `generate_vars_module` emits `Cell`/`RefCell` statics that nothing reads.
    `CompileMode::DirectRust` (`compiler.rs:528`) is a second compile target
    that produces finding 2.

11. **The Blank template is not a valid project.** It has no `Pulsar.toml`
    (`engine/src/uri/parser.rs:78` rejects it), ships generated
    `src/{main,lib,engine_main}.rs` that are already stale (`main.rs` has no
    `mod plugins;`), and ships two scripts crates, one with the path from
    finding 3.

12. **Engine installs cannot build games.** Every engine dependency in a game
    manifest is a path into the source checkout the editor was compiled from.
    A Hub install (`pulsar_install_layout`) has no sources, so a project opened
    in it has nothing to point at.

The root cause behind all twelve: there is no model of a project. Each writer
reads what it needs from wherever it is, writes text, and hopes the next
writer agrees.

---

## 1. Reference: how shipped engines do it

| | Source of truth (committed) | Generated (not committed) | When |
|---|---|---|---|
| Unreal | `.uproject` (plugins, engine association), `Source/*.Build.cs` | `.sln`/`.vcxproj`, `Intermediate/` | `GenerateProjectFiles` on open, on plugin change, on engine switch; UBT re-derives at build |
| Unity | `Packages/manifest.json`, `packages-lock.json` | `Library/`, `.csproj` | on every domain reload; lock pins resolved versions |
| Godot (C#) | `project.godot`, `.csproj` (user-owned, SDK-referenced) | `.godot/` | SDK resolved by version, not by path |

What they share, and what Pulsar lacks: one descriptor the user edits; every
generated file is derived, disposable, out of the way, and never hand-edited;
the engine is referenced by **identity** (version or association), resolved to
a location on the machine at generation time; and generation is triggered by
the editor on every input change.

---

## 2. Design

### 2.1 Ownership

| Path | Owner | Committed | Notes |
|---|---|---|---|
| `Pulsar.toml` | user (editor writes through a typed API) | yes | identity, engine requirement, plugins, build options |
| `scripts/<crate>/**` | user | yes | depends on engine crates with `{ workspace = true }`: no paths |
| content (`*.class/`, `scene/`, assets, `Pulsar/project.json`) | user / editor tools | yes | Blueprint compile writes **only** `events/.build/module.json` |
| `Cargo.toml` (root) | generator | yes | virtual workspace: members, `[workspace.dependencies]`, `[patch]`; every path relative, into `.pulsar/sdk/` (2.3) |
| `Cargo.lock` | generator seeds, cargo updates | yes | seeded from the engine's own lockfile; holds no paths |
| `.pulsar/sdk/**` | generator, per machine | no | the engine SDK mirror: the only thing rebuilt per machine (2.3) |
| `.pulsar/build/game/**` | generator | no | the game crate: `Cargo.toml`, `src/{main,lib,plugins,scripts}.rs`, nativized classes |
| `.pulsar/build/generated.json` | generator | no | the record (2.4) |
| `.gitignore` entries | generator adds once | yes | `/target`, `/.pulsar/sdk/`, `/.pulsar/build/` |

No marker comments, no opt-outs. A user who wants different launcher code
writes it in a scripts crate (2.6); a user who wants a different build setting
sets it in `Pulsar.toml`.

### 2.2 One crate: `pulsar_project` (crates/core)

Replaces `core_project_builder.rs`, the generation half of `native_scripts.rs`,
and the baking half of `engine_backend/build.rs`. Knows no plugin kind (reads
`pulsar_plugins` descriptors, as today; `just arch` holds).

```
pulsar_project
  descriptor.rs   Pulsar.toml: typed read/write (toml_edit, formatting kept), format version
  sdk.rs          SDK index, EngineSdk::locate, the .pulsar/sdk mirror + normalization (2.3)
  model.rs        ProjectModel::resolve(root, &EngineSdk) -> ProjectModel   (reads inputs only)
  render/         ProjectModel -> FileSet   (pure; one module per file; toml via typed structs)
  apply.rs        FileSet + record -> writes, deletes, drift report (2.4)
  sync.rs         sync(root, Trigger) -> SyncReport   (the one public entry point)
  migrate.rs      format 1 -> 2 (section 5, P4)
  create.rs       create(root, Template) for new projects
```

`ProjectModel` is plain data: name (from `Pulsar.toml`, the one identity),
crate idents, engine fingerprint and SDK index, enabled runtime plugin roots,
scripts crates (by parsing their manifests with `toml`), nativized classes
(2.6), build profile. `render` cannot touch the filesystem, so it is
golden-testable and deterministic: no clocks, sorted iteration, stable
formatting.

### 2.3 The engine SDK, mirrored into the project

**No Cargo file anywhere in a project holds a machine-specific path.** The
engine crates a game links are mirrored into the project at `.pulsar/sdk/`,
and every Cargo path points there, relative to the project:

```toml
# <project>/Cargo.toml (generated, portable, committed)
[workspace]
members  = [".pulsar/build/game", "scripts/*"]
exclude  = [".pulsar/sdk"]
resolver = "2"

[workspace.dependencies]
pulsar_game = { path = ".pulsar/sdk/crates/core/pulsar_game" }
lights_plugin = { path = ".pulsar/sdk/plugins/vendor/lights" }
# ...

[patch."https://github.com/Far-Beyond-Pulsar/..."]
some_crate = { path = ".pulsar/sdk/crates/third-party/..." }   # path patches relative too; git patches stay git
```

The Cargo files are the same on every machine for a given engine version and
`Pulsar.toml`. What is rebuilt per machine is the **SDK mirror**, not Cargo:

- **The SDK index** (`sdk::SdkIndex`): the engine's local packages a game's
  dependency graph activates, each with its SDK-relative directory; the name
  of every crate in that graph; the engine workspace's `[workspace.package]`,
  `[workspace.dependencies]`, `[patch]` and `[profile]` tables; the installed
  plugins' descriptors; the engine's `Cargo.lock`. Its fingerprint (a hash
  of all of it) identifies the engine a project was generated from.
  - **Cargo computes the graph**, not a re-implementation of its resolver:
    `cargo tree --locked -e normal,build --target all` from the game crate's
    engine dependencies and every installed runtime plugin root. Features,
    targets, optional dependencies and `[patch]` redirects are Cargo's own
    answer. Verified first: Cargo reads no manifest of an *inactive* optional
    path dependency of a package that is not a workspace member, which every
    mirrored package is, so the editor-only side of a plugin (its `editor`
    feature's crates) is never mirrored.
  - **Each package inherits from its own workspace**, found the way Cargo
    finds it (`package.workspace`, else the nearest `[workspace]` at or above
    it that does not exclude it). That is not always the engine's: Helio is a
    workspace of its own (excluded from the engine's), and its crates inherit
    from it. The index carries every such workspace's tables
    (`SdkIndex::workspaces`) and each package's (`SdkPackage::workspace`).
  - **Installed plugins** are the normal dependencies of the engine's
    `installed_plugins` crate (the set the editor links), not every workspace
    member with a descriptor: the workspace also holds test-fixture plugins.
  - For a checkout the index is built on demand and cached in the mirror's
    record, keyed by a hash of the workspace manifest, the lockfile, and each
    indexed package's manifest and descriptor; a package can only become
    reachable through a change to one of those. For an exported SDK it is
    `pulsar-sdk.json`, written by `EngineSdk::export` (the release pipeline's
    call) from the same code.
- **The SDK source** on this machine, found by `EngineSdk::locate()`:
  `PULSAR_SDK` when set; an install's `sdk/`
  (`InstallLayout::sdk_dir`, added to the Hub/engine contract); or the source
  checkout, found by walking up from the running executable (a workspace root
  whose `[workspace.dependencies]` has `pulsar_game` at a path). Never
  `CARGO_MANIFEST_DIR`. Missing → a clear error; nothing is written.
- **The mirror** (`.pulsar/sdk/`, gitignored): each indexed package copied
  under its SDK-relative directory, so the index's relative paths just gain a
  `.pulsar/sdk/` prefix. Copied: everything in the package directory except
  `target/`, dot-files and dot-directories, the package's top-level `tests/`,
  `benches/` and `examples/`, and nested packages (mirrored on their own when
  the game uses them). Each manifest is **normalized** the way `cargo package`
  does it: inherited `workspace = true` values resolved (path dependencies
  rebased onto the package), and dev-dependencies, lints, test/bench/example
  targets and workspace-root tables removed. An SDK package's lints are not
  the game's: its `deny`s must not fail a game build. Normalizing is
  idempotent, so an exported (already normalized) SDK mirrors through the
  same code.
- **Mirroring is incremental**: `.pulsar/sdk/sdk.json` records, per file, the
  source's size and modification time and the output's size. A file is
  rewritten when its source changed, its output is missing or resized, or
  (manifests) the index changed; files the SDK no longer has are deleted with
  any folder left empty. A fresh clone, a new machine or an engine upgrade
  rebuilds the mirror on the next sync; an unchanged one costs a stat walk.
  The mirror is per-machine Cargo input, so it is written to the real disk
  (`sdk/fs.rs`, the crate's one `std::fs` module), never through a remote
  project's provider.
- Scripts crates say `pulsar_game = { workspace = true }`, resolved through the
  root `[workspace.dependencies]`.

Cargo's own lockfile records path dependencies without a path, so
`Cargo.lock` is portable too. It is seeded from the engine's when the project
has none or the engine fingerprint changed: the engine's entries win, and the
project keeps only the crates the engine does not lock at all (a second
version of an engine crate would make the engine's bare-name references
ambiguous, so it is dropped, and so is any kept entry that refers to such a
crate by bare name). Cargo prunes and extends it on the next build.

What the root manifest carries from the engine is what applies to the game's
graph: a `[workspace.dependencies]` or `[patch]` entry only for a crate in the
graph (or one the game crate names), a local one only when the mirror has it;
`[profile.*.package]` overrides only for crates in the graph, so Cargo has
nothing to warn about.

The reflection-alias patch (`build.rs:186-195`) and the `libc` /
`find-msvc-tools` pins (finding 8) are not carried over: the seeded lockfile
pins every version the engine uses, and the SceneDB reflection split is fixed
at its source (bump SceneDB's `pulsar_reflection` rev) before P2 lands.

### 2.4 Apply, and the record

`.pulsar/build/generated.json`:

```json
{ "format": 1,
  "engine": "<SDK index fingerprint>",
  "files": { "Cargo.toml": "<sha256>", ".pulsar/build/game/src/main.rs": "<sha256>" } }
```

`apply` is two steps, `plan` (reads, writes nothing) and `commit`:

1. Each file whose content differs from disk is written, atomically
   (`virtual_fs::write_file_atomically`).
2. Each file in the old record that the new `FileSet` no longer has is
   deleted, with any folder that leaves empty (finding 6).
3. A file on disk that differs from the hash the record says was written was
   hand-edited: it is overwritten and reported as drift.
4. A file that exists, differs, and is **not** in the record was not written
   by the generator (a format-1 project's `Cargo.toml`, a user's file): the
   sync fails with `SyncError::Unowned` before writing anything, the SDK
   mirror included. Migration (P4) is what takes those over.
5. The record is written last. An interrupted sync leaves the old record, so
   the next sync redoes the work.

### 2.5 Triggers

`sync` is cheap when nothing changed, with no separate input hash to keep
correct: the SDK index is reused while the checkout files it was built from
are unchanged, the mirror compares file stamps, and the generated files are
rendered in memory (a handful of small files) and written only where they
differ. So it is called everywhere:

| Trigger | Where |
|---|---|
| Project opened (including after an engine upgrade) | an engine `InitGraph` step after the project is mounted |
| `Pulsar.toml` changed (plugin enabled/disabled, settings) | `engine_fs` watcher on the project |
| a `scripts/*/Cargo.toml` added, removed or changed | same watcher |
| before Build, Play, Package | `ui_build` runner, `pie.rs`, `pulsar_package` (no-op when current) |
| command line | `pulsar project sync [--force]` so `cargo` works without the editor |

The Build window's "Regenerate core project files" toggle becomes a
"Force regenerate" command; the step is no longer optional. Each sync runs as
an editor task with its own span log (`.agents/EDITOR_TASKS.md`).

### 2.6 What the game crate is

`.pulsar/build/game/` is glue only:

- `main.rs` → `pulsar_game::standalone::run(setup)`, the `[[bin]]` named
  after the project (`star_fall`, so a packaged game's executable carries its
  name); `lib.rs` → the PIE `pulsar_pie_*` shim (unchanged ABI), the `[lib]`
  `<package>_pie`; `plugins.rs` → `use <root> as _;` per enabled runtime
  plugin.
- `setup.rs` → `setup()` calls each scripts crate's `register_scripts`, then
  `game.enable_project_scripting()`. Replaces `engine_main.rs`; the
  `Pulsar/level.json` prefabs path is deleted (finding 10).
- `classes/<ident>.rs` → only for classes whose saved `compile_target` is
  `rust` (Q2), generated from `module.json` through `pulsar_script_codegen` at
  sync time. The Blueprint editor never writes Rust.

The drift probe (`pulsar_game/build.rs`) stays: it is the fast compile check of
the codegen against the real `NativeActor`.

### 2.7 Callers get facts, not guesses

`SyncReport` carries the package name, the `[[bin]]` name and the lib target
name. PIE and the packager read them from there instead of scanning the
manifest (finding 9). The PIE fast path keeps its "skip cargo when nothing
changed" behaviour, keyed on `SyncReport::changed()` plus the scripts crates'
source mtimes instead of every `.rs`/`.toml` under the project; the artifact
path comes from cargo's `--message-format=json` (`compiler-artifact`), not
from `PieHost::output_dylib_path` guessing.

---

## 3. What gets deleted

- `engine_backend/src/services/core_project_builder.rs` (all of it) and its
  re-export; `engine_backend/build.rs` game baking (`GAME_DEPENDENCIES`,
  `build_game_manifest_deps`, `build_game_plugins`, reflection alias,
  `with_features`); `remove_missing_local_patch_entries`,
  `pin_compatible_dependencies`, the three TOML line editors.
- `native_scripts.rs` generation role (discovery of script *actors* for the
  add-object flow stays, moved next to its consumer or kept as a read API on
  `ProjectModel`).
- Blueprint editor: `compile_to_class_directory`, `generate_vars_module`, the
  parent `mod.rs` append, the timestamp header (the `CompileMode` picker
  stays, now persisted — Q2).
- `pulsar_script_codegen::actor::class_files` (the second layout).
- `Pulsar/level.json` and `LevelPrefabEntry`/`LevelConfig`.
- `pie.rs` `any_source_newer`, `read_crate_name`.
- Template-Blank: `src/`, the absolute-path scripts crate, `Pulsar/level.json`,
  committed `events/*.rs`, `vars/`, `mod.rs` in `NewBlueprintClass.class`.

---

## 4. Decisions

- **Q1. Where a Hub-installed engine's crates come from. Decided:** each
  install ships `sdk/`, a normalized source snapshot of the crates a game
  links plus their path `[patch]` sources and the `Cargo.lock`, built by the
  release pipeline. It is the source `.pulsar/sdk/` is mirrored from (2.3);
  projects never point at it directly.
- **Q2. Blueprint → Rust export. Decided:** the Blueprint editor's existing
  Rust/VM picker (`CompileMode`, `blueprint_editor/src/core/types.rs:152`)
  stays the control. Today it is panel state reset on open
  (`editor/panel.rs:539`); it becomes a per-class setting saved in the class's
  `graph_save.json` (`"compile_target": "vm" | "rust"`, absent = `vm`). Compiling
  always writes `module.json`; the editor writes no Rust. The generator reads
  each class's `compile_target` and emits `.pulsar/build/game/src/classes/<ident>.rs`
  for the `rust` ones, so changing the picker and compiling triggers a sync.
- **Q3. Machine-specific Cargo paths. Decided:** there are none. The SDK is
  mirrored into `.pulsar/sdk/` (gitignored, rebuilt per machine) and every
  Cargo path is project-relative, so the root `Cargo.toml` and `Cargo.lock`
  are identical on every machine and committed; an engine upgrade shows up as
  a reviewable diff. `pulsar project sync` rebuilds the mirror and the game
  crate for command-line users after a clone.

---

## 5. Phases

Per the standing rule, every phase is built; issues that do not block are
logged in section 6 and fixed in a cleanup pass.

- **P1 — `pulsar_project` crate.** descriptor, sdk, model, render, apply,
  sync, the record. Tests: golden output per file; idempotency (second sync
  writes nothing, reads only inputs); deletion of files no longer produced;
  drift detection; interrupted sync; no absolute path in any file the
  generator writes outside `.pulsar/sdk/sdk.json`; the same project synced from
  two different directories gives byte-identical `Cargo.toml` and
  `Cargo.lock`; SDK mirror: incremental copy, deletion, manifest normalization
  (no `workspace = true` left in a mirrored manifest), rebuilt after deletion.

  **As built.** `crates/core/pulsar_project`: `descriptor` (Pulsar.toml
  `[project]` name/version + `[plugins]`), `sdk` (`index`, `normalize`,
  `mirror`, `fs`, `EngineSdk::{locate, at, index, export}`), `model`,
  `render` (`root`, `game`, `lock`), `apply` (`plan`/`commit`, the record),
  `sync` (`sync`, `sync_with`, `SyncOptions { force }`, `SyncReport`). Not
  wired to any caller yet (P2). Tests:
  - unit (paths, TOML writer round-trip, descriptor, package names,
    versions, tree-line parsing including proc-macro lines and paths with
    parentheses, normalization and its idempotence, lock seeding);
  - `tests/sync.rs` against a miniature engine checkout (stub crates, a
    proc-macro, an installed and an uninstalled plugin, an editor-only
    plugin, an optional editor crate, a `[patch]`, profile overrides), with
    Cargo resolving the result offline: portable output, second sync writes
    nothing, two locations byte-identical, engine source edits and removals
    and a new dependency reach the mirror, deleted mirror rebuilt, drift
    restored and reported, plugins follow Pulsar.toml, an unowned
    `Cargo.toml` refused with nothing written, scripts crate checks, a class
    compiled to Rust generated and removed, an exported SDK generating the
    same project as its checkout;
  - `tests/real_engine.rs` (`--ignored`): sync against this checkout and
    `cargo check --workspace` the result, with a scripts crate and a class
    compiled to Rust.

  **Real-engine check (this checkout, Windows).** The game's graph is 165
  local packages; the mirror is 4,110 files, 38.8 MiB. First sync (index
  built with Cargo, full mirror) 5.4-9.1 s; a second sync, nothing changed,
  0.43-0.50 s and writes nothing. Cargo resolves the generated workspace
  (root manifest, patches, profiles, seeded lockfile, normalized mirror) and
  starts compiling; the check then fails in `engine_backend`'s build script,
  `window_manager` and `meshes_component`, packages reading files outside
  their own directory (section 6, issues 1-3a), which the old builder never
  exposed because its games pointed into the engine checkout. With those
  fixes simulated in a kept copy of the generated project (the old build
  script stubbed, the escaped files supplied where the includes look),
  `cargo check --workspace` of the whole project passes: the game crate,
  the scripts crate and the class compiled to Rust. So the real-engine check
  passes once issues 1-3a land in P2; nothing else in the generated project
  or the mirror fails. `PULSAR_REAL_CHECK_DIR=<dir>` keeps the project for
  that kind of inspection, and the check runs with `--keep-going` so one
  failing crate does not hide the others.

  Departures from section 2 as first written: the index is computed by
  Cargo (`cargo tree`), not embedded by a build script; the generation record
  carries no input hash (rendering is cheap; see 2.5); `scripts.rs` is
  `setup.rs`; the executable is named after the project and the library
  `<package>_pie`.
- **P2 — switch every caller, delete the old builder.** Build runner, PIE,
  packager, engine startup step, watcher triggers, `pulsar project sync`.
  `core_project_builder.rs` and the `engine_backend/build.rs` baking go in the
  same change; compile errors find the callers. `generated_project_compiles`
  drives `create` + `sync` instead of hand-assembled files.
- **P3 — Blueprint editor writes data only.** The picker's choice is saved per
  class in `graph_save.json` and loaded with the class; `compile_to_class_directory`,
  the vars module and every `mod.rs` write are removed; a `rust` class is
  nativized by the generator.
  (Submodule: `plugins/vendor/blueprint_editor`; merges dependency-first.)
- **P4 — migration and templates.** `Pulsar.toml [project] format = 2`. On
  open, a format-1 project is migrated once, with a report: generated root
  files moved to `.pulsar/trash/`; each scripts crate's `pulsar_game = { path
  = ... }` rewritten to `{ workspace = true }` (that key only, via
  `toml_edit`); a root `Cargo.toml` without markers has its extra dependencies
  listed in the report for the user to move into a scripts crate;
  `Pulsar/level.json` with non-empty prefabs is reported, not dropped
  silently. Template-Blank rewritten to user-owned files only, with a
  `Pulsar.toml`. `create()` instantiates templates.
- **P5 — CI and docs.** A job instantiates every template, syncs, and
  `cargo check`s it (extends `just ci-drift-check`), plus a format-1 fixture
  (Template-Blank as it is today) through migration + check.
  `.agents/PROJECT_GENERATION.md` (as built) and an `AGENTS.md` row.

## 6. Issues log

Found while building P1. Each names the phase that fixes it. **Blocks the
real-engine check** means `tests/real_engine.rs` cannot pass until it is
fixed.

### Packages that read outside their own directory

The mirror holds each package's own directory, as `cargo package` would
publish it. Anything a package reads above its directory at build time
exists in the engine checkout but not in a project's `.pulsar/sdk/`.
Found by scanning the 165 local packages of the game's graph (their
`build.rs`, `include_str!`/`include_bytes!` and `CARGO_MANIFEST_DIR`
joins) and confirmed by the real-engine check.

1. **`crates/core/engine_backend/build.rs` reads the engine workspace's
   `Cargo.toml` through `../../..`.** *Blocks the real-engine check:* the
   build script panics in the mirror (`failed to read Pulsar workspace
   manifest .pulsar/sdk/crates/core/engine_backend/../../../Cargo.toml`).
   Everything it produces serves the old builder only: the baked game
   manifest, the baked plugin list, and `PULSAR_HELIO_GIT_REVISION`, which
   only `core_project_builder.rs` (`HELIO_GIT_REVISION`) and that file's own
   test read. **P2** deletes the whole build script together with the
   builder; nothing needs to replace the Helio revision.
2. **`crates/core/window_manager/src/configs.rs:13,16` and
   `crates/editor/ui_common/src/menu/mod.rs:863` embed
   `assets/images/logo_sqrkl*.png` from the repository root.** These are
   editor crates that the game graph should not contain at all (issue 4).
   Where they stay in the graph, the logos need one owner inside a package:
   a `branding` module in `window_manager` (which `ui_common` already
   depends on) holding the PNGs and exporting them, used by `ui_common`,
   `ui_about` and `ui_loading_screen` too. `engine/build.rs` keeps its
   `.ico`/`.icns` (the engine binary is never mirrored). **P2.**
3. **`plugins/vendor/voxel/component/src/presets.rs:57-66` embeds
   `assets/examples/voxel_*/scene/default.level` from the repository root.**
   A runtime plugin crate, so it is in every game with the voxel plugin. The
   preset levels belong to the voxel plugin: move them into
   `plugins/vendor/voxel/component/presets/` in the voxel plugin's
   repository (the examples under `assets/examples/` can then read them from
   there, or keep their own copies). Submodule: merge in `Plugin_Voxel` first,
   then re-pin. **P2.**
3a. **`plugins/vendor/meshes/component/src/mesh_files.rs:10-19` embeds the
   engine's primitive meshes** (`assets/meshes/primitives/SM_{Cube,Sphere,
   Cylinder,Plane,Torus}.fbx`) through `include_bytes!(concat!(env!(
   "CARGO_MANIFEST_DIR"), "/../../../../assets/..."))`, a form the first
   source scan missed and the real-engine check found. The primitives are
   the meshes plugin's: they move into `plugins/vendor/meshes/component/`
   in the meshes plugin's repository, and anything else that reads
   `assets/meshes/primitives` is pointed there. Submodule: `Plugin_Meshes`
   first. **P2.**
4. **`crates/core/helio_component/src/subsystems.rs:5`:
   `ENGINE_ASSETS_DIR = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets")`.**
   Not a build failure, a runtime one: compiled in the mirror it points at
   `.pulsar/sdk/assets`, which does not exist, so `resolve_asset_path` never
   finds the engine's built-in assets in a game (and the path is a machine
   path compiled into the game, which `pulsar_package::machine_path_in` is
   meant to keep out of shipped content). Built-in assets a game needs must
   ship as content (`pulsar/` mount) or with the plugin that uses them.
   **P2.**
5. Checked and not issues: `plugins/vendor/meshes/asset-compat/src/rig_converter.rs:881`
   and `crates/ui/wgpui-base/src/text/text_view.rs:1310` include files from
   outside their package, but only inside `#[cfg(test)]` modules, which a
   dependency never compiles. Build scripts in the graph other than
   `engine_backend`'s (`helio_component`, `plugin_editor_api`,
   `pulsar_game`, `wgpui`) read nothing outside their package.

### The game links editor crates

6. **The game's graph contains the editor's UI** (`ui_shell`, `ui_common`,
   `ui_auth`, `ui_types_common`, `window_manager`, `gpui-ce`/WGPUI),
   mirrored and compiled into every game that enables the meshes or
   materials plugin, and the reason issue 2 reaches a game at all. `cargo tree -i` from the runtime plugin roots:
   - `meshes_component` depends on `ui_common` and `gpui-ce` directly, not
     behind the plugin's `editor` feature;
   - `meshes_plugin`, `materials_plugin` and `asset_import` (through
     `meshes_component`) depend on `plugin_editor_api`, which brings
     `window_manager` and `gpui-ce`;
   - `pulsar_game` itself brings only `ui_types_common`, through `engine_fs`.
   The fix is in the meshes and materials plugins (submodules): their editor
   dependencies behind `editor`, as every other plugin's are. `engine_fs`'s
   use of `ui_types_common` is a kernel crate depending on an editor crate,
   which `just arch` should also reject. **P2** (plugins merge first).

### Scripting

7. **Rust-target classes compile but nothing runs them.** The generator emits
   `classes/<module>.rs` for each class whose `compile_target` is `rust`,
   and the game compiles it, but the script driver binds a placed class
   instance to its compiled VM module only: there is no registry of a
   class's native form for it to prefer. Until there is, the generated
   module is compiled under `#[allow(dead_code)]` and the class runs as a
   script. **P3** adds the binding.
8. **Trait declarations for Rust classes live in the Blueprint plugin**
   (`blueprint_editor/.../trait_contracts::generate_module`). The old
   editor-side export appended them to the class's Rust; the generator, in
   core, cannot call into a plugin, so a Rust-target class that declares
   traits is generated without them. **P3** moves the generation into core
   (`pulsar_script_codegen` or `pulsar_traits`).

### Before P2 lands

9. **The SceneDB reflection split.** SceneDB pins an older
   `pulsar_reflection` than the engine; the old builder papered over it with
   a `//`-aliased git patch in generated games (`build.rs:186-195`). The
   generator carries the engine's `[patch]` table as it is and adds no alias,
   so the split must be fixed at its source (bump SceneDB's reflection rev)
   before generated games replace the old ones.
10. **Two names for a project.** The generator reads the name from
    `Pulsar.toml` `[project] name` (falling back to the folder name); the
    editor's project settings have their own `project.name`
    (`ProjectSettings`), which the old builder used. **P2** makes the
    settings page read and write `Pulsar.toml`.
11. **`native/<crate>` extension libraries** (`pulsar_game::scripting::driver::native_libs`)
    are a second user-owned crate convention: separately built cdylibs
    loaded by the script driver. The generator does not know them, so they
    neither join the workspace nor get `{ workspace = true }` engine crates.
    **P2** decides whether they become workspace members like scripts
    crates.
12. **The mirror is written to the real disk** at the project's path, since
    it is per-machine input for the local Cargo. For a remote or
    peer-to-peer project there is no local project path; where its mirror
    and build live is undecided. **P2.**

### Build output

13. **A game build prints the engine's warnings.** Cargo caps lints only for
    registry and git dependencies; mirrored SDK packages are path
    dependencies, so their warnings (179 in the real-engine check, from
    `pulsar_reflection`, `pulsar_std`, `plugin_editor_api`, `wgsl_std` and
    others) appear in every game build. Normalization already strips
    `[lints]`, so no engine `deny` can fail a game build, but rustc's default
    warnings remain and no manifest key caps them per package. The real fix
    is warning-free engine crates (CI's clippy gate covers libraries, not
    rustc warnings in every crate); meanwhile the P2 build runner shows
    diagnostics for the project's own crates and folds the rest.
14. **Generated class code has unused imports.** `pulsar_script_codegen`
    emits one fixed `use ...::vm::{...}` list for every class
    (`src/lib.rs:136`), so a class that uses no collection or unary
    instruction warns (`CollOp`, `Type`, `UnOp` in the real-engine check's
    `door.rs`). **P3** emits the list it uses (or allows `unused_imports` on
    that line), with its golden tests.

### Environment

15. **`cargo tree --target all` downloads other platforms' crates** the first
    time the index is built on a fresh checkout (it needs every target's
    dependencies to answer), so the first sync on a new machine needs the
    network even when the engine is already built. Later syncs reuse the
    cached index. Exported SDKs never run Cargo.
16. **Pre-existing:** `cargo clippy` on `pulsar_install_layout` fails the
    `std::fs` rule (`Path::is_dir`, `std::fs::remove_file`, ...) in code this
    work did not touch; `cargo clippy -p pulsar_project` without `--no-deps`
    stops there. Unrelated to project generation.
