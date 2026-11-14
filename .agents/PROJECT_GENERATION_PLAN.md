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
| `Cargo.toml` (root) | generator | **no** | virtual workspace: members, `[workspace.dependencies]`, `[patch]` — the only place an engine path appears |
| `Cargo.lock` | generator seeds, cargo updates | no | seeded from the engine's own lockfile |
| `.pulsar/build/game/**` | generator | no | the game crate: `Cargo.toml`, `src/{main,lib,plugins,scripts}.rs`, nativized classes |
| `.pulsar/build/generated.json` | generator | no | the record (2.4) |
| `.gitignore` entries | generator adds once | yes | `/target`, `/Cargo.toml`, `/Cargo.lock`, `/.pulsar/build/` |

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
  sdk.rs          EngineSdk: where this engine's crates are on this machine (2.3)
  model.rs        ProjectModel::resolve(root, &EngineSdk) -> ProjectModel   (reads inputs only)
  render/         ProjectModel -> FileSet   (pure; one module per file; toml via typed structs)
  apply.rs        FileSet + record -> writes, deletes, drift report (2.4)
  sync.rs         sync(root, Trigger) -> SyncReport   (the one public entry point)
  migrate.rs      format 1 -> 2 (section 5, P4)
  create.rs       create(root, Template) for new projects
```

`ProjectModel` is plain data: name (from `Pulsar.toml`, the one identity),
crate idents, engine SDK root and fingerprint, enabled runtime plugin roots,
scripts crates (by parsing their manifests with `toml`), nativized classes
(2.6), build profile. `render` cannot touch the filesystem, so it is
golden-testable and deterministic: no clocks, sorted iteration, stable
formatting.

### 2.3 The engine SDK

The engine is referenced by identity and located at sync time, never baked as
an absolute path at engine compile time:

- **Engine build** carries a relative description of what games link: the
  game dependency list, the runtime plugin roots, the workspace `[patch]`
  table, all with **workspace-relative** paths, plus the engine's
  `Cargo.lock`, embedded by `pulsar_project/build.rs`. And an engine
  fingerprint (version + git rev of the build).
- **`EngineSdk::locate()`** resolves the root those relative paths are under:
  the source checkout for a dev build (found from the running executable's
  location, the way `pulsar_install_layout` finds plugins; not
  `CARGO_MANIFEST_DIR`), or the install's `sdk/` directory for a Hub install
  (decision Q1). Fails with a clear error, never a half-written project.
- Paths become absolute only in the root `Cargo.toml`, which is per-machine and
  regenerated (2.1). Scripts crates say `pulsar_game = { workspace = true }`.

The reflection-alias patch (`build.rs:186-195`) and the `libc` /
`find-msvc-tools` pins (finding 8) are not carried over: the seeded lockfile
pins every version the engine uses, and the SceneDB reflection split is fixed
at its source (bump SceneDB's `pulsar_reflection` rev) before P2 lands.

### 2.4 Apply, and the record

`.pulsar/build/generated.json`:

```json
{ "format": 2, "generator": 1,
  "inputs": "<hash of every input below>",
  "files": { "Cargo.toml": "<sha256>", ".pulsar/build/game/src/main.rs": "<sha256>" } }
```

`apply`:

1. Write each file whose content differs from disk, atomically
   (`write_file_atomically`, already used at `:722`).
2. Delete each file in the old record that the new `FileSet` no longer has
   (finding 6).
3. If a file on disk differs from the hash the record says was written, it was
   hand-edited: overwrite it and report it as drift ("put this in
   `Pulsar.toml`"), shown in the editor's Problems panel.
4. Write the record last. An interrupted sync leaves the old record, so the
   next sync redoes the work.

### 2.5 Triggers

`sync` hashes its inputs (engine fingerprint, SDK root, `Pulsar.toml`, the
scripts crate manifests, the nativized class modules, generator version) and
returns at once when the hash matches the record and no recorded file is
missing. So it is called everywhere, cheaply:

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

- `main.rs` → `pulsar_game::standalone::run(setup)`; `lib.rs` → the PIE
  `pulsar_pie_*` shim (unchanged ABI); `plugins.rs` → `use <root> as _;` per
  enabled runtime plugin.
- `scripts.rs` → `setup()` calls each scripts crate's `register_scripts`, then
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
changed" behaviour, keyed on the record's input hash plus the scripts crates'
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

- **Q1. Where a Hub-installed engine's crates come from. Decided:** each install ships `sdk/`, a source snapshot of the workspace
  crates a game links plus their `[patch]` sources and the `Cargo.lock`, built
  by the release pipeline; `EngineSdk::locate()` prefers it. Alternative: git
  revs only — needs network at first build, every submodule pushed to its
  upstream at the pinned rev, and the reflection alias hack stays.
- **Q2. Blueprint → Rust export. Decided:** the Blueprint editor's existing
  Rust/VM picker (`CompileMode`, `blueprint_editor/src/core/types.rs:152`)
  stays the control. Today it is panel state reset on open
  (`editor/panel.rs:539`); it becomes a per-class setting saved in the class's
  `graph_save.json` (`"compile_target": "vm" | "rust"`, absent = `vm`). Compiling
  always writes `module.json`; the editor writes no Rust. The generator reads
  each class's `compile_target` and emits `.pulsar/build/game/src/classes/<ident>.rs`
  for the `rust` ones, so changing the picker and compiling triggers a sync.
- **Q3. Root `Cargo.toml` committed or ignored. Decided:** ignored,
  regenerated per machine (it holds the only engine paths).
  `pulsar project sync` restores it for command-line users.

---

## 5. Phases

Per the standing rule, every phase is built; issues that do not block are
logged in section 6 and fixed in a cleanup pass.

- **P1 — `pulsar_project` crate.** descriptor, sdk, model, render, apply,
  sync, the record. Tests: golden output per file; idempotency (second sync
  writes nothing, reads only inputs); deletion of files no longer produced;
  drift detection; interrupted sync; no absolute path in any committed file.
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

(empty — filled during P1-P5)
