# Quick build and Play-In-Editor availability: plan

**Status: Phase 0 — design. No code changes.** Depends on the project
generation rewrite (`.agents/PROJECT_GENERATION_PLAN.md`): the generated
workspace, `.pulsar/build/`, and `pulsar_project::sync`.

The goal: when only data changed (Blueprints, levels, assets), Play and Build
& Run are near-instant and reuse the native binary already built; when native
code changed, the engine knows it for certain and rebuilds with Cargo. Never a
stale binary running new data it can't run, never a needless rebuild.

---

## 0. Findings

1. **Scripts and assets are already data.** A packaged game reads classes
   (compiled modules), levels and assets from `Content/` next to its
   executable (`pulsar_content::ContentRoot::discover`, pak or loose files);
   nothing of a Blueprint is compiled into the exe (unless its compile target
   is Rust, see 2.2). So "putting new bytecode into the old exe" needs **no
   change to the exe's bytes**: write a new `Content/` beside the cached exe.
   Binary patching would break code signing, debug symbols and AV heuristics,
   and gains nothing.
2. **Today's staleness check is a guess.** PIE's fast path
   (`ui_level_editor/src/ui/panel/pie.rs:392`, `any_source_newer`) compares
   the mtime of every `.rs`/`.toml` under the project to the dylib's. It
   misses inputs that are not `.rs`/`.toml` (`include_str!`/`include_bytes!`
   targets, build-script inputs, `Cargo.lock`, `.cargo/config.toml`,
   `RUSTFLAGS`, the toolchain, the engine itself), trips on content `.toml`
   files that are not build inputs (`input/*.toml`, physics configs), and
   mtimes lie (checkout, copy, clock skew). The dylib path is guessed from the
   crate name (`PieHost::output_dylib_path`), the crate name read by scanning
   lines (`read_crate_name`).
3. **Bytecode links against natives.** A module's `imports` (name +
   signature, `pulsar_script_vm::Import`) must exist in the binary that runs
   it. A binary built with a plugin disabled has none of its natives or
   components. Today nothing checks that a module fits the binary before it
   runs; a mismatch surfaces as a link error at play time.
4. **Packaging always rebuilds.** `pulsar package` cooks content then always
   runs `cargo build --release` (`pulsar_package/src/lib.rs:405`); Build & Run
   uses `cargo run` and reads the project's loose files, so Build & Run never
   exercises cooked content.

---

## 1. Concepts

| | What | Owner |
|---|---|---|
| **Native fingerprint** | A hash of everything that determines the native binary: the Cargo inputs of the generated workspace, the toolchain, build flags, the engine SDK. Equal fingerprint ⇒ an identical build. | `pulsar_project::native` (new) |
| **Build stamp** | A record beside each built artifact: its fingerprint, its own hash, and what it can run (natives, components, versions). Written only after a successful build; read before any reuse. | `pulsar_project::stamp` (new) |
| **Compatibility** | Data (modules, cooked levels) fits a stamp: every import exists with its signature, formats are supported, the component classes it places are registered. | `pulsar_project::stamp` |
| **Quick build** | Stamp valid and fingerprint equal ⇒ reuse the binary, compile scripts, re-cook and repack only changed content. Otherwise the fallback: Cargo rebuild, new stamp. | `ui_build` runner, `pulsar_package` |
| **PIE availability** | The same check against the PIE library's stamp. | `ui_level_editor` PIE panel |

---

## 2. The native fingerprint

### 2.1 Inputs: defined by Cargo, not by file extensions

Scanning the project for `.rs`/`.toml` is both too much (content `.toml`) and
too little (`include_bytes!` targets, `Cargo.lock`). The inputs are defined by
the build itself:

1. **The workspace's package directories.** The generated root `Cargo.toml`
   names the members (`.pulsar/build/game`, `scripts/*`) and the SDK mirror's
   packages; only files inside those directories can be Cargo inputs. Within
   them, the same exclusions as the SDK mirror (`target/`, dot-directories,
   nested packages counted on their own). Content anywhere else in the
   project (levels, assets, `input/*.toml`, `Pulsar/`, `.pulsar/` settings)
   is never native input, whatever its extension.
2. **Exact file set from the last build.** After each successful build, the
   stamp records the files Cargo's dep-info (`target/<profile>/<bin>.d`, and
   each local crate's `deps/*.d`) lists: every source rustc read, including
   `include_str!`/`include_bytes!` targets and `#[path]` modules. Those are
   hashed by content.
3. **New files a build would pick up.** Within the package directories, every
   `*.rs` and every manifest not in the dep-info set (a new module, a new
   scripts crate) is a change. A new file of another type only matters once
   referenced from a `.rs`, which changes that `.rs`.
4. **Build scripts.** A local package with a `build.rs` re-runs per its
   `cargo:rerun-if-changed`/`rerun-if-env-changed` lines; Cargo records them in
   `target/<profile>/build/<pkg>-*/output`. Those paths and env values join
   the fingerprint. A build script without `rerun-if` lines re-runs on any
   change in its package, already covered by 1.
5. **Workspace-level files:** root `Cargo.toml`, `Cargo.lock`,
   `rust-toolchain(.toml)`, `.cargo/config(.toml)` in the project and its
   ancestors (Cargo reads those), `Pulsar.toml` (it decides the plugin set;
   also covered through the generated manifests).
6. **Environment and toolchain:** `rustc -vV` (version, host, LLVM), the target
   triple, the profile, and the build environment Cargo reads (`RUSTFLAGS`,
   `CARGO_ENCODED_RUSTFLAGS`, `RUSTC_WRAPPER`, `CARGO_PROFILE_*`,
   `CARGO_BUILD_*`, `CC`/`CXX`/`AR`, `*_LINKER`).
7. **The engine:** the SDK index fingerprint and the mirror record
   (`.pulsar/sdk/sdk.json` per-file stamps), which already change whenever an
   engine file a game links changes.

Classes whose compile target is Rust are generated into `.pulsar/build/game`
(project generation plan, 2.6), so they are native input by 1: switching a
class to Rust, or editing one that is, needs a rebuild. Script-target classes
never touch the fingerprint.

### 2.2 Computing it cheaply

A per-file cache in `.pulsar/build/native-files.json`: path → (size, mtime ns,
content hash). A file whose size and mtime match its entry reuses the hash;
any other is read and hashed (BLAKE3). The fingerprint is a hash over the
sorted `(path, content hash)` list plus the non-file inputs. mtimes only decide
*whether to re-hash*, never *whether something changed*, so a touched but
unchanged file costs a hash, not a rebuild, and a changed file with an old
mtime is still caught when its size differs; with equal size and mtime (rare)
a periodic full re-hash and `force` cover it.

### 2.3 Not hashed, on purpose

- `target/` and `.pulsar/build/stage/` (outputs).
- `.pulsar/` settings, layout, keybindings, caches; `Pulsar/` runtime settings
  (data, read at run time).
- Anything outside the package directories (content).

---

## 3. The build stamp

Written next to each artifact after a successful build, as
`<artifact>.pulsar-build.json` (`target/<triple>/<profile>/star_fall.exe.pulsar-build.json`,
and the same for the PIE library):

```json
{
  "format": 1,
  "artifact": { "file": "star_fall.exe", "sha256": "…", "size": 123 },
  "native_fingerprint": "…",
  "profile": "release", "target": "x86_64-pc-windows-msvc",
  "engine": { "sdk_fingerprint": "…", "version": "…" },
  "abi": { "pie": 5, "script_module_format": [1, 5] },
  "plugins": ["pulsar.lights", "pulsar.meshes", "…"],
  "natives": { "Transform::set_rotation_degrees": "<signature hash>", "…": "…" },
  "components": ["StaticMeshComponent", "…"],
  "events": { "…": "<signature hash>" }
}
```

**The binary describes itself.** The natives, components, events and
versions are not predicted from source: after the build, the runner starts
the built game with `--pulsar-describe` (a `pulsar_game::standalone` flag: no
window, no content, prints the description and exits; the PIE library gets
the same as a `pulsar_pie_describe` export). What the stamp says the binary
can run is what that binary reports.

**A stamp is trusted only when** the artifact's hash matches (a binary
replaced by hand, or by a build the editor did not run, is not reused),
`format` is known, and `native_fingerprint` equals the current one.

---

## 4. Quick build (Build & Run, Package)

```
sync project ─▶ native fingerprint ─▶ stamp valid & equal? ──no──▶ cargo build ─▶ describe ─▶ write stamp ─┐
                                          │yes                                                              │
                                          ▼                                                                 ▼
                              compile scripts ─▶ check compatibility ─no──▶ (fallback: cargo build) ──────────┤
                                          │yes                                                              │
                                          ▼                                                                 │
                              cook changed content ─▶ repack ─▶ stage ─▶ run / copy out ◀───────────────────┘
```

- **Compile scripts** headlessly (what `pulsar package` does in `compile`).
  Errors stop the build, as today.
- **Compatibility** (`stamp::check`): every module's imports exist in the
  stamp with equal signature hashes; module format within the stamp's range;
  every component class placed by a cooked level or prefab is registered;
  every engine event a module subscribes to exists. A failure names the
  module, the missing native and the plugin that provides it, then falls back
  to a rebuild (a plugin enabled since the build) rather than shipping data
  that can't run.
- **Incremental cook.** A cook cache in `.pulsar/build/cook/`: per source
  asset, keyed by (source hash, cooker version, profile, target, the
  engine's cook-format version), the cooked bytes. Only assets whose key
  changed are cooked again; class modules are re-encoded only when their
  `module.json` changed.
- **Repack.** The pak is rebuilt from the cooked cache (reads, no cooking) and
  written to a temporary file, then renamed over `Content/game.pak`. A running
  copy of the game holds the old pak open on Windows: Build & Run stops the
  running game first (it already owns its process).
- **Stage.** `.pulsar/build/stage/<profile>-<target>/` holds the binary
  (hard-linked from `target/`, copied when linking fails) and `Content/`. Build
  & Run starts the staged binary, so it runs cooked content exactly as a
  shipped game does (finding 4). `pulsar package` is staging plus a copy to
  `--out`; `--skip-build` becomes "require a valid stamp".
- **Fallback** is the existing Cargo build, after which the describe step
  writes the stamp, then the same cook/repack/stage. The build dashboard shows
  which path ran and why ("Native code changed: scripts/gameplay/src/lib.rs",
  "Plugin pulsar.voxel enabled since the last build").

---

## 5. Play-In-Editor

PIE loads the game library into the editor and reads the project's loose
content; scripts already reload in place (#833). Availability follows the
library's stamp:

| State | When | Play |
|---|---|---|
| **Ready** | Stamp valid, fingerprint equal, scripts compatible | Starts at once; a running session reloads changed classes in place |
| **Needs build** | Fingerprint differs, no stamp, or scripts incompatible | Builds the library (dashboard), then plays — or is disabled, see Q1 |
| **Blocked** | The last build of this fingerprint failed | Disabled, with the failure; re-enabled when the fingerprint changes |

The toolbar shows the state and the reason, updated by the project watcher
(the fingerprint is recomputed on change, debounced), so the user knows before
pressing Play whether it will be instant.

Native hot reload of a running PIE session (swapping the library while the
world keeps running, `reload` in `pie.rs`) stays as it is, but only when the
fingerprint changed and the new library's stamp is compatible with the live
session's modules.

---

## 6. What gets deleted

- `pie.rs` `any_source_newer`, `read_crate_name`, `same_artifact`'s mtime
  comparison; `PieHost::output_dylib_path` (artifact paths come from Cargo's
  `compiler-artifact` messages, recorded in the stamp).
- `pulsar_package`'s unconditional build step (replaced by quick build with
  the fallback).
- Build & Run's `cargo run` (replaced by build-if-needed + run the staged
  binary).

---

## 7. Decisions (need the user)

- **Q1. Play when native code changed:** build then play automatically (one
  click, the dashboard opens), or disable Play until the user builds?
  *Proposed:* build then play, with the reason on the button ("Rust changed
  — Play will rebuild"), since a disabled button with a reason is one more
  step for the same outcome.
- **Q2. Staging location:** `.pulsar/build/stage/` (proposed: generator-owned,
  gitignored, next to the game crate) or under `target/`.

---

## 8. Phases

- **Q1-P1 — `pulsar_project::native`:** input discovery (package dirs,
  dep-info, build-script outputs, workspace files, env, SDK), the hash cache,
  `NativeFingerprint` with a diff ("what changed") for the UI. Tests: a content
  `.toml` change does not change it; `.rs`, `include_bytes!` target, new
  module file, `Cargo.lock`, `RUSTFLAGS`, toolchain and SDK changes do; a
  touched-but-same file costs a hash and no change.
- **Q1-P2 — describe + stamp:** `--pulsar-describe` and `pulsar_pie_describe`,
  the stamp writer and reader, `stamp::check`. Tests: a stamp of a replaced
  artifact is refused; a module importing a native the binary lacks is
  refused with the plugin named.
- **Q1-P3 — incremental cook + staging:** the cook cache, pak rewrite, the
  stage directory; `pulsar package` on top of it.
- **Q1-P4 — runner and PIE:** Build & Run and Package through quick build with
  the fallback; PIE states on the toolbar; the dashboard showing the path and
  reason. Delete section 6's code.
- **Q1-P5 — verification:** an end-to-end test: build once, edit a Blueprint
  ⇒ quick path (no Cargo process started), edit a `.rs` ⇒ Cargo, enable a
  plugin ⇒ Cargo; timings recorded in the "As built" notes.

## 9. Issues log

(empty)
