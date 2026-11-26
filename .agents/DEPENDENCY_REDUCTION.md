# Dependency reduction: analysis, measured savings and plan

Read-only analysis of the monorepo as of commit `c3b388c81` (Windows host, `x86_64-pc-windows-msvc`, Cargo 1.99.1,
the lock of that commit; a merge into `main` was running while this was written, so later commits may differ in
small ways). Nothing tracked was changed. Every "measured" number below comes from one of four mechanisms, and each
item says which one:

| tag | how it was measured |
|---|---|
| **lock** | package count of the `Cargo.lock` that `cargo metadata --offline` writes in a scratch copy of the workspace (manifests and sources only, `C:\tb_scratch\mexp`) after the edit. This counts every platform and every optional dependency of a workspace member. |
| **tree** | `cargo tree -p <root> -e normal,build --target x86_64-pc-windows-msvc` unique lines in the same scratch copy: what a build of that root really compiles on Windows with its resolved features. |
| **built** | the set of distinct packages `cargo check --workspace --all-targets` really compiled on Windows (1431) and how many of the removed packages are in it. |
| **lint** | the compiler's own `unused_crate_dependencies` lint (`RUSTFLAGS="-W unused-crate-dependencies"`, JSON diagnostics, every target of every workspace package), cross-checked with a text search of each package's sources. |

`cargo-machete`, `cargo-udeps` and `cargo-deny` are not installed on this machine; the lint is stronger than
machete's text search, and `deny` is only proposed (section 7). The scratch copies live under `C:\tb_scratch`
(`full` = a complete copy used for real compiles, `mbase`/`mexp` = manifest-only copies used for lock and tree
counts) and the compile cache under `C:\tb_deps`; delete them freely.

## 0. Summary

* Baseline: **1763 packages** in `Cargo.lock` (1522 unique names; 325 path, 41 git, 1397 registry). The editor
  (`pulsar_engine`) compiles **972** packages on Windows, the game (`pulsar_game`) 508, `plugin_builder` 136, the six
  shared dylibs 39 to 710 each. `cargo check --workspace --all-targets` compiles 1431.
* **Delete outright, no feature lost** (all measured, section 7): move `pulsar-relay`, the 14 `*-examples` crates and
  the two web/android demos out of the root workspace (lock -139), delete the dead PebbleVault `World`
  (lock -31, compile-checked), move SceneDB's bevy/criterion benches out (lock -31), drop `rust_decimal` from `ui`
  (lock -15), and delete 285 declared-but-unused dependency lines (lock -16, compile-checked on the whole
  workspace). Measured together (the cuts interact, they are not additive): **lock 1763 -> 1503 (-260, -15%)** including +2 for the regenerated
  workspace hack, editor closure 972 -> about 900, game closure 508 -> 386.
* **Plus edits that touch no engine feature but need forks/unification** (gpui http client, ureq 2, toml, dirs,
  html5ever, device_query, pollster): lock **1473 (-290, -16%)**, `ui_dylib` closure 635 -> 566,
  `wgpui_dylib` 503 -> 414. With the not-measured value-bag/git2/clap/native-tls items (section 7, wave 3b) the
  honest floor without removing anything an engine user can do is **about 1430 to 1470 (-17% to -19%)**.
* Behaviour trades (not free, listed in section 8: LaTeX in markdown, the agent's built-in web/HTML tools, build-sound
  playback) would take it to about 1384 (-21%).
* A "huge" reduction (-40% or more) is **not** reachable without dropping engine capability. What is left after the
  cuts is the real engine: wgpu/naga and the windows-rs generations, gpui/cosmic-text/taffy, 83 proc-macro crates, 28
  tree-sitter grammars, oxc (TypeScript), rapier/parry, image codecs. Section 8 prices the ones that are capabilities.
* The workspace hack costs **+2 lock packages** (itself and one crate only it pins) and **+14 packages in an editor-only
  build** (exr, pulp, pulp-wasm-simd-flag, lebe, zune-inflate, zlib-rs, bit_field, approx, num-complex, owo-colors,
  raw-cpuid, reborrow, syn 1, zeroize_derive: the union of features the sky/HDR plugins ask of `image`). That is what its design buys.

### Top 15 outright removals (lock package counts are measured in the scratch copy; "built" = compiled on Windows)

| # | removal | lock | built | where | feature lost |
|--:|---|--:|--:|---|---|
| 1 | move `crates/core/pulsar-relay` out of the root workspace (own workspace/lock) | -83 | -72 | root `Cargo.toml` `exclude`, `release.yml`, `deploy/rancher` | none (a server; the editor never links it) |
| 2 | move the 14 `plugins/vendor/*/examples` crates, `meshes/web-demos`, `meshes/android-demos` out | -55 | -52 | root `Cargo.toml` members; each plugin repo | none (demo binaries) |
|   | (1 and 2 together, overlap removed) | **-139** | **-125** | | |
| 3 | `PebbleVault` in `engine_backend`: `World` is a dead re-export (no user in the tree) | -31 | -27 | `crates/core/engine_backend/Cargo.toml:35`, `src/subsystems/world`, `src/lib.rs:36` | none (dead code) |
| 4 | SceneDB bench dev-deps `bevy_ecs`, `criterion` (plotters, ciborium, ...) | -31 | -24 | `crates/external/scenedb/crates/pulsar_scenedb/Cargo.toml` (`benches/vs_bevy_ecs.rs`) | none (benches) |
| 5 | `rust_decimal` in `ui` (only `impl Sealed for rust_decimal::Decimal`, `src/plot/scale/sealed.rs:6`) | -15 | -1 | `crates/ui/wgpui-component/crates/ui/Cargo.toml` | Decimal as a plot scale (unused here) |
| 6 | 285 declared-but-unused dependency lines (Appendix A) | -16 (almost all relay dev tooling that cut 1 removes anyway; independent effect about -2) | n/m | 123 manifests in 34 repos | none (rustc proves they are not linked) |
| 7 | `pulsar_package`: `clap` only for the `pulsar` CLI bin, but the editor links the lib | -4 | editor closure **-13** | `crates/core/pulsar_package/Cargo.toml:51` | none (gate behind a `cli` feature / `required-features`) |
| 8 | `gpui-ce` -> `gpui_http_client` (zed-reqwest, zed-async-tar, async-std, ...) | -17 | -12 | `crates/ui/wgpui/Cargo.toml` `http_client =` | URL image loading through gpui's HTTP client (nothing in Pulsar uses it) |
| 9 | `ui_level_editor` `ureq 2.12` -> `ureq 3` (already in the tree) | 0 alone, part of wave 3 | | `crates/editor/ui_level_editor/Cargo.toml:70` | none |
| 10 | `gpui-ce` `device_query 2.1` -> `4` (matches `ui_level_editor`) | -1 | -1 | `crates/ui/wgpui/Cargo.toml:124` | none |
| 11 | `wgpui-base` html5ever 0.27 / markup5ever_rcdom 0.3 -> 0.39 (what `ui` uses) | -2 | -2 | `crates/ui/wgpui-base/Cargo.toml:36-37` | none |
| 12 | `pulsar-config` `dirs 5` -> `6` (and `directories 5` in `ui`, `notify 6` in `ui`) | -1 (+about -4 est.) | -1 | `crates/third-party/pulsar-config/Cargo.toml:13` | none |
| 13 | `log` `kv_unstable_serde` forced by `gpui-ce` and three crates.io `gpui_*` crates (value-bag, sval, serde1: 14 packages) | est. -14 | | `crates/ui/wgpui/Cargo.toml:65` + vendor/patch `gpui_util`, `gpui_sum_tree`, `gpui_http_client` | none (nothing reads log key-values) |
| 14 | `gpui_util` -> `git2 0.20.4` (a second libgit2 binding; `ui_git_manager` uses 0.21) | est. -1..-3 | | patch `gpui_util` or align `ui_git_manager` to 0.20 | none |
| 15 | `ort` `tls-native` -> `tls-rustls` in `mocap_capture` (native-tls, schannel via ureq/ort-sys) | est. -2 | | `plugins/vendor/mocap/capture/Cargo.toml` | none (build-time download of onnxruntime) |

Items 13 to 15 are **estimated** (they need a patch/vendor step that cannot be tried offline); everything else was
run.

## 1. Baseline

### 1.1 The lock

| | count |
|---|--:|
| `[[package]]` entries in `Cargo.lock` | **1763** |
| unique package names | 1522 |
| path packages (workspace members 283 + non-member path crates 42: Helio, the wgpui crates, nebula, graphy...) | 325 |
| git packages (14 repositories, see below) | 41 |
| crates.io packages | 1397 |
| names that appear with more than one version | 176 (241 extra packages) |
| `cargo metadata` (all platforms) resolved packages | 1756 (7 lock entries are not in the resolver output; not investigated) |
| `cargo metadata --filter-platform windows` packages | 1488 (46 of them only reachable through dev-dependencies) |
| distinct packages `cargo check --workspace --all-targets` compiled on Windows | 1431 |
| editor-reachable on Windows by the metadata graph (normal + build edges) | 1261 (over-counts optional deps, see 1.2) |

Git repositories in the lock: `Gamma` (3 crates), `priority-threadpool` (2), `Mockaco` (5 crates **at two revisions**
`e36ce16...` in `ui_git_manager`/`ui_multiplayer` and `e3bb374...` in `plugins/vendor/code_editor`: 5 duplicate
packages), `wgpu` fork (`helio/v30.0.1`: wgpu, wgpu-core, wgpu-hal, wgpu-types, naga, ... one revision), `Solid3D`
(6 crates, **two revisions**: `a87fdf0...` in Helio/meshes asset-compat, `b9eb501...` in `asset_viewer`: `solid-rs` and
`solid-fbx` twice), `Pulsar-Dialog`, `PIF-rs`, `Quark`, `Discord-RPC-RS`, `ToolbeltRS`, `UASSET`, `tree-sitter-md`.

### 1.2 Closures per build root (Windows, `cargo tree -e normal,build`)

| root | packages | unique names |
|---|--:|--:|
| `pulsar_engine` (the editor, workspace hack included) | **972** | 869 |
| `pulsar_game` | 508 | 472 |
| `plugin_builder` | 136 | 133 |
| `hack_gen` | 124 | 121 |
| `wgpui_dylib` | 503 | 458 |
| `ui_dylib` | 635 | 571 |
| `profiling_dylib` | 39 | 39 |
| `reflection_dylib` | 641 | 577 |
| `scenedb_dylib` | 644 | 580 |
| `helio_dylib` | 710 | 638 |

The `cargo metadata` resolve graph lists optional dependencies that the enabled features never activate (glam 0.30,
0.31 and 0.32 under nalgebra 0.35 are in the lock and in `metadata`, and are **not** compiled), so the metadata
numbers in the appendices over-count by a few percent; the `tree` and `built` numbers above and in section 7 are
the compiled truth. Everything under `reflection_dylib`, `scenedb_dylib` and `helio_dylib` repeats the same ~570
packages: the six libraries share one closure by design (workspace hack, `BUILD_LESSONS.md` section 2).

Packages in the lock that the editor never builds: **181** on Windows by the metadata graph (relay 79 unique, the
examples about 100, fixtures, tools). Moving the 17 crates of cuts 1 and 2 frees 160 on the unmodified lock and 139
after wave 1 (wave 1 had already taken relay's unused dev-dependencies).

### 1.3 What each direct dependency costs

Appendix D lists the 45 most expensive direct dependencies (packages that disappear when that one dependency is
removed from the whole workspace, Windows). Top of it: `PebbleVault` 34, `mathjax-svg-rs` 27, `jsonwebtoken` 26 (relay),
`boa_engine` 24 (via mathjax), `config` 18 (relay), `wgpu` 17, `tool_registry_builtin` 17, `rust_decimal` 15, `sqlx` 14
(relay), `ratatui` 13 (web-demos), `rapier3d` 12, `gpui_http_client` 11, `rodio` 10, `ureq` 10, `opentelemetry-otlp` 10
(relay). Edge-level costs (a dependency edge, not a crate) are what finds the cheap cuts: for example
`pulsar_package -> clap` frees 13 although `clap` has other users on other platforms.

### 1.4 Native C compiles (compile-time heavy, 45 build scripts that run `cc`/`cmake`/`bindgen` on Windows)

28 tree-sitter grammars (`ui` feature `tree-sitter-languages`, requested by `engine`, `ui_git_manager`, `code_editor`
and the `ui_dylib` manifest), `libgit2-sys` (two `git2` majors share it), `libsqlite3-sys`, `zstd-sys`, `libz-sys`,
`ring`, `mach-dxcompiler-rs`, `meshopt`, `intel_tex_2`, `openxr-sys`, `psm` (via stacksafe), `alloca`, and, until cut 3,
`PebbleVault`/`mysql_common`/`bindgen`. 83 proc-macro crates and 157 build scripts in the 1431.

Compile time and binary size were **not measured** (no `cargo build --timings`: a cold release build of 1260 crates
was out of scope). Package count and C-compile count are the proxy.

## 2. Unused declared dependencies

### Method

1. Every package with a path source (322 manifests including the 34 plugin/UI/Helio/SceneDB submodules) was checked
   by `cargo check --workspace --all-targets --keep-going` with `-W unused-crate-dependencies`, JSON output. A
   dependency counts as unused when rustc warns about it in **every** compilation unit that is handed it (lib, lib
   test, bins, tests, examples, benches; for a dev-dependency only the test/example/bench units; for a
   build-dependency the build script).
2. Each result was joined with a text search of the package's own `.rs` files (renames and `package =` aliases
   resolved through the real lib name; `dep:`/`dep/feature` forwarding in `[features]` detected).
3. **Safe** = lint says unused **and** the name appears nowhere in the package's sources **and** no feature forwards
   it. 285 entries. They were **removed in a full scratch copy and the whole workspace re-checked**
   (`--workspace --all-targets`, Windows, default features): the only failures are the same three compile errors the
   unmodified tree already has (`pulsar_scenedb` lib test and `subsystem_reflection_test`, `pulsar_engine` test
   `linked_editors`), i.e. no new error and no new unused-crate warning. `Cargo.lock` went 1763 -> 1747.
4. The remaining lint hits (157) need a human: the name does appear in the sources but is not linked on Windows
   (platform-gated code, `dll`/feature-gated code, doctests) or the entry is a deliberate link-only root.

### Results

| class | count | confidence |
|---|--:|---|
| lint-unused, all rows | 442 | |
| **safe to delete now** (Appendix A, grouped by repository and manifest) | **285** (251 normal, 33 dev, 1 build; about 125 are edges to workspace crates, about 160 to registry crates) | high: compile-checked |
| deliberate, keep: `sdk_roots` (7 shared dylibs, link-only roots), `pulsar_workspace_hack`, plugin test fixtures | 18 | n/a |
| needs a look (Appendix B) | 139 | cfg/feature/doctest only, or app-specific |

Where the 285 live: monorepo 71 (`crates/*`, `plugins/installed`, `tools`), then `plugins/vendor/meshes` 42,
`plugins/vendor/bake` 28 (nebula), Helio 19, `code_editor` 10, `blueprint_editor` 8, `foliage`/`sky`/`portals` 8 each,
`asset_viewer`, `fog`, `gameplay`, `sprites` and `psgc` 6 each, and 20 more repositories with 1 to 5 each.
Typical shapes: `pulsar_reflection`/`pulsar_scenedb_derive`/`pulsar_world_registry`/`serde_json`/`tracing`/`glam`/`log`
listed in component crates that no longer use them after the SceneDB/Reflection migration (portals, fog, particles,
post, sky, water, decals, the helio `helio-pass-*` crates), `parking_lot` in 12 crates, `reqwest` in `ui_common` and
`ui_multiplayer`, `futures`/`uuid` in the editors, `rust-i18n-macro`/`rust-i18n-support` next to `rust-i18n`
(`script_editor_plugin`, `asset_viewer_plugin`, `ui_common`, `plugin_matter`), and `criterion`/`mockall`/`proptest` in
`pulsar-relay` (dev).

Package effect is small on purpose: these are mostly edges to crates that something else still needs. Lock -16, and
that is essentially `pulsar-relay`'s dev tooling (mockall, predicates, proptest, rusty-fork, wait-timeout, unarray, fragile,
downcast, termtree, quick-error, rand_xorshift, bit-set 0.8), which wave 2 removes anyway: the independent package effect of
wave 1 is about 2. The value is build graph hygiene, faster `cargo
check` and honest manifests, plus it is the precondition for the hack generator to stay small.

Separate finding: **a normal dependency used only by tests/examples** (grep: no hit in `src/`, hits in `tests/` or
`examples/`) should be a dev-dependency. The text search found them in `helio-presets` (`wgpu` 74 lines,
`helio-pass-tsr`, `glam`), `helio-pass-fxaa` (`bytemuck`), `nebula-gpu` (`nebula-core` dev-only use), `scenedb-inspector-agent`
(`pulsar_reflection`). Moving them out of `[dependencies]` removes no package; it narrows downstream closures.

Dev- and build-dependencies were analysed in the same pass (33 safe dev rows, 1 safe build row `serde_json` in
`pulsar_script_conformance`; 26 dev rows and 2 build rows in the look list: mostly doctest use in Helio passes and
plugin roots: `helio-feature-api` dev in `decals_plugin`, `materials_plugin`, `voxel_plugin`, `sprites_plugin`,
`meshes_plugin`).

## 3. Duplicate versions

Lock: 176 names, 241 extra packages. Built on Windows: **124 names, 162 extra packages**. Appendix C lists every one
with the parents of each version and a chain to the workspace crate that introduces it. Controllable means "a
manifest we own can change the answer".

| class | versions (built) | who pulls the minority versions | route | packages saved |
|---|---|---|---|--:|
| windows family: `windows` 0.34/0.48/0.54/0.59/0.62.2; `windows-sys` 0.48/0.52/0.59/0.60.2/0.61.2; `windows-core` x3, `windows-result` x3, `windows-targets` x3, `windows_x86_64_msvc` x4, `windows-link`, `-implement`, `-strings`, `-registry` x2 | 5+5+3+3+3+4+2+2+2+2 = 31 extra | 0.34: `shared_memory` (`scenedb-inspector-agent`, examples only); 0.48: `device_query` x2; 0.54: `cpal` <- `rodio` <- `ui_build`; 0.59: `clipboard-rs` <- `gpui-ce`; windows-sys 0.48: `notify 6` (ui), `dirs-sys 0.4`, `winreg 0.52` (`sysuri`); 0.52: `socket2 0.5` (SceneDB), `winit`; 0.59: `colored 2`, `console`, `rustix 0.38` (PebbleVault); 0.60: `arboard`, `notify 8` | cuts 1-3, 10, 12 take out 0.34, 0.48 (one of two), 0.59 `colored`; `rodio` or `clipboard-rs` (gpui has both `arboard` and `clipboard-rs` in `src/platform/cross/platform.rs`) removes 0.54 / 0.59 | est. -12 with cuts 1-3,10; est. -8 more with the two optional ones |
| `glam` 0.27 / 0.33 (+0.30, 0.31, 0.32 in the lock only) | 1 extra built | `solid-*` (Solid3D git) at glam 0.27 | bump Solid3D | -1 |
| `hashbrown` 0.12.3 (lock) 0.14.5, 0.15.5, 0.16.1, 0.17.1 | 3 extra built | `dashmap`/`hashlink` (0.14), `fontdue`/`lru`/`parry3d` (0.15), `boa_*`/`gpu-allocator` (0.16), `wgpu`/`naga`/`oxc`/`indexmap` (0.17) | boa leaves with mathjax (0.16 stays via gpu-allocator, regress); not ours | 0..-1 |
| `phf` x4 (+generator/shared/codegen/macros: 4x4 extra) | 12 extra | 0.10 `selectors` <- `scraper` <- `tool_registry_builtin`; 0.11 `markup5ever 0.12` <- `html5ever 0.27` <- `wgpui-base`; 0.13 `tokio-postgres` (PebbleVault) `boa_interner` `web_atoms`; 0.14 `oxc` | cut 3 removes 0.13 parent (but `web_atoms` keeps it unless ui drops mathjax/html5ever 0.39 uses 0.13), cut 11 removes 0.11, toolbelt removes 0.10 | est. -4 (cut 3) to -10 (3 + 11 + toolbelt) |
| `syn` 1 / 2 / 3 | 2 extra | 1: `uasset`'s `binread_derive`, `rkyv_derive`, `bytecheck_derive`, `ptr_meta_derive`; 3: `async-trait`, `bytemuck_derive`, `clap_derive`, `darling_core`... | 1 disappears with cut 5 (rkyv) and `solid-unreal`/`uasset`; 3 vs 2 is the ecosystem's own split | -1 |
| `itertools` x5, `rand` x3 (+`rand_core` x3, `getrandom` x3, `rand_chacha` x2), `bit-vec` x3 | many | `rust-i18n-support` (itertools 0.11), `salva3d`/examples (0.12, rapier 0.18/0.24, bit-vec 0.6/0.8), `ratatui` (0.13), `egui` (0.15), PebbleVault/`jsonwebtoken`/`rust_decimal` (rand 0.8), `boa_engine`/`gpui_util`/`opentelemetry_sdk` (rand 0.9) | cuts 1-3, 5 remove itertools 0.12/0.13/0.15, rand 0.8 (partly), bit-vec 0.6/0.8, rapier3d 0.18/0.24, nalgebra 0.33, parry3d 0.13/0.19, simba 0.8.1 | est. -18 together |
| `nalgebra` 0.32.6 / 0.35, `simba` 0.8.1 / 0.10, `wide`, `safe_arch`, `nalgebra-macros` | 4 extra | `mocap_capture` + `mocap_editor` pin 0.32 | bump both mocap crates to 0.35 (3 manifests: `capture` lines 24 and 30, `editor` line 30) | est. -4 |
| `thiserror` 1.0.69 (+`-impl`) | 2 extra | 16 parents: `graphy`, 7 `nebula-*` (`plugins/vendor/bake/vendor/nebula`), `psgc`, `async_zip` | bump `graphy`, `psgc`, `nebula` to 2 (local), `async_zip` external | -2 only if all go |
| `dirs` 4/5/6, `dirs-sys` 3, `directories` 5/6, `winreg` 2 | 6 extra | `gpui_util` (dirs 4), `pulsar-config` (dirs 5), `ui`'s `directories 5` | bump `pulsar-config`, `ui`; `dirs 4` stays with gpui_util | -4 |
| `toml` 0.8/1.x (+`toml_edit`, `toml_datetime`, `serde_spanned`, `winnow` 2 each) | 5 extra | `pulsar-config`, `ui` (0.8); `rust-i18n-support` (0.8, external) | bump the two: the pair stays alive through `rust-i18n-support` unless it moves too | 0 (-5 only with a patched rust-i18n-support) |
| `pollster` 0.2.5/0.3/0.4/1.0.1 | 3 extra | `postage` (gpui-ce), `nebula-core`, `pulsar_game`/SceneDB/`pulsar_world_registry`/`engine_class_derive` at 1.0.1 vs Helio/`helio_component`/`engine_backend` at `=0.4.0` | pick 1.0 everywhere or 0.4 everywhere (Helio uses 0.4) | -2 (0.3 and one of 0.4/1.0) |
| `notify` 6 / 8, `device_query` 2.1 / 4, `ureq` 2 / 3, `reqwest` 0.12 / 0.13, `tower-http` 2, `zip` 2.4 / 9.5, `similar` 2 / 3, `ropey` 1 / 2, `bincode` 1 / 2, `base64` 0.22 / 0.23, `libloading` 0.8 / 0.9, `flume` 0.11 / 0.12, `spin` 2, `rustc-hash` 1 / 2, `smol_str` 2, `strum` 2, `compact_str` 2, `html5ever`/`markup5ever`/`xml5ever`/`rcdom`/`string_cache`/`tendril` 6 pairs | one extra each (about 22) | `ui` (notify 6, ropey?), `ui_level_editor` (ureq 2, device_query 4), ToolbeltRS (reqwest 0.12, scraper), `pulsar_image_format` (zip 2.4), `script_editor_plugin` (similar 2.7), `mockaco-core` (ropey 1.6, similar 3), `helio-bake` (bincode 1.3.3), `nokhwa` (flume 0.11), `wgpui-base` (html5ever 0.27), `winit` (smol_str 0.2), `ratatui` (examples) | each is a one-line bump in our manifest or removed with the crate that pulls it (relay, examples, toolbelt) | est. -12 |
| git duplicates: `Mockaco` 2 revs (5 packages), `Solid3D` 2 revs (2) | 7 extra | `ui_git_manager`/`ui_multiplayer` `e36ce16` vs `code_editor` `e3bb374` ("keep Mockaco and WGPUI on the same tree-sitter 0.25 API"); `asset_viewer` `b9eb501` vs Helio `a87fdf0` | pin all to the newer rev in one go per repo | **-7** |
| `profiling` 0.1.0 (path) / 1.0.18 (crates.io `profiling-v1`) | 1 | workspace alias `profiling-v1`; egui/rapier use the real one | by design (the fork); not removable | 0 |
| crates.io vs path | none found (no crate exists both from a path and from a registry except the `profiling` pair above) | | | |
| wgpu/naga/ash | **one** wgpu fork revision (wgpu, wgpu-core, wgpu-hal, wgpu-types 30.0.1, naga 30.0.1, naga-types), `ash` 0.38 once. `wgpu-types 29.0.4` exists in the lock only through bevy (SceneDB dev) and goes with cut 4 | | | -1 |

Not controllable from here (pinned by crates.io crates we do not own): `windows-targets`/`windows_*` splits inside
`windows-sys` generations, `getrandom` 0.2/0.3/0.4 (ring, rand_core 0.6/0.9/0.10), `syn` 1/2/3, `hashbrown` x4,
`block-buffer`/`digest`/`crypto-common`/`cpufeatures` (RustCrypto 0.10 vs 0.11 generations: `sha1`, `sha2`, `hmac`,
`hkdf`; the 0.10 users are `tungstenite`, `jsonwebtoken`, `ed25519-dalek 2`, i.e. mostly relay), `skrifa`/`read-fonts`/
`font-types`/`harfrust` (cosmic-text 0.18 vs `swash`/`egui`), `phf` 0.10-0.14. After cuts 1-12 the duplicate count falls
from 125 names / 167 extra to 77 names / 103 extra (same resolve basis, section 7 tier B); about 60 of those remaining extras
belong to the third-party classes in this paragraph.

## 4. Heavy or redundant crates

Call-site numbers are lines in our own `.rs` files (`pkg::` / `use pkg`), across the 4354 source files of the 322 path
packages.

| family | what is there | who / how many sites | replacement | packages saved | risk |
|---|---|---|---|--:|---|
| async runtimes | `tokio` full (14 members, 158 lines in 39 files), `smol` (19 members, 163 lines in 62 files), `async-std` 1.13 (only via `zed-async-tar`), `async-io`/`async-executor`/`blocking`/`polling` (smol), `futures-lite` 1+2, `async-channel` 1+2, `flume` 0.11+0.12 (`pulsar_lsp`, 31 lines in 11 files), `crossbeam-channel` (`profiling`, `ui_log_viewer`: 2 call sites; `engine_backend` declares it unused), `postage` (gpui test support only), `pollster` x4 | | async-std disappears with cut 8; crossbeam-channel direct deps -> `std::sync::mpsc` (2 sites); `postage` -> dev/test-support in gpui (3 sites in `test_context.rs`) | -6 (async-std and friends with cut 8), -2 (crossbeam-channel, postage) | low |
| HTTP clients | `reqwest 0.13` (6 agent providers, `pulsar_auth`, `friends_engine`, `ui_shell`, `pulsar_engine`: 60 lines in 31 files; `ui_common`, `ui_multiplayer` declare it **unused**), `reqwest 0.12.28` (ToolbeltRS), `zed-reqwest` (gpui), `ureq 3` (`engine_fs`, `pulsar_language`, `ort-sys` build download), `ureq 2` (`ui_level_editor`, 1 file), `tiny_http` (web-demos), relay's axum/hyper/quinn/tungstenite | 13 `ureq` lines in 5 files | one client: reqwest 0.13 blocking (already the dominant one) for the three `ureq` users; ureq stays while `ort-sys` downloads | -3..-6 | low-medium (API rewrite of 13 lines) |
| TLS | `rustls 0.23` + `ring` only (no aws-lc, no openssl); `native-tls` + `schannel` via `ureq`/`ort`; `webpki-roots` 1.0 + 0.26 (relay); `rustls-native-certs` (`async-tungstenite`); `rustls-platform-verifier` (reqwest) | | `ort` `tls-rustls`; relay out | -2 (native) -1 | low |
| serde formats | `serde_json` (154 members, 2431 lines), `toml` 0.8+1.x (16 members, 306 lines in 34 files), `bincode` 1+2 (`helio-bake` on 1.3.3, 9 members on 2), `serde-saphyr`+`json5`+`ron`+`rkyv` only via third parties | | `helio-bake` -> bincode 2 (3 sites); toml 0.8 -> 1 (pulsar-config, ui) | -1, -0..5 | low |
| hashing / crypto | `sha2` 0.10+0.11 (14 members, 16 lines), `sha1` both, `hmac`, `hkdf`, `digest`, `curve25519`, `ed25519-dalek` 2+3, `rsa`, `p256`, `p384`, `ecdsa` (all but sha2/sha1/digest are relay + `jsonwebtoken`); non-crypto: `ahash` (SceneDB, 3 sites), `rustc-hash` 1+2, `siphasher` 0.3+1, `foldhash` 0.1+0.2, `fxhash`, `seahash`, `fnv` | | relay out removes the whole signature stack (about 35); `ahash` -> `foldhash`/`rustc-hash` in SceneDB (3 sites) removes `ahash 0.8` only if egui/scraper also leave: no | in cut 1 | low |
| image / codecs | `image` 0.25 with 8 codecs requested by 33 crates (`png`, `jpeg`, `bmp`, `tga`, `gif`, `webp`, `tiff`, `hdr`, `exr`; 282 lines in 105 files), `png` 0.18 direct in `ui_level_editor` (9 lines), `resvg`/`usvg`/`tiny-skia`, `moxcms`, `exr`+`pulp`, `qoi`, `zune-*` | | the codec set is the asset importer's capability; `png` direct -> `image` (3 files) | 0 | n/a |
| logging | `tracing` (121 members, 1954 lines in 493 files), `log` (61 members, 552 lines in 190 files; wgpu, gpui and boa need `log` anyway), `env_logger` (19 members: 16 are examples; plugin_manager dev, helio-snapshot), `defmt` (via a Helio crate), `opentelemetry-*` (relay), `tracing-subscriber` 4, `tracing-appender` | | `log` stays as the facade of third parties; replacing our 552 sites saves 0 packages | 0 | n/a |
| regex / parsing | `regex` (6 members, 29 lines), `aho-corasick`, `regress` (boa), `pest` (config/json5, relay), `nom`, `winnow` 2, tree-sitter 28 grammars + json, `oxc_*` 14 (TypeScript plugin), `html5ever` 2, `markdown` | | none beyond cuts 5, 8, 11 | | |
| error / lazy / locks | `anyhow` (48 members, 899 lines), `thiserror` 1+2 (32 members, 58 lines), no `eyre`; `once_cell` (8 members, 18 lines), `lazy_static` (3 members, 5 lines), `parking_lot` (43 members, 309 lines in 176 files), `spin` 2, `inventory`/`linkme` (26 / 6 members), `ctor`, `itertools` (`ui`, 11 lines), `futures` (18 members, 64 lines; `futures-util` direct only in relay) | | `once_cell`, `lazy_static`, `parking_lot`, `itertools` are also pulled by 15-37 third-party crates each, so replacing our uses with `std::sync::LazyLock`/`Mutex` saves **0 packages**; do it for style, not for the count | 0 | n/a |
| time | `chrono` (19 members, 66 lines), `time` (8 third-party parents), `jiff` (via `env_logger`'s `humantime`: 5 packages), `web-time` | | `env_logger` with `default-features = false, features = ["auto-color"]` in the 3 non-example users removes `jiff` | -5 (once examples are out the only users are `helio-snapshot`, `plugin_manager` dev, `ui` dev) | low |
| clipboard / OS | `arboard` + `clipboard-rs` both used by gpui (`platform/cross/platform.rs`), `arboard` again in `blueprint_editor_plugin` (4 lines), `rfd`, `open`, `device_query` 2 versions, `winreg`/`sysuri`, `dirs`+`directories` (11 members) | | drop `clipboard-rs` from gpui (windows 0.59 chain: edge cost 7) after porting its image/text paths to `arboard`; `directories` or `dirs`, not both | -7, -3 | medium (clipboard behaviour on Linux/Wayland) |
| audio / camera / ML | `rodio` -> `cpal`, `symphonia` (one 150-line `audio.rs` in `ui_build`), `nokhwa` + `ort` + `gltf` (mocap, optional features `camera`/`pose`) | | see section 8 | | |

## 5. Default features, and the workspace hack

Findings (the edge-cost table in `edges.txt` terms; "enabled by" is the feature that switches the dependency on):

* **`log` kv**: `gpui-ce`, `gpui_util`, `gpui_sum_tree` and `gpui_http_client` ask for `log/kv_unstable_serde`
  (`crates/ui/wgpui/Cargo.toml:65` and three crates.io crates). That enables `value-bag`, `value-bag-sval2`,
  `value-bag-serde1`, `sval*`: **14 packages** for a feature nothing reads. Removing it in our fork is one line; the
  three registry crates need a `[patch.crates-io]` to vendored copies (they are tiny Zed utility crates).
* **`clap` derive+env** in `pulsar_package` (`Cargo.toml:51`): `clap`, `clap_builder`, `clap_derive`, `clap_lex`,
  `anstream`, `anstyle*`, `colorchoice`, `strsim`, `heck`, ...: **13 in the editor closure** for a CLI entry point
  (`src/main.rs`, 168 lines) the editor does not run. Gate the dependency behind a `cli` feature that the `pulsar`
  bin lists in `required-features`; the editor depends on the lib with `default-features = false` already.
* **`ui` feature `tree-sitter-languages`** (root `Cargo.toml:346`, `engine`, `ui_git_manager`, `code_editor`,
  `ui_dylib`): 28 grammar crates plus `tree-sitter-md` from git, each a C compile. All are requested at once; the
  syntax set is a product decision (section 8).
* **`ui` -> `rust_decimal` (default features)** pulls `rkyv`, `bitvec`, `bytecheck`, `ptr_meta`, `hashbrown 0.12`,
  `ahash 0.7` through its `std`/`rkyv` features in the lock (15); only the lock is affected, Windows builds the plain
  crate.
* **`mermaid-rs-renderer`** (`ui`, `default-features = false`) still brings `clap` 4 derive and `dotenv` on non-default
  features: check its `cli` feature stays off (it is off on Windows).
* **`tokio` `full`** is set once in the root `Cargo.toml:722` and 11 members repeat it; unused `signal`/`process`/`fs`
  features cost compile time, not packages. Trim to `rt-multi-thread, sync, time, macros, net, io-util` after a feature
  audit.
* **`image`** is requested with 7 to 9 codecs by 33 crates; `exr` (+`pulp`, `lebe`, `bit_field`, `zune-inflate`,
  `zlib-rs`: 7) is turned on only by `helio-pass-sky` (`hdr`, `exr`) and the sky editor; correct for the product, but it
  is the reason the hack adds packages to an editor-only build.
* **`windows`** 0.62.2 feature lists are per crate and small (`ui`: Direct3D11/Dxgi/Foundation); the sprawl is the
  five generations, not the features.

### The workspace hack (`tools/workspace_hack`, `tools/hack_gen`)

* Size: 2188 lines, 2152 dependency entries in 15 tables (706 direct dependencies on Windows), every pin `=x.y.z` with explicit feature lists; generated
  from `cargo tree --target all` over the editor, every plugin root with `dll`, and the six libraries.
* Cost in **lock** packages: 0 (+1 for itself). On the full workspace it also widens no feature (0 crates differ with
  and without it, measured), because the workspace union is already the superset.
* Cost in an **editor-only build**: 970 vs 955 tree packages, the 14 listed in section 0, from the sky/HDR features of
  `image` and a few feature pins (`raw-cpuid`, `owo-colors`) reaching `pulsar_engine` through the hack.
* It **pins names to exact versions** in all its entries, so `cargo update -p x` cannot move any crate inside the
  libraries without regenerating it; that is deliberate (hash stability) but means every dependency change in sections
  2 to 4 ends with `cargo run -p hack_gen`. Cuts to packages inside the libraries shrink the hack too (it is derived).
* Needed entries: everything inside the six closures (about 570 names); entries outside them would be dead. The
  generator already decides membership from the union resolve; nothing else to prune by hand.
* Keep `[target.<triple>]` tables as generated: they are why Linux does not compile `windows-future`.

## 6. Tree structure

* **Workspace members with no dependents** (28): `pulsar-relay`, 14 `*-examples`, `helio-web-demos`,
  `helio-android-demos`, `meshes-examples`, `lights-examples` (cut 1 and 2: 17 crates, **lock -139**), plus products
  that are fine as roots (`pulsar_engine`, `level_migrate`, `plugin_builder`, `hack_gen`, `todo_check`, `sdk_roots`) and
  libraries nothing links: `one_owner` (0 dependencies), `pulsar_project`, `scene_inventory`.
* **Examples/tools dragging deps into the main lock**: `pulsar-relay` (79 unique packages: `axum`, `sqlx`/postgres,
  `jsonwebtoken`+RustCrypto, `opentelemetry-otlp`/tonic/prost, `rcgen`+`x509-parser`+`asn1-rs`, `quinn`, `config`,
  `prometheus`, `clap`), `meshes-examples` (47: `rapier3d` 0.18 and 0.24, `salva3d`, `egui`+`egui-wgpu`+`epaint`+`vello_cpu`,
  `rusqlite` 0.32 again), `helio-web-demos` (27: `ratatui`, `tiny_http`, `wasm-bindgen`...), the `sprites/decals/post/water/gi`
  examples (13 to 15 unique packages each, mostly `winit`, `env_logger`, `pollster`, `image`). The `experiments/` folder is **not** a problem: it
  is untracked, holds two stand-alone workspaces (`experiments/engine_dylib/toy`, `.../ws`) with their own
  `Cargo.lock`, and no root member includes it.
* How to move them without losing them: give each group its own `[workspace]` root (own lock, own target dir), keep
  `path = ...` dependencies on the shared crates, and list them in `exclude`. The root `Cargo.toml` already uses
  `exclude` for `crates/ui/wgpui`, `crates/renderer/helio`, `crates/third-party/graphy` and nebula. Touch points:
  `.github/workflows/release.yml` (builds the relay image), `deploy/rancher/*`, `architecture/raw-threads-allowed.txt`,
  `architecture/std-fs-allowed.txt`, root `Cargo.toml` members list (explicit entries, lines 5 to 220), and the CI step that
  names "every plugin's examples" (`.github/workflows/ci.yml` lines 117 and 148, 234-237). Alternative that keeps one
  lock: nothing; Cargo has no per-member lock. The examples of a plugin could also stay in the plugin repo as
  `[[example]]` entries of the root crate if their dependencies are already there.
* **Dev/test-only heavy deps** (46 packages on Windows): SceneDB's `bevy_ecs` 0.19 (13) and `criterion` (18) for
  `benches/vs_bevy_ecs.rs`, relay's `mockall`/`proptest`/`criterion`, `plugin_manager` fixtures. Move the SceneDB
  benches to a bench crate outside the workspace (own manifest): lock **-31**.
* **Crates that pull a heavy dependency for one helper**: `ui` -> `rust_decimal` (one `impl Sealed`), `ui_build` ->
  `rodio`+`cpal`+`symphonia` (one 150-line `audio.rs`), `engine_backend` -> PebbleVault (dead), `pulsar_package` -> `clap`
  (bin only), `gpui-ce` -> `stacksafe` -> `stacker` -> `psm` + `cc` (one `#[stacksafe]` in `elements/div.rs`: 6 packages),
  `gpui-ce` -> `device_query` (resize detection fallback), `gpui-ce` -> `postage` (test support only), `ui_level_editor` ->
  `ureq 2` + `png` direct.
* **Tiny crates that could merge**: 250 library crates, 26 of them below 150 lines (`helio-water-api` 27,
  `helio-light-host` 42 but 21 dependents, `post_editor` 32, `water_editor` 36, `fog_editor` 44, `sprites_editor` 57,
  `helio-cables-api` 72, `nebula` 72, `ui_auth` 93, `catalog_fixture_*` 11 to 53). Merging them removes **workspace
  members**, not external packages (the 325 path packages are not the cost); worth it only for build-graph depth.
  Do not merge across the plugin-per-DLL boundary (one DLL per plugin, `BUILD_LESSONS.md`).
* `tools/dep_analysis.ps1` already exists (textual, duplicates + unused); the lint approach here supersedes its
  unused-dependency half.

## 7. Plan

Numbers are cumulative where stated (scratch copy; `lock` is all platforms, `editor`/`game`/`ui_dylib` are Windows
`cargo tree` closures). The hack crate is excluded from the experiments (it is regenerated at the end: +1).

| step | lock | editor closure | game | ui_dylib | wgpui_dylib | helio_dylib |
|---|--:|--:|--:|--:|--:|--:|
| baseline (HEAD) | 1763 | 972 | 508 | 635 | 503 | 710 |
| wave 1: delete 285 unused declared deps (hack removed in the experiment) | 1745 | 955 | 503 | 635 | 502 | 709 |
| **wave 2 (tier A)**: relay + examples/demos out, PebbleVault, SceneDB bench deps, `rust_decimal` | **1501** | **884** | **386** | 634 | 497 | 702 |
| **wave 3 (tier B)**: + gpui http client, ureq 3, toml 1, dirs 6, html5ever 0.39, device_query 4, pollster | **1471** | **858** | 375 | **566** | **414** | 633 |
| wave 3b (est., not run): + `clap` gate (-4 lock, -13 editor), value-bag/`log` kv (-14), git2 0.20 (-1..3), ort `tls-rustls` (-2), `directories 5`/`notify 6` bumps in `ui` (-4), Mockaco/Solid3D single rev (-7), mocap nalgebra 0.35 (-4) | about **1425 to 1440** | about 820 | n/e | n/e | n/e | n/e |
| wave 4 (tier C, **behaviour**): + mathjax, ToolbeltRS built-ins, rodio | 1382 | 775 | 375 | 494 | 414 | 563 |
| wave 4b: + `clap` gate | 1378 | 762 | | | | |

Rows after the baseline are measured **without** the workspace hack (regenerated at the end): add +2 lock, and about +14 to +15 to the editor column (the hack's feature pins, section 5); the other roots do not change. Per-edit detail (lock package deltas, each from the baseline of
wave 1 unless stated): relay -83, examples+demos -55 (both -139), PebbleVault -31 (compile-checked on the whole
workspace), SceneDB dev deps -31, `rust_decimal` -15 (after relay/PebbleVault leave; alone it is 0 because `sqlx`
and `mysql_common` also pull it), mathjax -28, ToolbeltRS -18, rodio -28, gpui http client -17, `html5ever` -2,
`dirs` -1, `device_query` -1, `ureq 2 -> 3` 0 on its own (relay's rustls stack still holds `webpki-roots`) and part of the
tier B delta once relay is gone.

### Waves, in order

**Wave 1: delete unused declared dependencies** (285 entries, Appendix A; risk very low, effort 1 day: the edits are
mechanical, `rmdeps`-style). Hash-neutral for the 33 dev entries and the 231 normal/build entries outside the shared
libraries; 21 entries sit inside a shared-library closure (`helio`, `helio-components`, `helio-pass-planar-reflection`,
`-postprocess`, `-ssr`, `helio-presets`, `helio-xr`, `profiling`, `pulsar_reflection_derive`, `wgpui-base`) and change
those crates' hashes (below). Keep the 18 intentional rows (`sdk_roots`, fixtures, the hack). Then review Appendix B
(139) in the owning repo. Packages: -16 lock; build-graph and diagnostics hygiene.

**Wave 2: structural removals, no feature lost** (about -245 lock, -88 editor, -117 game). Order inside the wave:
1. SceneDB benches out of `pulsar_scenedb` dev-deps (SceneDB repo), `rust_decimal` out of `ui` (WGPUI-Component repo),
   `gpui-ce` untouched yet.
2. `PebbleVault`: delete `crates/core/engine_backend/src/subsystems/world`, the `pub mod world;` in
   `subsystems/mod.rs`, `pub use subsystems::world::World;` (`lib.rs:36`) and `pebble =` (`Cargo.toml:35`). Verified:
   whole workspace `check --all-targets` unchanged. Check no plugin DLL in the field imports `engine_backend::World`
   (nothing in the tree does).
3. Relay, examples, demos: new roots with their own lock, `exclude` in the root manifest, fix the CI/release/deploy
   references listed in section 6.
4. `cargo run -p hack_gen`, commit the regenerated hack.

**Wave 3: unify and fork-edit, no feature lost** (about -30 lock measured, -45 est.). In the WGPUI fork
(Far-Beyond-Pulsar/WGPUI, never gpui-ce): drop `http_client`, `log` `kv_unstable_serde`, bump `device_query` to 4, and
decide `clipboard-rs` vs `arboard`; vendor the three registry crates that force `kv_unstable_serde` and `git2`
(`gpui_util`, `gpui_sum_tree`, `gpui_http_client`) via `[patch.crates-io]`. In the monorepo: `ui_level_editor` ureq 3,
`pulsar-config`/`ui` `toml 1`, `dirs 6`/`directories 6`/`notify 8`, `wgpui-base` html5ever 0.39, `pulsar_game` pollster,
`pulsar_package` `clap` behind `cli`, mocap `nalgebra 0.35`, Mockaco and Solid3D to one revision each, `ort`
`tls-rustls`, `helio-bake` `bincode 2`. Risk medium: each bump is a small API port (`html5ever`, `ureq`, `toml`,
`device_query`); none changes behaviour.

**Wave 4: replace/trade** only with explicit sign-off (section 8).

**Wave 5: structure**: dev-only dependencies out of `[dependencies]` lists (tests/examples), merge tiny crates only
where it also removes a graph edge, keep `experiments/` untracked or add `/experiments` to `.gitignore`.

### Best-case final counts

| scenario | `Cargo.lock` | editor (Windows tree) | game | notes |
|---|--:|--:|--:|---|
| today | 1763 | 972 | 508 | |
| waves 1-2 (measured, features intact) | 1503 | about 899 | 386 | -260 (-15%) |
| waves 1-3 measured | 1473 | about 873 | 375 | -290 (-16%) |
| waves 1-3 with the estimated items | 1427 to 1442 | about 835 | about 370 | **-18% to -19%**: the floor without touching a capability |
| + wave 4 behaviour trades | 1384 | about 790 | 375 | -21% |
| + the product trims of section 8 (grammars, TypeScript, physics, mocap) | about 1290 | about 700 | | about -27%; each is a capability, not recommended |

### Guards (so the count stays down)

* **Lock budget in CI**: `n=$(grep -c '^\[\[package\]\]' Cargo.lock); test "$n" -le "$(cat architecture/lock-budget.txt)"`
  (shrink-only, like `architecture/kind-names.txt`). One line in `just arch` or a CI step; no tool needed. Also record
  the editor tree count: `cargo tree -p pulsar_engine -e normal,build --prefix none --locked | sort -u | wc -l`.
* **Unused dependencies in CI**: the lint run used here is the guard: `RUSTFLAGS="-W unused-crate-dependencies" cargo
  check --workspace --all-targets --locked` with the three known failing units fixed, then fail on any
  `extern crate ... is unused` for a path package (JSON diagnostics, the aggregation used here is 60 lines). Do not
  put `-W unused-crate-dependencies` into the shared RUSTFLAGS of release builds: it changes crate hashes
  (`BUILD_LESSONS.md` section 2); it belongs in its own CI job and its own target dir. `cargo machete` is a cheaper
  second opinion (text based; add `ignored = [...]` for the derive-only names).
* **`cargo deny`**: add `deny.toml` with `[bans] multiple-versions = "deny"`, `skip` entries for the third-party
  classes listed in section 3 (windows*, syn, getrandom, hashbrown, RustCrypto 0.10/0.11, phf, fonts), `wildcards =
  "deny"`, and `[sources] unknown-git = "deny"` with the allow-list of the 14 git repositories; run it with
  `cargo deny check bans sources`. New duplicates then fail review, the existing ones are an explicit list. (Not run:
  `cargo-deny` is not installed here.)
* **`just deps`** recipe: print the lock count, unique names, duplicate names, per-root `cargo tree` counts (the six
  libraries, `pulsar_engine`, `pulsar_game`, `plugin_builder`) and the top 20 direct dependencies by unique packages (the
  script behind Appendix D), so a PR can paste the before/after.
* Keep `just check-submodule-pins` and `hack_gen --check` as they are.

### Order that fits the submodule flow and the shared-dylib rules

1. **Hash-neutral first** (no shared-library hash changes, any order): dev-dependency and bench changes, examples and
   relay moves, plugin-only crates outside the six closures (231 of the safe entries), fixtures, `tools/`. These can
   land as ordinary plugin PRs: change in the plugin repository, merge into its `main`, re-pin in a Pulsar PR
   (`Submodule changes merge dependency-first`, `AGENTS.md`).
2. **Everything inside a shared-library closure in one batch** (the 21 normal/build entries above, `ui`'s
   `rust_decimal`/`directories`/`notify`/`toml`/html5ever, `pulsar_scenedb`'s `ahash`/`socket2`, Helio crates, the WGPUI fork
   edits, `log` features): these change the hash of `ui_dylib`, `wgpui_dylib`, `scenedb_dylib`, `helio_dylib` and hence
   `scenedb_dylib-<hash>.dll` (`BUILD_LESSONS.md` section 5: a graph change renames the git-dependency library). Do
   it as **one coordinated release**: bump all affected library versions together, regenerate the hack once, rebuild
   every plugin, then publish. Several small trickles would each rename the DLLs and each break every plugin built
   against the previous one.
3. Merge order across repositories (dependency first): leaf libraries (`pulsar-profiling`, `pulsar-reflection`,
   SceneDB) -> WGPUI and WGPUI-Component (`crates/ui/wgpui`, `crates/ui/wgpui-component`) -> Helio -> the plugin
   repositories -> the monorepo (re-pin all, apply root-manifest changes, `cargo run -p hack_gen`, update the lock,
   `just check-submodule-pins`). Never commit a `Cargo.lock` produced without reading `git diff Cargo.lock | grep '^[+-]source'`
   (`BUILD_LESSONS.md` section 4; this analysis only ever ran `--locked --offline` against the real tree).
4. Feature changes (`log` kv, tokio features, `image` codecs) must be made in the manifest **and** reflected by the hack
   generator; a feature removed in one place but still requested in another changes nothing and wastes the change.

## 8. Items that remove or change behaviour (not free)

| item | packages (lock) | what is lost | notes |
|---|--:|---|---|
| `mathjax-svg-rs` in `ui` (`src/text/format/markdown.rs:7`) | -28 (boa_engine JS interpreter + 24) | LaTeX math in rendered markdown | replace by a pure-Rust LaTeX renderer, or lazy-load, or accept |
| `tool_registry_builtin` (ToolbeltRS) in `agent_chat_tools` (`src/lib.rs:26`) | -18 (reqwest 0.12, scraper, selectors, cssparser...) | the agent chat's built-in web/HTML tools | upgrade ToolbeltRS to reqwest 0.13 and drop `scraper` to keep the tools at about -8 |
| `rodio` in `ui_build` (`src/runner/audio.rs`) | -28 lock, -10 built | build/run sound playback | Windows PlaySound via the `windows` crate already in the tree is a possible replacement |
| `gpui_http_client` (wave 3) | -17 | URL image loading and any `HttpClient` use in gpui | counted as "no feature" above because nothing in Pulsar uses it; revisit if a web image element is planned |
| `ui` feature `tree-sitter-languages` trimmed to the languages the engine edits (Rust, TypeScript/JS, JSON, TOML, YAML, Markdown, WGSL, C/C++) | up to -29 (28 grammars + `tree-sitter-language`) | syntax highlighting of the dropped languages | a product decision; no per-grammar measurement was run, 1 package and 1 C compile per grammar |
| TypeScript plugin `pulsar_script_ts` (oxc) | -27 | the TypeScript language | |
| physics plugin (`rapier3d`, parry, nalgebra, simba, glamx) | -16 (edge cost) | physics | |
| mocap (`nokhwa`, `ort`, `gltf`) | -16 | camera and pose capture | already optional features `camera`/`pose` of `mocap_capture` |
| `gilrs` (gamepad) in `input_plugin` | -3 | gamepad input | already feature `gamepad` |
| `stacksafe` in gpui (`elements/div.rs:34,1634`) | -6 (stacker, psm, cc) | stack-overflow guard on very deep element trees | |
| `device_query` in gpui resize detection | -1 | the polling fallback strategy for resize | |
| replacing `ureq` users by `reqwest` | -3..-6 | none expected | API rewrite of 13 lines |
| moving `rust_decimal`'s `Sealed` impl | -15 | `Decimal` as a plot scale | "outright removal 5" counts it because nothing uses it; it is a public API of the `ui` crate |

## 9. What was not verified

* Compile time and artifact size (no `--timings`, no release builds). Counts only.
* Other platforms: the lint and every compile ran on Windows with default features. The 6 target-gated safe
  entries (`cfg(windows)`: `winapi`, `windows`, `parking_lot` in `blueprint_editor_plugin`; `openxr` and `env_logger` under
  `cfg(not(target_arch = "wasm32"))`) were checked on Windows only, and deps used only under `cfg(target_os = "linux")`/`macos`/
  `android`/`wasm` or under non-default features (`dll`, `gpu`, `camera`, `pose`, `gamepad`) are in the look list or
  were not seen. Run the same `check` on Linux and macOS before merging wave 1 (2 CI jobs).
* The three failing units (`pulsar_scenedb` lib test + `subsystem_reflection_test`, `pulsar_engine` test
  `linked_editors`) are unrelated to this work and block a full proof for dependencies used only in those units; the
  text search keeps such entries out of the safe list. Doctests are not compiled by `check`, so dev-dependencies used
  only in doctests could be flagged: dev rows were kept in the look list unless they have no text hit at all.
* Wave 3 edits (`html5ever`, `ureq 3`, `toml 1`, `device_query 4`, nalgebra, `notify 8`) were run through **manifest
  resolution only** (the lock numbers are real), not compiled; each needs a small API port and may surface a failure.
  Wave 3b numbers marked "est." are estimates from the edge-cost table, not runs.
* `rust-i18n-support` (toml 0.8, itertools 0.11, serde-saphyr) and the other crates.io crates cannot be changed without
  patches; counts assume they stay.
* The merge in progress may shift lock numbers by a few packages; re-run the scripts (Appendix E) at the new HEAD
  before acting.
* The workspace hack's effect on a plugin-only DLL build was not measured, only on the editor (+15 tree packages).
* `cargo-deny`/`cargo-machete` configurations are proposals.

## Appendix A: safe-to-delete declared dependencies (285; compile-checked)

`[dev]` and `[build]` mark the table; no mark is `[dependencies]`; a trailing parenthesis is the `cfg(...)` target table.
Entries in a plugin or library repository are changed there first (section 7).


**(monorepo: Pulsar-Native)** (71)

- `crates/agent-providers/agent_provider_docker_model_runner/Cargo.toml`: `serde`
- `crates/agent-providers/agent_provider_github_copilot/Cargo.toml`: `serde`
- `crates/core/engine/Cargo.toml`: `plugin_editor_api`, `pulsar_error`, `smol`
- `crates/core/engine_backend/Cargo.toml`: `bitflags`, `dashmap`, `pulsar_error`, `ui_gen_macros`, `uuid`, `winapi` (cfg(windows))
- `crates/core/engine_class_derive/Cargo.toml`: `gpui-ce` [dev]
- `crates/core/engine_state/Cargo.toml`: `directories`, `pulsar-multiplayer-core`
- `crates/core/friends_engine/Cargo.toml`: `base64`, `directories`
- `crates/core/helio_component/Cargo.toml`: `glam`, `pulsar_events`, `pulsar_scenedb_derive`, `naga` [dev]
- `crates/core/plugin_manager/Cargo.toml`: `env_logger` [dev]
- `crates/core/pulsar-multiplayer-core/Cargo.toml`: `chrono`, `futures`, `uuid`
- `crates/core/pulsar-relay/Cargo.toml`: `criterion` [dev], `mockall` [dev], `proptest` [dev], `tempfile` [dev]
- `crates/core/pulsar_class/Cargo.toml`: `glam`
- `crates/core/pulsar_core/Cargo.toml`: `profiling`
- `crates/core/pulsar_findings/Cargo.toml`: `inventory`
- `crates/core/pulsar_lsp/Cargo.toml`: `lsp-types`, `ropey`, `serde`
- `crates/core/pulsar_mcp/Cargo.toml`: `serde`
- `crates/core/pulsar_package/Cargo.toml`: `pulsar_script_runtime`
- `crates/core/pulsar_script_conformance/Cargo.toml`: `glam`, `serde_json`, `serde_json` [build]
- `crates/core/pulsar_settings/Cargo.toml`: `inventory`
- `crates/core/pulsar_std/Cargo.toml`: `pulsar_reflection_derive` [dev], `serde` [dev], `serde_json` [dev]
- `crates/core/window_manager/Cargo.toml`: `serde`
- `crates/editor/ui_common/Cargo.toml`: `reqwest`, `rust-i18n-macro`, `rust-i18n-support`
- `crates/editor/ui_core/Cargo.toml`: `asset_import`, `pulsar_reflection`
- `crates/editor/ui_documentation/Cargo.toml`: `inventory`
- `crates/editor/ui_friends/Cargo.toml`: `chrono`, `engine_state`, `ui_shell`
- `crates/editor/ui_git_manager/Cargo.toml`: `mockaco-core`, `parking_lot`
- `crates/editor/ui_helio_viewport/Cargo.toml`: `tracing`
- `crates/editor/ui_keybindings/Cargo.toml`: `inventory`
- `crates/editor/ui_level_editor/Cargo.toml`: `helio-pass-buffer-view`, `ui_types_common`
- `crates/editor/ui_log_viewer/Cargo.toml`: `anyhow`, `directories`
- `crates/editor/ui_mcp/Cargo.toml`: `plugin_editor_api`
- `crates/editor/ui_multiplayer/Cargo.toml`: `async-tungstenite`, `futures`, `reqwest`, `uuid`
- `crates/editor/ui_plugin_manager/Cargo.toml`: `inventory`, `tracing`
- `crates/editor/ui_shell/Cargo.toml`: `tracing`
- `crates/subsystems/pulsar_scene/Cargo.toml`: `pulsar_reflection`
- `crates/ui/wgpui-base/Cargo.toml`: `ui-macros`

**plugins/vendor/meshes** (42)

- `plugins/vendor/meshes/Cargo.toml`: `pulsar_game` [dev]
- `plugins/vendor/meshes/android-demos/Cargo.toml`: `helio-presets`, `log`
- `plugins/vendor/meshes/asset-compat/Cargo.toml`: `bytemuck`, `helio-pass-forward-lit`, `pulsar_scenedb`
- `plugins/vendor/meshes/component/Cargo.toml`: `helio-settings`, `profiling`, `thiserror`
- `plugins/vendor/meshes/editor/Cargo.toml`: `ui_common`, `tempfile` [dev]
- `plugins/vendor/meshes/examples/Cargo.toml`: `helio-pass-fxaa`, `helio-scene-derivations`, `openxr` (cfg(not(target_arch = "wasm32")))
- `plugins/vendor/meshes/feature/forward/Cargo.toml`: `pulsar_reflection`, `pulsar_scenedb`, `serde_json`
- `plugins/vendor/meshes/feature/gbuffer/Cargo.toml`: `pulsar_reflection`, `pulsar_scenedb`, `serde_json`
- `plugins/vendor/meshes/feature/object-batch/Cargo.toml`: `helio-components` [dev]
- `plugins/vendor/meshes/feature/shadow/Cargo.toml`: `log`, `helio-components` [dev]
- `plugins/vendor/meshes/feature/transparent/Cargo.toml`: `helio-pass-occlusion-cull`, `pulsar_scenedb`
- `plugins/vendor/meshes/feature/vg-bvh/Cargo.toml`: `log`
- `plugins/vendor/meshes/feature/vg-cull/Cargo.toml`: `log`
- `plugins/vendor/meshes/feature/vg-raster/Cargo.toml`: `log`
- `plugins/vendor/meshes/feature/vg/Cargo.toml`: `helio-components` [dev]
- `plugins/vendor/meshes/host/Cargo.toml`: `helio-components`, `pulsar_reflection`, `pulsar_scenedb_derive`, `serde_json`, `image` [dev], `naga` [dev]
- `plugins/vendor/meshes/snapshot/Cargo.toml`: `bytemuck`, `helio-components`, `helio-presets`
- `plugins/vendor/meshes/web-demos/Cargo.toml`: `helio-components`, `helio-core`, `helio-presets`, `env_logger` (cfg(not(target_arch = "wasm32")))

**plugins/vendor/bake** (28)

- `plugins/vendor/bake/vendor/nebula/crates/nebula-ao/Cargo.toml`: `glam`, `log`, `thiserror`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-audio/Cargo.toml`: `log`, `nebula-gpu`, `rayon`, `thiserror`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-core/Cargo.toml`: `bitflags`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-gpu/Cargo.toml`: `glam`, `log`, `thiserror`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-light/Cargo.toml`: `glam`, `log`, `rayon`, `thiserror`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-nav/Cargo.toml`: `bytemuck`, `log`, `nebula-gpu`, `thiserror`, `wgpu`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-probe/Cargo.toml`: `log`, `thiserror`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-serialize/Cargo.toml`: `log`
- `plugins/vendor/bake/vendor/nebula/crates/nebula-visibility/Cargo.toml`: `log`, `nebula-gpu`, `rayon`, `thiserror`, `bitflags` [dev]

**crates/renderer/helio** (19)

- `crates/renderer/helio/crates/helio-components/Cargo.toml`: `glam`, `log`, `pulsar_reflection`, `pulsar_scenedb_derive`, `serde_json`, `wgpu`
- `crates/renderer/helio/crates/helio-presets/Cargo.toml`: `helio-pass-hiz`, `helio-pass-postprocess`
- `crates/renderer/helio/crates/helio-wasm/Cargo.toml`: `glam`
- `crates/renderer/helio/crates/helio-xr/Cargo.toml`: `libloading`
- `crates/renderer/helio/crates/helio/Cargo.toml`: `arrayvec`, `pulsar_reflection`, `thiserror`, `openxr` (cfg(not(target_arch = "wasm32")))
- `crates/renderer/helio/crates/passes/3d/helio-pass-planar-reflection/Cargo.toml`: `log`
- `crates/renderer/helio/crates/passes/3d/helio-pass-postprocess/Cargo.toml`: `pulsar_reflection`, `pulsar_scenedb`
- `crates/renderer/helio/crates/passes/3d/helio-pass-ssr/Cargo.toml`: `log`
- `crates/renderer/helio/crates/scenedb-inspector-agent/Cargo.toml`: `wgpu`

**plugins/vendor/code_editor** (10)

- `plugins/vendor/code_editor/Cargo.toml`: `engine_backend`, `engine_state`, `parking_lot`, `rust-i18n-macro`, `rust-i18n-support`, `similar`, `ui_common`, `ui_types_common`, `uuid`, `wgpui-base`

**plugins/vendor/blueprint_editor** (8)

- `plugins/vendor/blueprint_editor/Cargo.toml`: `arboard`, `futures`, `graphy-render-wgpu`, `parking_lot`, `parking_lot` (cfg(windows)), `schemars`, `wgpu`, `windows` (cfg(windows))

**plugins/vendor/foliage** (8)

- `plugins/vendor/foliage/editor/Cargo.toml`: `parking_lot`
- `plugins/vendor/foliage/examples/Cargo.toml`: `helio-components`, `pulsar_reflection`, `pulsar_scenedb_derive`
- `plugins/vendor/foliage/feature/gbuffer/Cargo.toml`: `pulsar_scenedb`
- `plugins/vendor/foliage/feature/place/Cargo.toml`: `pulsar_reflection`, `pulsar_scenedb`, `serde_json`

**plugins/vendor/sky** (8)

- `plugins/vendor/sky/component/Cargo.toml`: `helio`, `helio_component`
- `plugins/vendor/sky/examples/Cargo.toml`: `helio-core`, `helio-pass-debug-overlay`, `helio-pass-perf-overlay`, `image`, `pulsar_reflection`
- `plugins/vendor/sky/feature/Cargo.toml`: `serde_json`

**plugins/vendor/portals** (8)

- `plugins/vendor/portals/component/Cargo.toml`: `bytemuck`, `helio-settings`, `helio_component`, `pulsar_scenedb_derive`
- `plugins/vendor/portals/examples/Cargo.toml`: `bytemuck`, `helio-components`, `pulsar_reflection`, `pulsar_scenedb_derive`

**plugins/vendor/asset_viewer** (6)

- `plugins/vendor/asset_viewer/Cargo.toml`: `anyhow`, `helio_component`, `parking_lot`, `rust-i18n-macro`, `rust-i18n-support`, `serde`

**plugins/vendor/fog** (6)

- `plugins/vendor/fog/component/Cargo.toml`: `helio_component`, `pulsar_scene_model`
- `plugins/vendor/fog/examples/Cargo.toml`: `helio-components`, `pulsar_reflection`, `pulsar_scenedb_derive`
- `plugins/vendor/fog/feature/Cargo.toml`: `helio-pass-post-volumes` [dev]

**plugins/vendor/gameplay** (6)

- `plugins/vendor/gameplay/Cargo.toml`: `engine_backend`, `pulsar_events`, `pulsar_scenedb`, `pulsar_world_registry`, `serde_json`, `tracing`

**plugins/vendor/sprites** (6)

- `plugins/vendor/sprites/examples/Cargo.toml`: `helio-light-host`
- `plugins/vendor/sprites/feature/batch/Cargo.toml`: `log`, `pulsar_reflection`, `serde_json`
- `plugins/vendor/sprites/feature/cull/Cargo.toml`: `log`
- `plugins/vendor/sprites/feature/simulate/Cargo.toml`: `log`

**crates/third-party/psgc** (6)

- `crates/third-party/psgc/crates/psgc/Cargo.toml`: `serde`, `thiserror`, `tracing-subscriber` [dev]
- `crates/third-party/psgc/crates/wgsl_std/Cargo.toml`: `serde`, `tracing`, `tracing-subscriber` [dev]

**plugins/vendor/default_pipelines** (5)

- `plugins/vendor/default_pipelines/Cargo.toml`: `tracing`, `helio-core` [dev], `pollster` [dev], `tempfile` [dev], `wgpu` [dev]

**plugins/vendor/table_editor** (5)

- `plugins/vendor/table_editor/Cargo.toml`: `engine_state`, `futures`, `ui_common`, `ui_types_common`, `uuid`

**plugins/vendor/lights** (5)

- `plugins/vendor/lights/api/Cargo.toml`: `glam` [dev], `naga` [dev]
- `plugins/vendor/lights/examples/Cargo.toml`: `helio-pass-deferred-light`
- `plugins/vendor/lights/feature/hlfs/Cargo.toml`: `helio-mats` [dev], `pulsar_scenedb` [dev]

**plugins/vendor/voxel** (5)

- `plugins/vendor/voxel/component/Cargo.toml`: `helio-settings`
- `plugins/vendor/voxel/editor/Cargo.toml`: `helio_component`
- `plugins/vendor/voxel/feature/Cargo.toml`: `serde`, `helio-components` [dev], `helio-pass-forward-lit` [dev]

**plugins/vendor/post** (4)

- `plugins/vendor/post/component/Cargo.toml`: `helio_component`, `pulsar_scene_model`
- `plugins/vendor/post/examples/Cargo.toml`: `pulsar_reflection`, `pulsar_scenedb_derive`

**plugins/vendor/skm_editor** (3)

- `plugins/vendor/skm_editor/component/Cargo.toml`: `pulsar_scenedb_derive`
- `plugins/vendor/skm_editor/editor/Cargo.toml`: `serde`, `tracing`

**plugins/vendor/decals** (3)

- `plugins/vendor/decals/feature/Cargo.toml`: `log`, `pulsar_reflection` [dev], `helio-components` [dev]

**plugins/vendor/materials** (3)

- `plugins/vendor/materials/graph_editor/Cargo.toml`: `futures`, `parking_lot`, `wgsl_std`

**plugins/vendor/water** (3)

- `plugins/vendor/water/component/Cargo.toml`: `helio`, `helio_component`
- `plugins/vendor/water/examples/Cargo.toml`: `helio-components`

**crates/third-party/graphy** (2)

- `crates/third-party/graphy/Cargo.toml`: `smallvec`
- `crates/third-party/graphy/crates/graphy-blueprint/Cargo.toml`: `uuid`

**plugins/vendor/particles** (2)

- `plugins/vendor/particles/component/Cargo.toml`: `helio_component`
- `plugins/vendor/particles/feature/Cargo.toml`: `pulsar_reflection`

**plugins/vendor/lens** (2)

- `plugins/vendor/lens/examples/Cargo.toml`: `helio-components`, `pulsar_reflection`

**plugins/vendor/matter_editor** (2)

- `plugins/vendor/matter_editor/Cargo.toml`: `rust-i18n-macro`, `rust-i18n-support`

**crates/external/pulsar-profiling** (2)

- `crates/external/pulsar-profiling/crates/profiling/Cargo.toml`: `crossbeam-channel`, `parking_lot`

**plugins/vendor/trait_editor** (2)

- `plugins/vendor/trait_editor/Cargo.toml`: `serde`, `tracing`

**plugins/vendor/gi** (1)

- `plugins/vendor/gi/examples/Cargo.toml`: `pulsar_reflection`

**plugins/vendor/cables** (1)

- `plugins/vendor/cables/feature/Cargo.toml`: `pulsar_scenedb`

**crates/external/pulsar-reflection** (1)

- `crates/external/pulsar-reflection/crates/pulsar_reflection_derive/Cargo.toml`: `owo-colors`

**plugins/vendor/typescript** (1)

- `plugins/vendor/typescript/compiler/Cargo.toml`: `serde`

**plugins/vendor/splines** (1)

- `plugins/vendor/splines/editor/Cargo.toml`: `tracing`


## Appendix B: needs a look (139 lint hits that the text search could not clear)

Pattern: the name appears in the sources but rustc did not link it on Windows with default features (cfg or feature
gated, doctest, `dll` feature) or the entry is deliberate. The big groups are the Android/WASM demo crates
(`helio-android-demos`, `helio-web-demos`: whole packages gated to other targets), the example crates, Helio's
`helio-feature-api`/`helio-pass-*` dev rows (doctests), `plugin_manager`'s fixtures, and `wgpui-base` (markdown/html
pipeline compiled behind a feature that the default check does not enable).

- `crates/core/engine/Cargo.toml`: `pulsar_workspace_hack` (link-only root, intentional), `rfd` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `plugin_manager` [dev] (named in 0 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_language` [dev] (named in 0 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `tempfile` [dev] (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `windows` (named in 17 src / 0 test / 1 build lines: used only under cfg/feature or doctest; cfg(windows))
- `crates/core/engine_backend/Cargo.toml`: `crossbeam-channel` (feature-forwarded; optional), `ui` (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest; optional)
- `crates/core/plugin_editor_api/Cargo.toml`: `inventory` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_script_vm` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `tracing` [build] (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `window_manager` (feature-forwarded; optional)
- `crates/core/plugin_manager/Cargo.toml`: `engine_state` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-core` (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `parking_lot_core` (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `profiling` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_events` (named in 5 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_settings` (named in 4 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_world_registry` (named in 8 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `fixture_dep_base` [dev] (named in 0 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `fixture_dep_orphan` [dev] (named in 0 src / 1 test / 0 build lines: used only under cfg/feature or doctest), `fixture_dep_top` [dev] (named in 0 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `plugin_loader_fixture` [dev] (named in 0 src / 1 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/core/plugin_manager/tests/fixtures/dependency_plugins/base/Cargo.toml`: `tracing` (), `ui` ()
- `crates/core/plugin_manager/tests/fixtures/dependency_plugins/orphan/Cargo.toml`: `tracing` (), `ui` ()
- `crates/core/plugin_manager/tests/fixtures/dependency_plugins/top/Cargo.toml`: `tracing` (), `ui` ()
- `crates/core/pulsar-relay/Cargo.toml`: `bytes` (named in 16 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `config` (named in 168 src / 32 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/core/pulsar_game/Cargo.toml`: `helio` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `uuid` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/core/pulsar_macros/Cargo.toml`: `linkme` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/core/pulsar_script_vm/Cargo.toml`: `pulsar_script_vm_test_library` [dev] (named in 0 src / 1 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/core/pulsar_traits/Cargo.toml`: `pulsar_traits` [dev] (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/editor/ui_common/Cargo.toml`: `window_manager` (named in 4 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/editor/ui_friends/Cargo.toml`: `pulsar_auth` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/editor/ui_log_viewer/Cargo.toml`: `notify` (named in 33 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `tokio` (named in 4 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/editor/ui_multiplayer/Cargo.toml`: `engine_backend` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/external/scenedb/crates/pulsar_scenedb/Cargo.toml`: `bevy_ecs` [dev] (named in 0 src / 11 test / 0 build lines: used only under cfg/feature or doctest), `crossterm` [dev] (named in 0 src / 3 test / 0 build lines: used only under cfg/feature or doctest), `naga` [dev] (named in 0 src / 13 test / 0 build lines: used only under cfg/feature or doctest), `ratatui` [dev] (named in 0 src / 15 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/helio-components/Cargo.toml`: `bytemuck` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-core` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/helio-presets/Cargo.toml`: `glam` (named in 0 src / 3 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-tsr` (named in 0 src / 9 test / 0 build lines: used only under cfg/feature or doctest), `image` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `wgpu` (named in 0 src / 74 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/helio/Cargo.toml`: `image` (named in 13 src / 2 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/passes/3d/helio-pass-fxaa/Cargo.toml`: `bytemuck` (named in 0 src / 1 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/passes/3d/helio-pass-ssr/Cargo.toml`: `bytemuck` (named in 3 src / 4 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/renderer/helio/crates/scenedb-inspector-agent/Cargo.toml`: `pulsar_reflection` (named in 0 src / 1 test / 0 build lines: used only under cfg/feature or doctest)
- `crates/ui/wgpui-base/Cargo.toml`: `chrono` (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `data-url` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `futures` (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `html5ever` (named in 9 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `markdown` (named in 129 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `markup5ever_rcdom` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `tracing` (named in 11 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `web-time` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `smol` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest; cfg(not(target_family = "wasm"))), `windows` (named in 16 src / 0 test / 0 build lines: used only under cfg/feature or doctest; cfg(target_os = "windows"))
- `plugins/vendor/bake/vendor/nebula/crates/nebula-gpu/Cargo.toml`: `nebula-core` [dev] (named in 0 src / 5 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/bake/vendor/nebula/crates/nebula-serialize/Cargo.toml`: `bincode` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/blueprint_editor/Cargo.toml`: `pulsar_plugins` (named in 6 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/decals/Cargo.toml`: `helio-feature-api` [dev] (named in 4 src / 1 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/decals/examples/Cargo.toml`: `helio-components` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/default_pipelines/Cargo.toml`: `helio` [dev] (named in 5 src / 5 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/fog/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/fog/feature/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/foliage/api/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/foliage/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/gi/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/lens/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/lens/feature/lens/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/lights/api/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/lights/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/lights/feature/billboard/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/lights/feature/deferred/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/materials/Cargo.toml`: `helio-feature-api` [dev] (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/materials/api/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/materials/editor/Cargo.toml`: `tracing` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `ui` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/materials/graph_editor/Cargo.toml`: `log` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/Cargo.toml`: `pulsar_scene_model` [dev] (named in 16 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `ui` (named in 11 src / 19 test / 0 build lines: used only under cfg/feature or doctest; optional), `helio-feature-api` [dev] (named in 8 src / 2 test / 0 build lines: used only under cfg/feature or doctest), `helio-vg-build` [dev] (named in 4 src / 6 test / 0 build lines: used only under cfg/feature or doctest), `image` [dev] (named in 62 src / 37 test / 0 build lines: used only under cfg/feature or doctest), `naga` [dev] (named in 30 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_reflection` [dev] (named in 30 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/android-demos/Cargo.toml`: `env_logger` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `glam` (named in 24 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio` (named in 9 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-feature-api` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-light-api` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-mesh-api` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-mesh-host` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-materials` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-wasm` (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_scenedb` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `wgpu` (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `winit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest; cfg(not(target_os = "android"))), `winit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest; cfg(target_os = "android"))
- `plugins/vendor/meshes/api/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/meshes/asset-compat/Cargo.toml`: `helio` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/component/Cargo.toml`: `image` (named in 9 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_class` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `pulsar_settings` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/examples/Cargo.toml`: `helio-feature-api` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-gi` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-sky` (named in 4 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-vg` (named in 2 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-vg-bvh` (named in 5 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-vg-cull` (named in 11 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-vg-raster` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-vg-build` (named in 6 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-vg-format` (named in 20 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `rayon` (named in 9 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `serde` (named in 38 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `serde_json` (named in 78 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `serde_json` [build] (named in 78 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/feature/vg-shade/Cargo.toml`: `log` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/meshes/web-demos/Cargo.toml`: `bytemuck` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-asset-compat` (named in 3 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-feature-api` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-materials` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-wasm` (named in 183 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `log` (named in 30 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/particles/examples/Cargo.toml`: `helio-components` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/portals/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/post/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/post/feature/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `plugins/vendor/sky/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/sprites/Cargo.toml`: `helio-feature-api` [dev] (named in 2 src / 1 test / 0 build lines: used only under cfg/feature or doctest), `helio-pass-sprite-batch` [dev] (named in 12 src / 14 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/sprites/examples/Cargo.toml`: `helio` (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest), `helio-components` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/table_editor/Cargo.toml`: `open` (named in 7 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/voxel/Cargo.toml`: `helio-feature-api` [dev] (named in 15 src / 3 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/voxel/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/water/examples/Cargo.toml`: `helio-pass-forward-lit` (named in 1 src / 0 test / 0 build lines: used only under cfg/feature or doctest)
- `plugins/vendor/water/feature/Cargo.toml`: `serde_json` (feature-forwarded; optional)
- `tools/sdk_roots/Cargo.toml`: `helio_dylib` (link-only root, intentional), `profiling_dylib` (link-only root, intentional), `pulsar_workspace_hack` (link-only root, intentional), `reflection_dylib` (link-only root, intentional), `scenedb_dylib` (link-only root, intentional), `ui_dylib` (link-only root, intentional), `wgpui_dylib` (link-only root, intentional)


## Appendix C: duplicate versions (built on Windows; the `glam` 0.30/0.31/0.32 entries are lock-only)

Format: name: version (parents; example chain from a workspace crate, `*` = workspace crate).

- **glam x5**: 0.27.0 (by solid-fbx 0.1.0, solid-gltf 0.1.0, solid-obj 0.1.0, solid-rs 0.1.0, +2; e.g. asset_viewer_plugin*>solid-fbx 0.1.0>glam 0.27.0) | 0.30.10 (by nalgebra 0.35.0; e.g. physics_plugin*>rapier3d 0.35.3>nalgebra 0.35.0>..) | 0.31.1 (by nalgebra 0.35.0; e.g. physics_plugin*>rapier3d 0.35.3>nalgebra 0.35.0>..) | 0.32.1 (by nalgebra 0.35.0; e.g. physics_plugin*>rapier3d 0.35.3>nalgebra 0.35.0>..) | 0.33.5 (by animation_component*, asset_viewer_plugin*, cables_component*, decals-examples*, +81; e.g. pulsar_reflection*>glam 0.33.5)
- **hashbrown x5**: 0.12.3 (by rkyv 0.7.46; e.g. ui*>rust_decimal 1.42.1>rkyv 0.7.46>..) | 0.14.5 (by dashmap 6.2.1, hashlink 0.9.1, ordered-multimap 0.7.3; e.g. pulsar_reflection*>dashmap 6.2.1>hashbrown 0.14.5) | 0.15.5 (by fontdue 0.9.4, lru 0.12.5, parry3d 0.19.0; e.g. blueprint_editor_plugin*>graphy-render-wgpu*>fontdue 0.9.4>..) | 0.16.1 (by boa_engine 0.21.1, boa_gc 0.21.1, boa_interner 0.21.1, gpu-allocator 0.28.0, +4; e.g. ui*>mathjax-svg-rs 0.4.0>boa_engine 0.21.1>..) | 0.17.1 (by glifo 0.2.0, indexmap 2.14.0, naga 30.0.1, naga-types 30.0.1, +7; e.g. pulsar_engine*>wgpu 30.0.1>hashbrown 0.17.1)
- **itertools x5**: 0.11.0 (by rust-i18n-support 4.2.1; e.g. ui_common*>rust-i18n-support 4.2.1>itertools 0.11.0) | 0.12.1 (by bindgen 0.69.5, salva3d 0.9.0; e.g. meshes-examples*>salva3d 0.9.0>itertools 0.12.1) | 0.13.0 (by ratatui 0.29.0, unicode-truncate 1.1.0; e.g. helio-web-demos*>ratatui 0.29.0>itertools 0.13.0) | 0.14.0 (by boa_engine 0.21.1, gpui-ce*, gpui_util 0.2.2, prost-derive 0.14.4, +1; e.g. ui*>itertools 0.14.0) | 0.15.0 (by egui 0.36.2; e.g. meshes-examples*>egui 0.36.2>itertools 0.15.0)
- **windows x5**: 0.34.0 (by win-sys 0.3.1; e.g. helio-voxel-examples*>scenedb-inspector-agent*>shared_memory 0.12.4>..) | 0.48.0 (by device_query 2.1.0, device_query 4.0.1; e.g. ui_level_editor*>device_query 4.0.1>windows 0.48.0) | 0.54.0 (by cpal 0.16.0; e.g. ui_build*>rodio 0.21.1>cpal 0.16.0>..) | 0.59.0 (by clipboard-rs 0.3.5; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.62.2 (by blueprint_editor_plugin*, engine_backend*, gilrs-core 0.6.8, gpu-allocator 0.28.0, +8; e.g. pulsar_engine*>windows 0.62.2)
- **windows-sys x5**: 0.48.0 (by dirs-sys 0.4.1, notify 6.1.1, winreg 0.52.0; e.g. ui*>notify 6.1.1>windows-sys 0.48.0) | 0.52.0 (by socket2 0.5.10, winit 0.30.13; e.g. pulsar_scenedb*>socket2 0.5.10>windows-sys 0.52.0) | 0.59.0 (by colored 2.2.0, console 0.15.11, rustix 0.38.44; e.g. engine_backend*>PebbleVault 0.7.0>colored 2.2.0>..) | 0.60.2 (by arboard 3.6.1, notify 8.2.0; e.g. blueprint_editor_plugin*>arboard 3.6.1>windows-sys 0.60.2) | 0.61.2 (by anstyle-query 1.1.5, anstyle-wincon 3.0.11, async-io 2.6.0, colored 3.1.1, +21; e.g. pulsar_engine*>rfd 0.17.2>windows-sys 0.61.2)
- **phf x4**: 0.10.1 (by selectors 0.25.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>scraper 0.20.0>..) | 0.11.3 (by cssparser 0.31.2, markup5ever 0.12.1; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.13.1 (by boa_interner 0.21.1, tokio-postgres 0.7.18, web_atoms 0.2.6; e.g. engine_backend*>PebbleVault 0.7.0>tokio-postgres 0.7.18>..) | 0.14.0 (by oxc_ast_macros 0.152.0, oxc_regular_expression 0.152.0, oxc_syntax 0.152.0; e.g. pulsar_script_ts*>oxc_ast 0.152.0>oxc_ast_macros 0.152.0>..)
- **phf_generator x4**: 0.10.0 (by phf_codegen 0.10.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>scraper 0.20.0>..) | 0.11.3 (by phf_codegen 0.11.3, phf_macros 0.11.3, string_cache_codegen 0.5.4; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.13.1 (by phf_codegen 0.13.1, phf_macros 0.13.1, string_cache_codegen 0.6.1; e.g. engine_backend*>PebbleVault 0.7.0>tokio-postgres 0.7.18>..) | 0.14.0 (by phf_macros 0.14.0; e.g. pulsar_script_ts*>oxc_ast 0.152.0>oxc_ast_macros 0.152.0>..)
- **phf_shared x4**: 0.10.0 (by phf 0.10.1, phf_codegen 0.10.0, phf_generator 0.10.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>scraper 0.20.0>..) | 0.11.3 (by phf 0.11.3, phf_codegen 0.11.3, phf_generator 0.11.3, phf_macros 0.11.3, +2; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.13.1 (by phf 0.13.1, phf_codegen 0.13.1, phf_generator 0.13.1, phf_macros 0.13.1, +2; e.g. engine_backend*>PebbleVault 0.7.0>tokio-postgres 0.7.18>..) | 0.14.0 (by phf 0.14.0, phf_generator 0.14.0, phf_macros 0.14.0; e.g. pulsar_script_ts*>oxc_ast 0.152.0>oxc_ast_macros 0.152.0>..)
- **pollster x4**: 0.2.5 (by postage 0.5.0; e.g. pulsar_engine*>gpui-ce*>postage 0.5.0>..) | 0.3.0 (by nebula-core*; e.g. nebula-core*>pollster 0.3.0) | 0.4.0 (by asset_viewer_plugin*, decals-examples*, engine_backend*, fog-examples*, +19; e.g. engine_backend*>pollster 0.4.0) | 1.0.1 (by pulsar_game*; e.g. pulsar_game*>pollster 1.0.1)
- **windows_x86_64_msvc x4**: 0.34.0 (by windows 0.34.0; e.g. helio-voxel-examples*>scenedb-inspector-agent*>shared_memory 0.12.4>..) | 0.48.5 (by windows-targets 0.48.5; e.g. ui*>notify 6.1.1>windows-sys 0.48.0>..) | 0.52.6 (by windows-targets 0.52.6; e.g. pulsar_scenedb*>socket2 0.5.10>windows-sys 0.52.0>..) | 0.53.1 (by windows-targets 0.53.5; e.g. blueprint_editor_plugin*>arboard 3.6.1>windows-sys 0.60.2>..)
- **bit-vec x3**: 0.6.3 (by rapier3d 0.18.0; e.g. meshes-examples*>salva3d 0.9.0>rapier3d 0.18.0>..) | 0.8.0 (by rapier3d 0.24.0; e.g. meshes-examples*>rapier3d 0.24.0>bit-vec 0.8.0) | 0.9.1 (by bit-set 0.10.0, wgpu-core 30.0.1, yasna 0.6.0; e.g. pulsar_engine*>wgpu 30.0.1>wgpu-core 30.0.1>..)
- **convert_case x3**: 0.4.0 (by derive_more 0.99.20; e.g. pulsar_engine*>gpui-ce*>gpui_http_client 0.2.2>..) | 0.6.0 (by config 0.15.25; e.g. pulsar-relay*>config 0.15.25>convert_case 0.6.0) | 0.10.0 (by derive_more-impl 2.1.1; e.g. pulsar_engine*>gpui-ce*>derive_more 2.1.1>..)
- **dirs x3**: 4.0.0 (by gpui_util 0.2.2; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 5.0.1 (by pulsar-config*; e.g. pulsar-config*>dirs 5.0.1) | 6.0.0 (by shellexpand 3.1.2, ui_core*, ui_fab_search*; e.g. ui_core*>dirs 6.0.0)
- **dirs-sys x3**: 0.3.7 (by dirs 4.0.0; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 0.4.1 (by directories 5.0.1, dirs 5.0.1; e.g. pulsar-config*>dirs 5.0.1>dirs-sys 0.4.1) | 0.5.0 (by directories 6.0.0, dirs 6.0.0; e.g. pulsar_engine*>directories 6.0.0>dirs-sys 0.5.0)
- **getrandom x3**: 0.2.17 (by ahash 0.7.8, const-random-macro 0.1.16, nanorand 0.7.0, rand_core 0.6.4, +1; e.g. pulsar_engine*>rustls 0.23.46>ring 0.17.14>..) | 0.3.4 (by ahash 0.8.12, mysql_common 0.35.5, pulsar_content*, rand_core 0.9.5; e.g. pulsar_content*>getrandom 0.3.4) | 0.4.3 (by jobserver 0.1.35, rand 0.10.2, tempfile 3.27.0, uuid 1.25.0, +1; e.g. pulsar_script_vm*>uuid 1.25.0>getrandom 0.4.3)
- **nalgebra x3**: 0.32.6 (by mocap_capture*, mocap_editor*, parry3d 0.13.8, rapier3d 0.18.0, +1; e.g. mocap_capture*>nalgebra 0.32.6) | 0.33.3 (by parry3d 0.19.0, rapier3d 0.24.0; e.g. meshes-examples*>rapier3d 0.24.0>nalgebra 0.33.3) | 0.35.0 (by glamx 0.3.0, rapier3d 0.35.3; e.g. physics_plugin*>rapier3d 0.35.3>nalgebra 0.35.0)
- **parry3d x3**: 0.13.8 (by rapier3d 0.18.0, salva3d 0.9.0; e.g. meshes-examples*>salva3d 0.9.0>parry3d 0.13.8) | 0.19.0 (by rapier3d 0.24.0; e.g. meshes-examples*>rapier3d 0.24.0>parry3d 0.19.0) | 0.30.2 (by rapier3d 0.35.3; e.g. physics_plugin*>rapier3d 0.35.3>parry3d 0.30.2)
- **phf_codegen x3**: 0.10.0 (by selectors 0.25.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>scraper 0.20.0>..) | 0.11.3 (by markup5ever 0.12.1; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.13.1 (by web_atoms 0.2.6; e.g. ui*>html5ever 0.39.0>markup5ever 0.39.0>..)
- **phf_macros x3**: 0.11.3 (by phf 0.11.3; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.13.1 (by phf 0.13.1; e.g. engine_backend*>PebbleVault 0.7.0>tokio-postgres 0.7.18>..) | 0.14.0 (by phf 0.14.0; e.g. pulsar_script_ts*>oxc_ast 0.152.0>oxc_ast_macros 0.152.0>..)
- **rand x3**: 0.8.8 (by PebbleVault 0.7.0, jsonwebtoken 10.4.0, num-bigint-dig 0.8.6, phf_generator 0.10.0, +3; e.g. engine_backend*>PebbleVault 0.7.0>rand 0.8.8) | 0.9.5 (by boa_engine 0.21.1, gpui_util 0.2.2, opentelemetry_sdk 0.33.1, tungstenite 0.29.0; e.g. pulsar-relay*>opentelemetry_sdk 0.33.1>rand 0.9.5) | 0.10.2 (by gpui-ce*, postgres-protocol 0.6.12, pulsar-relay*, pulsar_std*, +6; e.g. pulsar_std*>rand 0.10.2)
- **rand_core x3**: 0.6.4 (by crypto-bigint 0.5.5, elliptic-curve 0.13.8, ff 0.13.1, group 0.13.0, +4; e.g. engine_backend*>PebbleVault 0.7.0>rand 0.8.8>..) | 0.9.5 (by rand 0.9.5, rand_chacha 0.9.0; e.g. pulsar-relay*>opentelemetry_sdk 0.33.1>rand 0.9.5>..) | 0.10.1 (by chacha20 0.10.2, crypto-common 0.2.2, curve25519-dalek 5.0.0, ed25519-dalek 3.0.0, +4; e.g. pulsar_content*>chacha20 0.10.2>rand_core 0.10.1)
- **rapier3d x3**: 0.18.0 (by salva3d 0.9.0; e.g. meshes-examples*>salva3d 0.9.0>rapier3d 0.18.0) | 0.24.0 (by meshes-examples*; e.g. meshes-examples*>rapier3d 0.24.0) | 0.35.3 (by physics_plugin*; e.g. physics_plugin*>rapier3d 0.35.3)
- **simba x3**: 0.8.1 (by nalgebra 0.32.6, parry3d 0.13.8, rapier3d 0.18.0; e.g. mocap_capture*>nalgebra 0.32.6>simba 0.8.1) | 0.9.1 (by nalgebra 0.33.3, parry3d 0.19.0, rapier3d 0.24.0; e.g. meshes-examples*>rapier3d 0.24.0>simba 0.9.1) | 0.10.2 (by glamx 0.3.0, nalgebra 0.35.0, parry3d 0.30.2, rapier3d 0.35.3; e.g. physics_plugin*>rapier3d 0.35.3>simba 0.10.2)
- **syn x3**: 1.0.109 (by binread_derive 2.1.0, bytecheck_derive 0.6.12, ptr_meta_derive 0.1.4, rkyv_derive 0.7.46; e.g. ui*>rust_decimal 1.42.1>rkyv 0.7.46>..) | 2.0.119 (by asn1-rs-derive 0.6.0, asn1-rs-impl 0.2.0, axum-macros 0.5.1, bindgen 0.69.5, +62; e.g. pulsar_reflection_derive*>syn 2.0.119) | 3.0.4 (by async-trait 0.1.92, bytemuck_derive 1.12.0, clap_derive 4.6.4, darling_core 0.24.1, +15; e.g. pulsar-multiplayer-core*>async-trait 0.1.92>syn 3.0.4)
- **windows-core x3**: 0.54.0 (by windows 0.54.0; e.g. ui_build*>rodio 0.21.1>cpal 0.16.0>..) | 0.59.0 (by windows 0.59.0; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.62.2 (by wgpu-hal 30.0.1, windows 0.62.2, windows-collections 0.3.2, windows-future 0.3.2, +1; e.g. pulsar_engine*>windows 0.62.2>windows-core 0.62.2)
- **windows-result x3**: 0.1.2 (by windows-core 0.54.0; e.g. ui_build*>rodio 0.21.1>cpal 0.16.0>..) | 0.3.4 (by windows-core 0.59.0, windows-registry 0.4.0; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.4.1 (by wgpu-hal 30.0.1, windows-core 0.62.2, windows-registry 0.6.1; e.g. pulsar_engine*>wgpu 30.0.1>wgpu-hal 30.0.1>..)
- **windows-targets x3**: 0.48.5 (by windows 0.48.0, windows-sys 0.48.0; e.g. ui*>notify 6.1.1>windows-sys 0.48.0>..) | 0.52.6 (by windows 0.54.0, windows-core 0.54.0, windows-result 0.1.2, windows-sys 0.52.0, +1; e.g. pulsar_scenedb*>socket2 0.5.10>windows-sys 0.52.0>..) | 0.53.5 (by windows 0.59.0, windows-core 0.59.0, windows-registry 0.4.0, windows-sys 0.60.2; e.g. blueprint_editor_plugin*>arboard 3.6.1>windows-sys 0.60.2>..)
- **ahash x2**: 0.7.8 (by hashbrown 0.12.3; e.g. ui*>rust_decimal 1.42.1>rkyv 0.7.46>..) | 0.8.12 (by egui 0.36.2, egui-wgpu 0.36.2, epaint 0.36.2, hashbrown 0.14.5, +3; e.g. pulsar_scenedb*>ahash 0.8.12)
- **async-channel x2**: 1.9.0 (by async-std 1.13.2; e.g. pulsar_engine*>gpui-ce*>gpui_http_client 0.2.2>..) | 2.5.0 (by async-global-executor 2.4.1, async-process 2.5.0, blocking 1.7.0, smol 2.0.2; e.g. pulsar_engine*>smol 2.0.2>async-channel 2.5.0)
- **base64 x2**: 0.22.1 (by axum 0.8.9, hyper-util 0.1.20, jsonwebtoken 10.4.0, mysql_common 0.35.5, +13; e.g. ui*>base64 0.22.1) | 0.23.1 (by engine_fs*, friends_engine*, pulsar-relay*, pulsar_mcp*, +2; e.g. engine_fs*>base64 0.23.1)
- **bincode x2**: 1.3.3 (by helio-bake*; e.g. helio-bake*>bincode 1.3.3) | 2.0.1 (by gpui-ce*, nebula-audio*, nebula-nav*, nebula-probe*, +4; e.g. pulsar_script_vm*>bincode 2.0.1)
- **bitflags x2**: 1.3.2 (by defmt 1.1.1, fluent-uri 0.1.4, globwalk 0.8.1, lsp-types 0.97.0, +3; e.g. ui*>lsp-types 0.97.0>bitflags 1.3.2) | 2.13.1 (by bindgen 0.69.5, boa_ast 0.21.1, boa_engine 0.21.1, boa_parser 0.21.1, +42; e.g. engine_backend*>bitflags 2.13.1)
- **block-buffer x2**: 0.10.4 (by digest 0.10.7; e.g. pulsar-relay*>axum 0.8.9>sha1 0.10.7>..) | 0.12.1 (by digest 0.11.3; e.g. plugin_manager*>sha2 0.11.0>digest 0.11.3>..)
- **colored x2**: 2.2.0 (by PebbleVault 0.7.0; e.g. engine_backend*>PebbleVault 0.7.0>colored 2.2.0) | 3.1.1 (by pulsar-relay*; e.g. pulsar-relay*>colored 3.1.1)
- **compact_str x2**: 0.8.2 (by ratatui 0.29.0; e.g. helio-web-demos*>ratatui 0.29.0>compact_str 0.8.2) | 0.10.0 (by oxc_span 0.152.0, oxc_str 0.152.0; e.g. pulsar_script_ts*>oxc_span 0.152.0>compact_str 0.10.0)
- **const-oid x2**: 0.9.6 (by der 0.7.10, digest 0.10.7, rsa 0.9.10; e.g. pulsar-relay*>jsonwebtoken 10.4.0>rsa 0.9.10>..) | 0.10.2 (by digest 0.11.3; e.g. plugin_manager*>sha2 0.11.0>digest 0.11.3>..)
- **cpufeatures x2**: 0.2.17 (by curve25519-dalek 4.1.3, sha1 0.10.7, sha2 0.10.9; e.g. pulsar-relay*>axum 0.8.9>sha1 0.10.7>..) | 0.3.0 (by chacha20 0.10.2, curve25519-dalek 5.0.0, sha1 0.11.0, sha2 0.11.0; e.g. pulsar_content*>chacha20 0.10.2>cpufeatures 0.3.0)
- **crypto-common x2**: 0.1.6 (by digest 0.10.7; e.g. pulsar-relay*>axum 0.8.9>sha1 0.10.7>..) | 0.2.2 (by digest 0.11.3; e.g. plugin_manager*>sha2 0.11.0>digest 0.11.3>..)
- **curve25519-dalek x2**: 4.1.3 (by ed25519-dalek 2.2.0; e.g. pulsar-relay*>jsonwebtoken 10.4.0>ed25519-dalek 2.2.0>..) | 5.0.0 (by ed25519-dalek 3.0.0; e.g. pulsar-relay*>ed25519-dalek 3.0.0>curve25519-dalek 5.0.0)
- **darling x2**: 0.23.0 (by mysql-common-derive 0.32.2; e.g. engine_backend*>PebbleVault 0.7.0>mysql 26.0.1>..) | 0.24.1 (by instability 0.3.14; e.g. helio-web-demos*>ratatui 0.29.0>instability 0.3.14>..)
- **darling_core x2**: 0.23.0 (by darling 0.23.0, darling_macro 0.23.0; e.g. engine_backend*>PebbleVault 0.7.0>mysql 26.0.1>..) | 0.24.1 (by darling 0.24.1, darling_macro 0.24.1; e.g. helio-web-demos*>ratatui 0.29.0>instability 0.3.14>..)
- **darling_macro x2**: 0.23.0 (by darling 0.23.0; e.g. engine_backend*>PebbleVault 0.7.0>mysql 26.0.1>..) | 0.24.1 (by darling 0.24.1; e.g. helio-web-demos*>ratatui 0.29.0>instability 0.3.14>..)
- **der x2**: 0.7.10 (by ecdsa 0.16.9, pkcs1 0.7.5, pkcs8 0.10.2, sec1 0.7.3, +1; e.g. pulsar-relay*>jsonwebtoken 10.4.0>p256 0.13.2>..) | 0.8.3 (by ureq 3.4.0; e.g. engine_fs*>ureq 3.4.0>der 0.8.3)
- **derive_more x2**: 0.99.20 (by gpui_http_client 0.2.2, selectors 0.25.0; e.g. pulsar_engine*>gpui-ce*>gpui_http_client 0.2.2>..) | 2.1.1 (by gpui-ce*; e.g. pulsar_engine*>gpui-ce*>derive_more 2.1.1)
- **device_query x2**: 2.1.0 (by gpui-ce*; e.g. pulsar_engine*>gpui-ce*>device_query 2.1.0) | 4.0.1 (by ui_level_editor*; e.g. ui_level_editor*>device_query 4.0.1)
- **digest x2**: 0.10.7 (by curve25519-dalek 4.1.3, ecdsa 0.16.9, elliptic-curve 0.13.8, hmac 0.12.1, +4; e.g. pulsar-relay*>axum 0.8.9>sha1 0.10.7>..) | 0.11.3 (by curve25519-dalek 5.0.0, hmac 0.13.0, md-5 0.11.0, sha1 0.11.0, +2; e.g. plugin_manager*>sha2 0.11.0>digest 0.11.3)
- **directories x2**: 5.0.1 (by ui*; e.g. ui*>directories 5.0.1) | 6.0.0 (by engine_state*, friends_engine*, pulsar_auth*, pulsar_engine*, +5; e.g. pulsar_engine*>directories 6.0.0)
- **downcast-rs x2**: 1.2.1 (by parry3d 0.13.8, rapier3d 0.18.0; e.g. meshes-examples*>salva3d 0.9.0>parry3d 0.13.8>..) | 2.0.2 (by parry3d 0.19.0, parry3d 0.30.2, rapier3d 0.24.0; e.g. meshes-examples*>rapier3d 0.24.0>downcast-rs 2.0.2)
- **ed25519 x2**: 2.2.3 (by ed25519-dalek 2.2.0; e.g. pulsar-relay*>jsonwebtoken 10.4.0>ed25519-dalek 2.2.0>..) | 3.0.0 (by ed25519-dalek 3.0.0; e.g. pulsar-relay*>ed25519-dalek 3.0.0>ed25519 3.0.0)
- **ed25519-dalek x2**: 2.2.0 (by jsonwebtoken 10.4.0; e.g. pulsar-relay*>jsonwebtoken 10.4.0>ed25519-dalek 2.2.0) | 3.0.0 (by pulsar-relay*; e.g. pulsar-relay*>ed25519-dalek 3.0.0)
- **event-listener x2**: 2.5.3 (by async-channel 1.9.0; e.g. pulsar_engine*>gpui-ce*>gpui_http_client 0.2.2>..) | 5.4.2 (by async-lock 3.4.2, async-process 2.5.0, engine_state*, event-listener-strategy 0.5.4, +1; e.g. engine_state*>event-listener 5.4.2)
- **fallible-iterator x2**: 0.2.0 (by postgres 0.19.14, postgres-protocol 0.6.12, postgres-types 0.2.14, tokio-postgres 0.7.18; e.g. engine_backend*>PebbleVault 0.7.0>postgres 0.19.14>..) | 0.3.0 (by rusqlite 0.32.1; e.g. engine_fs*>rusqlite 0.32.1>fallible-iterator 0.3.0)
- **fastrand x2**: 1.9.0 (by futures-lite 1.13.0; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 2.5.0 (by async-executor 1.14.1, flume 0.12.0, futures-lite 2.6.1, phf_generator 0.13.1, +3; e.g. pulsar_lsp*>flume 0.12.0>fastrand 2.5.0)
- **fixedbitset x2**: 0.4.2 (by petgraph 0.6.5; e.g. parking_lot_core*>petgraph 0.6.5>fixedbitset 0.4.2) | 0.5.7 (by futures-concurrency 7.7.1; e.g. pulsar_engine*>gpui-ce*>futures-concurrency 7.7.1>..)
- **float-cmp x2**: 0.9.0 (by strict-num 0.1.1; e.g. pulsar_engine*>gpui-ce*>usvg 0.47.0>..) | 0.10.0 (by meshopt 0.7.0; e.g. helio-vg-build*>meshopt 0.7.0>float-cmp 0.10.0)
- **flume x2**: 0.11.1 (by nokhwa 0.10.11; e.g. mocap_capture*>nokhwa 0.10.11>flume 0.11.1) | 0.12.0 (by gpui-ce*, pulsar_lsp*, sqlx-sqlite 0.9.0; e.g. pulsar_lsp*>flume 0.12.0)
- **foldhash x2**: 0.1.5 (by hashbrown 0.15.5; e.g. blueprint_editor_plugin*>graphy-render-wgpu*>fontdue 0.9.4>..) | 0.2.0 (by fastbloom 0.17.0, glifo 0.2.0, hashbrown 0.16.1, hashbrown 0.17.1, +1; e.g. pulsar_engine*>wgpu 30.0.1>hashbrown 0.17.1>..)
- **font-types x2**: 0.11.3 (by read-fonts 0.37.0; e.g. pulsar_engine*>gpui-ce*>cosmic-text 0.18.2>..) | 0.12.4 (by epaint 0.36.2, read-fonts 0.41.0; e.g. meshes-examples*>egui 0.36.2>epaint 0.36.2>..)
- **futures-lite x2**: 1.13.0 (by gpui_util 0.2.2; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 2.6.1 (by async-executor 1.14.1, async-fs 2.2.0, async-global-executor 2.4.1, async-io 2.6.0, +8; e.g. pulsar_engine*>smol 2.0.2>futures-lite 2.6.1)
- **git2 x2**: 0.20.4 (by gpui_util 0.2.2; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 0.21.0 (by ui_git_manager*; e.g. ui_git_manager*>git2 0.21.0)
- **harfrust x2**: 0.5.2 (by cosmic-text 0.18.2; e.g. pulsar_engine*>gpui-ce*>cosmic-text 0.18.2>..) | 0.12.0 (by epaint 0.36.2; e.g. meshes-examples*>egui 0.36.2>epaint 0.36.2>..)
- **hashlink x2**: 0.9.1 (by rusqlite 0.32.1; e.g. engine_fs*>rusqlite 0.32.1>hashlink 0.9.1) | 0.11.1 (by sqlx-core 0.9.0, yaml-rust2 0.11.1; e.g. pulsar-relay*>config 0.15.25>yaml-rust2 0.11.1>..)
- **hkdf x2**: 0.12.4 (by elliptic-curve 0.13.8; e.g. pulsar-relay*>jsonwebtoken 10.4.0>p256 0.13.2>..) | 0.13.0 (by sqlx-postgres 0.9.0; e.g. pulsar-relay*>sqlx 0.9.0>sqlx-postgres 0.9.0>..)
- **hmac x2**: 0.12.1 (by hkdf 0.12.4, jsonwebtoken 10.4.0, rfc6979 0.4.0; e.g. pulsar-relay*>jsonwebtoken 10.4.0>hmac 0.12.1) | 0.13.0 (by hkdf 0.13.0, postgres-protocol 0.6.12, sqlx-postgres 0.9.0; e.g. pulsar-relay*>sqlx 0.9.0>sqlx-postgres 0.9.0>..)
- **html5ever x2**: 0.27.0 (by markup5ever_rcdom 0.3.0, scraper 0.20.0, wgpui-base*; e.g. wgpui-base*>html5ever 0.27.0) | 0.39.0 (by markup5ever_rcdom 0.39.0+unofficial, ui*; e.g. ui*>html5ever 0.39.0)
- **json5 x2**: 0.4.1 (by config 0.15.25; e.g. pulsar-relay*>config 0.15.25>json5 0.4.1) | 1.3.1 (by mermaid-rs-renderer 0.2.2; e.g. ui*>mermaid-rs-renderer 0.2.2>json5 1.3.1)
- **libloading x2**: 0.8.9 (by ash 0.38.0+1.3.281, clang-sys 1.9.1, khronos-egl 6.0.0, wgpu-hal 30.0.1; e.g. pulsar_engine*>wgpu 30.0.1>wgpu-hal 30.0.1>..) | 0.9.0 (by engine_backend*, helio-xr*, openxr 0.21.1, plugin_manager*, +2; e.g. pulsar_script_vm*>libloading 0.9.0)
- **markup5ever x2**: 0.12.1 (by html5ever 0.27.0, markup5ever_rcdom 0.3.0, xml5ever 0.18.1; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1) | 0.39.0 (by html5ever 0.39.0, markup5ever_rcdom 0.39.0+unofficial, xml5ever 0.39.0; e.g. ui*>html5ever 0.39.0>markup5ever 0.39.0)
- **markup5ever_rcdom x2**: 0.3.0 (by wgpui-base*; e.g. wgpui-base*>markup5ever_rcdom 0.3.0) | 0.39.0+unofficial (by ui*; e.g. ui*>markup5ever_rcdom 0.39.0+unofficial)
- **mockaco-core x2**: 0.1.0 [git] (by mockaco-diff 0.1.0, mockaco-gpui 0.1.0, mockaco-language 0.1.0, mockaco-renderer 0.1.0, +1; e.g. ui_git_manager*>mockaco-core 0.1.0) | 0.1.0 [git] (by mockaco-diff 0.1.0, mockaco-gpui 0.1.0, mockaco-language 0.1.0, mockaco-renderer 0.1.0, +1; e.g. script_editor_plugin*>mockaco-core 0.1.0)
- **mockaco-diff x2**: 0.1.0 [git] (by mockaco-gpui 0.1.0; e.g. ui_multiplayer*>mockaco-gpui 0.1.0>mockaco-diff 0.1.0) | 0.1.0 [git] (by mockaco-gpui 0.1.0, script_editor_plugin*; e.g. script_editor_plugin*>mockaco-diff 0.1.0)
- **mockaco-gpui x2**: 0.1.0 [git] (by ui_git_manager*, ui_multiplayer*; e.g. ui_multiplayer*>mockaco-gpui 0.1.0) | 0.1.0 [git] (by script_editor_plugin*; e.g. script_editor_plugin*>mockaco-gpui 0.1.0)
- **mockaco-language x2**: 0.1.0 [git] (by mockaco-gpui 0.1.0, ui_git_manager*, ui_multiplayer*; e.g. ui_multiplayer*>mockaco-language 0.1.0) | 0.1.0 [git] (by mockaco-gpui 0.1.0, script_editor_plugin*; e.g. script_editor_plugin*>mockaco-language 0.1.0)
- **mockaco-renderer x2**: 0.1.0 [git] (by mockaco-gpui 0.1.0; e.g. ui_multiplayer*>mockaco-gpui 0.1.0>mockaco-renderer 0.1.0) | 0.1.0 [git] (by mockaco-gpui 0.1.0, script_editor_plugin*; e.g. script_editor_plugin*>mockaco-renderer 0.1.0)
- **nalgebra-macros x2**: 0.2.2 (by nalgebra 0.32.6, nalgebra 0.33.3; e.g. mocap_capture*>nalgebra 0.32.6>nalgebra-macros 0.2.2) | 0.3.0 (by nalgebra 0.35.0; e.g. physics_plugin*>rapier3d 0.35.3>nalgebra 0.35.0>..)
- **notify x2**: 6.1.1 (by ui*; e.g. ui*>notify 6.1.1) | 8.2.0 (by engine_fs*, helio-core*, ui_file_manager*, ui_log_viewer*; e.g. engine_fs*>notify 8.2.0)
- **num-bigint x2**: 0.4.8 (by boa_ast 0.21.1, boa_engine 0.21.1, boa_parser 0.21.1, der-parser 10.0.0, +3; e.g. ui*>mathjax-svg-rs 0.4.0>boa_engine 0.21.1>..) | 0.5.1 (by oxc_ecmascript 0.152.0, oxc_parser 0.152.0; e.g. pulsar_script_ts*>oxc_parser 0.152.0>num-bigint 0.5.1)
- **pem-rfc7468 x2**: 0.7.0 (by der 0.7.10, elliptic-curve 0.13.8; e.g. pulsar-relay*>jsonwebtoken 10.4.0>p256 0.13.2>..) | 1.0.0 (by der 0.8.3; e.g. engine_fs*>ureq 3.4.0>der 0.8.3>..)
- **profiling x2**: 0.1.0 [path] (by animation_component*, engine_backend*, helio-core*, helio-voxel-examples*, +23; e.g. profiling*) | 1.0.18 (by egui 0.36.2, egui-wgpu 0.36.2, egui-winit 0.36.2, epaint 0.36.2, +7; e.g. pulsar_engine*>profiling 1.0.18)
- **rand_chacha x2**: 0.3.1 (by rand 0.8.8; e.g. engine_backend*>PebbleVault 0.7.0>rand 0.8.8>..) | 0.9.0 (by rand 0.9.5; e.g. pulsar-relay*>opentelemetry_sdk 0.33.1>rand 0.9.5>..)
- **read-fonts x2**: 0.37.0 (by harfrust 0.5.2, skrifa 0.40.0; e.g. pulsar_engine*>gpui-ce*>cosmic-text 0.18.2>..) | 0.41.0 (by harfrust 0.12.0, skrifa 0.44.0; e.g. meshes-examples*>egui 0.36.2>epaint 0.36.2>..)
- **reqwest x2**: 0.12.28 (by tool_registry_builtin 0.1.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>reqwest 0.12.28) | 0.13.4 (by agent_provider_anthropic*, agent_provider_docker_model_runner*, agent_provider_gemini*, agent_provider_github_copilot*, +15; e.g. pulsar_auth*>reqwest 0.13.4)
- **ropey x2**: 1.6.1 (by mockaco-core 0.1.0; e.g. script_editor_plugin*>mockaco-core 0.1.0>ropey 1.6.1) | 2.0.0-beta.1 (by pulsar_lsp*, ui*, wgpui-base*; e.g. ui*>ropey 2.0.0-beta.1)
- **rstar x2**: 0.12.2 (by PebbleVault 0.7.0, graphy-render*, parry3d 0.19.0; e.g. blueprint_editor_plugin*>graphy-render*>rstar 0.12.2) | 0.13.0 (by parry3d 0.30.2; e.g. physics_plugin*>rapier3d 0.35.3>parry3d 0.30.2>..)
- **rustc-hash x2**: 1.1.0 (by bindgen 0.69.5, naga 30.0.1, naga-types 30.0.1, parry3d 0.13.8, +2; e.g. materials_graph_editor*>naga 30.0.1>rustc-hash 1.1.0) | 2.1.3 (by boa_ast 0.21.1, boa_engine 0.21.1, boa_interner 0.21.1, boa_parser 0.21.1, +14; e.g. helio-pass-voxel-planet*>rustc-hash 2.1.3)
- **rustix x2**: 0.38.44 (by which 4.4.2; e.g. engine_backend*>PebbleVault 0.7.0>bindgen 0.69.5>..) | 1.1.4 (by async-io 2.6.0; e.g. pulsar_engine*>smol 2.0.2>async-io 2.6.0>..)
- **safe_arch x2**: 0.7.4 (by wide 0.7.33; e.g. mocap_capture*>nalgebra 0.32.6>simba 0.8.1>..) | 1.2.0 (by wide 1.6.1; e.g. physics_plugin*>rapier3d 0.35.3>wide 1.6.1>..)
- **serde_spanned x2**: 0.6.9 (by toml 0.8.23, toml_edit 0.22.27; e.g. pulsar-config*>toml 0.8.23>serde_spanned 0.6.9) | 1.1.1 (by toml 1.1.4+spec-1.1.0; e.g. pulsar_engine*>toml 1.1.4+spec-1.1.0>serde_spanned 1.1.1)
- **sha1 x2**: 0.10.7 (by axum 0.8.9, mysql_common 0.35.5, tungstenite 0.29.0; e.g. pulsar-relay*>axum 0.8.9>sha1 0.10.7) | 0.11.0 (by sqlx-mysql 0.9.0, tungstenite 0.30.0; e.g. friends_engine*>async-tungstenite 0.35.0>tungstenite 0.30.0>..)
- **sha2 x2**: 0.10.9 (by ed25519-dalek 2.2.0, gpui_http_client 0.2.2, jsonwebtoken 10.4.0, mysql_common 0.35.5, +4; e.g. pulsar-relay*>jsonwebtoken 10.4.0>sha2 0.10.9) | 0.11.0 (by ed25519-dalek 3.0.0, friends_engine*, mocap_capture*, plugin_manager*, +13; e.g. plugin_manager*>sha2 0.11.0)
- **shlex x2**: 1.3.0 (by bindgen 0.69.5, gpui_util 0.2.2; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..) | 2.0.1 (by cc 1.2.67; e.g. ui*>tree-sitter 0.25.10>cc 1.2.67>..)
- **signature x2**: 2.2.0 (by ecdsa 0.16.9, ed25519 2.2.3, jsonwebtoken 10.4.0, rsa 0.9.10; e.g. pulsar-relay*>jsonwebtoken 10.4.0>signature 2.2.0) | 3.0.0 (by ed25519 3.0.0, ed25519-dalek 3.0.0; e.g. pulsar-relay*>ed25519-dalek 3.0.0>signature 3.0.0)
- **similar x2**: 2.7.0 (by script_editor_plugin*; e.g. script_editor_plugin*>similar 2.7.0) | 3.2.0 (by mockaco-diff 0.1.0, ui_git_manager*, ui_multiplayer*, ui_problems*; e.g. ui_multiplayer*>similar 3.2.0)
- **siphasher x2**: 0.3.11 (by phf_shared 0.10.0; e.g. agent_chat_tools*>tool_registry_builtin 0.1.0>scraper 0.20.0>..) | 1.0.3 (by fastbloom 0.17.0, phf_shared 0.11.3, phf_shared 0.13.1, phf_shared 0.14.0, +3; e.g. ui_common*>rust-i18n-support 4.2.1>siphasher 1.0.3)
- **skrifa x2**: 0.40.0 (by cosmic-text 0.18.2; e.g. pulsar_engine*>gpui-ce*>cosmic-text 0.18.2>..) | 0.44.0 (by epaint 0.36.2, glifo 0.2.0, swash 0.2.10; e.g. meshes-examples*>egui 0.36.2>epaint 0.36.2>..)
- **smol_str x2**: 0.2.2 (by winit 0.30.13; e.g. pulsar_game*>winit 0.30.13>smol_str 0.2.2) | 0.3.6 (by cosmic-text 0.18.2, gpui-ce*; e.g. pulsar_engine*>gpui-ce*>smol_str 0.3.6)
- **socket2 x2**: 0.5.10 (by mysql 26.0.1, pulsar_scenedb*; e.g. pulsar_scenedb*>socket2 0.5.10) | 0.6.5 (by hyper-util 0.1.20, pulsar-relay*, quinn 0.11.11, quinn-udp 0.5.15, +2; e.g. pulsar-relay*>socket2 0.6.5)
- **solid-fbx x2**: 0.1.0 [git] (by helio-asset-compat*; e.g. helio-asset-compat*>solid-fbx 0.1.0) | 0.1.0 [git] (by asset_viewer_plugin*; e.g. asset_viewer_plugin*>solid-fbx 0.1.0)
- **solid-rs x2**: 0.1.0 [git] (by helio-asset-compat*, solid-fbx 0.1.0, solid-gltf 0.1.0, solid-obj 0.1.0, +2; e.g. helio-asset-compat*>solid-rs 0.1.0) | 0.1.0 [git] (by asset_viewer_plugin*, solid-fbx 0.1.0; e.g. asset_viewer_plugin*>solid-rs 0.1.0)
- **spin x2**: 0.9.9 (by flume 0.11.1, flume 0.12.0, lazy_static 1.5.0; e.g. ui_types_common*>lazy_static 1.5.0>spin 0.9.9) | 0.10.1 (by gpui-ce*; e.g. pulsar_engine*>gpui-ce*>spin 0.10.1)
- **string_cache x2**: 0.8.9 (by markup5ever 0.12.1; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.9.0 (by web_atoms 0.2.6; e.g. ui*>html5ever 0.39.0>markup5ever 0.39.0>..)
- **string_cache_codegen x2**: 0.5.4 (by markup5ever 0.12.1; e.g. wgpui-base*>html5ever 0.27.0>markup5ever 0.12.1>..) | 0.6.1 (by web_atoms 0.2.6; e.g. ui*>html5ever 0.39.0>markup5ever 0.39.0>..)
- **strum x2**: 0.26.3 (by ratatui 0.29.0; e.g. helio-web-demos*>ratatui 0.29.0>strum 0.26.3) | 0.28.0 (by gpui-ce*; e.g. pulsar_engine*>gpui-ce*>strum 0.28.0)
- **strum_macros x2**: 0.26.4 (by strum 0.26.3; e.g. helio-web-demos*>ratatui 0.29.0>strum 0.26.3>..) | 0.28.0 (by strum 0.28.0; e.g. pulsar_engine*>gpui-ce*>strum 0.28.0>..)
- **tendril x2**: 0.4.3 (by gpui_util 0.2.2, markup5ever 0.12.1, markup5ever_rcdom 0.3.0, scraper 0.20.0; e.g. wgpui-base*>markup5ever_rcdom 0.3.0>tendril 0.4.3) | 0.5.1 (by markup5ever 0.39.0, markup5ever_rcdom 0.39.0+unofficial; e.g. ui*>markup5ever_rcdom 0.39.0+unofficial>tendril 0.5.1)
- **thiserror x2**: 1.0.69 (by async_zip 0.0.17, graphy*, nebula-ao*, nebula-audio*, +12; e.g. psgc*>thiserror 1.0.69) | 2.0.20 (by asn1-rs 0.7.2, boa_engine 0.21.1, defmt-parser 1.0.0, egui-wgpu 0.36.2, +65; e.g. pulsar_engine*>thiserror 2.0.20)
- **thiserror-impl x2**: 1.0.69 (by thiserror 1.0.69; e.g. psgc*>thiserror 1.0.69>thiserror-impl 1.0.69) | 2.0.20 (by thiserror 2.0.20; e.g. pulsar_engine*>thiserror 2.0.20>thiserror-impl 2.0.20)
- **toml x2**: 0.8.23 (by pulsar-config*, rust-i18n-support 4.2.1, ui*; e.g. pulsar-config*>toml 0.8.23) | 1.1.4+spec-1.1.0 (by config 0.15.25, engine_backend*, engine_state*, plugin_builder*, +8; e.g. pulsar_engine*>toml 1.1.4+spec-1.1.0)
- **toml_datetime x2**: 0.6.11 (by toml 0.8.23, toml_edit 0.22.27; e.g. pulsar-config*>toml 0.8.23>toml_datetime 0.6.11) | 1.1.1+spec-1.1.0 (by toml 1.1.4+spec-1.1.0, toml_edit 0.25.13+spec-1.1.0; e.g. pulsar_engine*>toml 1.1.4+spec-1.1.0>toml_datetime 1.1.1+spec-1.1.0)
- **toml_edit x2**: 0.22.27 (by toml 0.8.23; e.g. pulsar-config*>toml 0.8.23>toml_edit 0.22.27) | 0.25.13+spec-1.1.0 (by proc-macro-crate 3.5.0; e.g. ui*>mathjax-svg-rs 0.4.0>include-zstd 0.1.3>..)
- **tower-http x2**: 0.6.11 (by reqwest 0.12.28, reqwest 0.13.4; e.g. pulsar_auth*>reqwest 0.13.4>tower-http 0.6.11) | 0.7.0 (by pulsar-relay*; e.g. pulsar-relay*>tower-http 0.7.0)
- **tungstenite x2**: 0.29.0 (by tokio-tungstenite 0.29.0; e.g. pulsar-relay*>axum 0.8.9>tokio-tungstenite 0.29.0>..) | 0.30.0 (by async-tungstenite 0.35.0; e.g. friends_engine*>async-tungstenite 0.35.0>tungstenite 0.30.0)
- **unicode-width x2**: 0.1.14 (by unicode-truncate 1.1.0; e.g. helio-web-demos*>ratatui 0.29.0>unicode-truncate 1.1.0>..) | 0.2.0 (by annotate-snippets 0.12.5, codespan-reporting 0.13.1, console 0.15.11, getopts 0.2.24, +4; e.g. helio-web-demos*>ratatui 0.29.0>unicode-width 0.2.0)
- **ureq x2**: 2.12.1 (by ui_level_editor*; e.g. ui_level_editor*>ureq 2.12.1) | 3.4.0 (by engine_fs*, ort 2.0.0-rc.15, ort-sys 2.0.0-rc.15, pulsar_language*; e.g. engine_fs*>ureq 3.4.0)
- **webpki-roots x2**: 0.26.11 (by pulsar-relay*, ureq 2.12.1; e.g. pulsar-relay*>webpki-roots 0.26.11) | 1.0.12 (by hyper-rustls 0.27.9, reqwest 0.12.28, sqlx-core 0.9.0, ureq 3.4.0, +1; e.g. engine_fs*>ureq 3.4.0>webpki-roots 1.0.12)
- **which x2**: 4.4.2 (by bindgen 0.69.5; e.g. engine_backend*>PebbleVault 0.7.0>bindgen 0.69.5>..) | 6.0.3 (by gpui_util 0.2.2; e.g. pulsar_engine*>gpui-ce*>gpui_util 0.2.2>..)
- **wide x2**: 0.7.33 (by simba 0.8.1, simba 0.9.1; e.g. mocap_capture*>nalgebra 0.32.6>simba 0.8.1>..) | 1.6.1 (by rapier3d 0.35.3, simba 0.10.2; e.g. physics_plugin*>rapier3d 0.35.3>wide 1.6.1)
- **windows-implement x2**: 0.59.0 (by windows-core 0.59.0; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.60.2 (by windows-core 0.62.2; e.g. pulsar_engine*>windows 0.62.2>windows-core 0.62.2>..)
- **windows-link x2**: 0.1.3 (by windows-result 0.3.4, windows-strings 0.3.1; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.2.1 (by backtrace 0.3.76, chrono 0.4.45, libloading 0.8.9, libloading 0.9.0, +9; e.g. parking_lot_core*>windows-link 0.2.1)
- **windows-registry x2**: 0.4.0 (by zed-reqwest 0.12.15-zed; e.g. pulsar_engine*>gpui-ce*>gpui_http_client 0.2.2>..) | 0.6.1 (by hyper-util 0.1.20; e.g. pulsar_auth*>reqwest 0.13.4>hyper-util 0.1.20>..)
- **windows-strings x2**: 0.3.1 (by windows-core 0.59.0, windows-registry 0.4.0; e.g. pulsar_engine*>gpui-ce*>clipboard-rs 0.3.5>..) | 0.5.1 (by windows-core 0.62.2, windows-registry 0.6.1; e.g. pulsar_engine*>windows 0.62.2>windows-core 0.62.2>..)
- **winnow x2**: 0.7.15 (by toml_edit 0.22.27; e.g. pulsar-config*>toml 0.8.23>toml_edit 0.22.27>..) | 1.0.4 (by config 0.15.25, toml 1.1.4+spec-1.1.0, toml_edit 0.25.13+spec-1.1.0, toml_parser 1.1.3+spec-1.1.0; e.g. pulsar_engine*>toml 1.1.4+spec-1.1.0>winnow 1.0.4)
- **winreg x2**: 0.52.0 (by sysuri 0.4.0; e.g. pulsar_engine*>sysuri 0.4.0>winreg 0.52.0) | 0.56.0 (by ui_log_viewer*; e.g. ui_log_viewer*>winreg 0.56.0)
- **xml5ever x2**: 0.18.1 (by markup5ever_rcdom 0.3.0; e.g. wgpui-base*>markup5ever_rcdom 0.3.0>xml5ever 0.18.1) | 0.39.0 (by markup5ever_rcdom 0.39.0+unofficial; e.g. ui*>markup5ever_rcdom 0.39.0+unofficial>xml5ever 0.39.0)
- **zip x2**: 2.4.2 (by pulsar_image_format 0.1.0; e.g. plugin_matter*>pulsar_image_format 0.1.0>zip 2.4.2) | 9.5.0 (by rust_plugin*, sky_editor*, solid-usd 0.1.0; e.g. rust_plugin*>zip 9.5.0)


## Appendix D: the most expensive direct dependencies

Packages that disappear if this one dependency is removed from the whole workspace (Windows resolve; overlapping
groups are not additive; `pulsar_workspace_hack` is only a pin listing and is not counted as a user).

| packages freed if removed (whole workspace, Windows) | direct dependency | declared by |
|---:|---|---|
| 34 | `PebbleVault 0.7.0` | engine_backend |
| 27 | `mathjax-svg-rs 0.4.0` | ui |
| 26 | `jsonwebtoken 10.4.0` | pulsar-relay |
| 24 | `boa_engine 0.21.1` |  |
| 18 | `config 0.15.25` | pulsar-relay |
| 17 | `wgpu 30.0.1` | pulsar_scenedb, blueprint_editor_plugin, engine_backend, engine_state, helio_component, pulsar_scene_model (+104) |
| 17 | `tool_registry_builtin 0.1.0` | agent_chat_tools |
| 15 | `rust_decimal 1.42.1` | ui |
| 14 | `sqlx 0.9.0` | pulsar-relay |
| 13 | `log 0.4.34` | blueprint_editor_plugin, script_editor_plugin, editor_table_plugin, render_bps_plugin, asset_viewer_plugin, helio-mats (+55) |
| 13 | `ratatui 0.29.0` | helio-web-demos |
| 12 | `image 0.25.10` | helio_component, ui, ui_common, ui_shell, plugin_matter, asset_viewer_plugin (+36) |
| 12 | `rapier3d 0.35.3` | physics_plugin |
| 12 | `gpui_util 0.2.2` |  |
| 12 | `value-bag 1.13.2` |  |
| 12 | `wgpu-hal 30.0.1` |  |
| 11 | `gpui_http_client 0.2.2` |  |
| 10 | `ureq 3.4.0` | engine_fs, pulsar_language |
| 10 | `rodio 0.21.1` | ui_build |
| 10 | `opentelemetry-otlp 0.33.1` | pulsar-relay |
| 9 | `cosmic-text 0.18.2` |  |
| 9 | `usvg 0.47.0` |  |
| 9 | `rcgen 0.14.9` | pulsar-relay |
| 8 | `rust-i18n-support 4.2.1` | ui_common, script_editor_plugin, plugin_matter, asset_viewer_plugin |
| 8 | `async-compression 0.4.43` |  |
| 7 | `tracing-subscriber 0.3.23` | editor_task_queue, pulsar_engine, pulsar_package, pulsar-relay |
| 7 | `nokhwa 0.10.11` | mocap_capture |
| 7 | `digest 0.11.3` |  |
| 7 | `exr 1.74.2` |  |
| 7 | `zed-async-tar 0.5.0-zed` |  |
| 7 | `env_logger 0.11.11` | helio-voxel-examples, particles-examples, sprites-examples, decals-examples, lens-examples, portals-examples (+10) |
| 6 | `schemars 1.2.2` | blueprint_editor_plugin, ui_types_common, ui, wgpui-base, plugin_editor_api, script_editor_plugin (+4) |
| 6 | `toml 0.8.23` | pulsar-config, ui |
| 6 | `async-std 1.13.2` |  |
| 6 | `compression-codecs 0.4.38` |  |
| 6 | `lyon 1.0.19` |  |
| 6 | `stacksafe 1.0.3` |  |
| 6 | `time 0.3.55` |  |
| 6 | `axum 0.8.9` | pulsar-relay |
| 6 | `quinn 0.11.11` | pulsar-relay |
| 6 | `salva3d 0.9.0` | meshes-examples |
| 5 | `windows 0.62.2` | blueprint_editor_plugin, engine_backend, ui, wgpui-base, pulsar_engine, ui_log_viewer (+1) |
| 5 | `ort 2.0.0-rc.15` | mocap_capture |
| 5 | `markup5ever 0.39.0` |  |
| 5 | `rapier3d 0.24.0` | meshes-examples |


## Appendix E: how to reproduce

All commands are read-only against the real tree (`--locked --offline`) and write to a scratch target directory.

```
cargo metadata --locked --offline --format-version 1 [--filter-platform x86_64-pc-windows-msvc] > meta.json
CARGO_TARGET_DIR=C:/tb_deps RUSTFLAGS="-W unused-crate-dependencies" \
  cargo check --workspace --all-targets --keep-going --locked --offline --message-format json > check.json
cargo tree -p <root> -e normal,build --prefix none --target x86_64-pc-windows-msvc --locked --offline | sed 's/ (\*)//;s/ (proc-macro)//' | sort -u | wc -l
```

The aggregation scripts (Node, no Python on this machine) were: dependency closure per root, edge-removal cost
("packages freed if this dependency edge is cut"), chain to the introducing workspace crate for every duplicate, the
lint-JSON classifier (units per package vs warnings per extern crate, joined with a per-package text search), and an
experiment runner that copies the manifests of the repository to `C:\tb_scratch`, applies an edit, runs `cargo metadata
--offline` and diffs the resulting lock. They were kept in the session scratch directory only; promote the classifier and
the experiment runner to `tools/dep_report` if the CI guards above are adopted.
