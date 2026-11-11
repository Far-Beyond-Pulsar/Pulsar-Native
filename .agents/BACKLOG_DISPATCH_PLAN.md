# Backlog dispatch plan (draft, 2026-11-08)

Scope: Pulsar-Native#1604 #1600 #1603 #1599 #1605 #1608, Plugin_SKM#13,
Plugin_Blueprints#42, Helio#393 #390 #391, plus the roadmap items R0/R1.
All workers run as **Sonnet 5.5** (`model: "sonnet"`). The coordinator (this
session) writes briefs, checks diffs, and re-pins submodules. Workers never
merge.

## Rules every brief carries

1. Read `AGENTS.md`, then only the `.agents/*.md` files named in the brief.
2. Rendering work (Helio, `plugins/vendor/*` render side, WGSL, SceneDB GPU rows,
   presets, kind editor tools) loads the `helio-architecture` skill first and
   ends the report with: **Design, AAA comparison, Deleted, Verified**.
3. One owner per concept; the frame keeps only what other kinds read; delete
   what you replace (no compat shims). Shrink-only lists (`kind-names.txt`, the
   planned kind-name budget) only get lines removed.
4. Write scope is stated per task. Two writers never share files. Report which
   files were touched.
5. Branch `backlog/<task-id>`; commit; open a **draft** PR (CI skips drafts).
   Submodule changes go in the submodule's own repo first (dependency-first
   merge order, `.agents/CRATES.md` "Submodule pins").
6. Cost: run `cargo check -p <crate>` / `cargo test -p <crate>` for touched
   crates only; no workspace-wide builds unless the brief says so. GPU tests
   are excluded on CI; run them locally only if named. Chunks suite needs
   `--release`.
7. Update the matching `.agents/*.md` doc in the same change. TODO comments need
   an issue reference (`todo_check`).
8. Final report: what changed, what is verified (with command output), what is
   not done. Never claim a check you did not run.

## Preconditions to settle before dispatch

- `gh` is not installed on this machine (issues were read through the REST API).
  Workers need `gh` (or a token) to open PRs. Install it, or workers stop at a
  pushed branch and I open the PRs.
- **#1599 VG** says the final design is not decided ("the user said so"). Needs
  a design from you before any rework worker runs; Wave 1 only diagnoses.
- **R0/R1** (rendering roadmap: measurement, ray scene) have no issue text I
  could find; point me at the roadmap doc (likely Helio `docs/`), or I scope them
  from `.agents/ROADMAP.md` in Wave 5.
- The `std::fs` migration (148 files) wants a **quiet window**: no other writer
  active. It is scheduled as its own wave.

## Waves

Tracks A (Pulsar core/plugin API), B (rendering), C (leaf fixes) overlap
only where the table's "Conflicts" column says otherwise. Within a wave,
tasks marked `‖` run in parallel.

### Wave 0 — read-only audits and small independent fixes

| ID | Task | Write scope | Conflicts |
|---|---|---|---|
| A0 ‖ | **#1608 Phase 0 audit**: registration matrix, `inventory::iter` classification, plugin-to-plugin dependency/contract inventory, crate/layer map (catalog in `plugin_manager` or a host crate). Output: `.agents/PLUGIN_API_REFACTOR_PLAN.md` (replaces `PLUGIN_API_V2*.md`) | `.agents/` only | none |
| C1 ‖ | **#1604 Linux link**: find the C++ dep missing its runtime (`cargo tree`, build scripts of `-sys` crates); add the `stdc++` link in the owning `build.rs` or `pulsar_game`. Cannot reproduce on Windows, so verify by pushing a draft PR and `gh workflow run CI --ref <branch> -f full=true` or the Linux lane | `pulsar_game` build files, CI yaml | A-track edits to `pulsar_game` (small, rebases fine) |
| C2 ‖ | **#1599 VG diagnosis** (read-only + notes): run `virtual_geometry_dense_acceptance`, look at page-residency (107) and error-vs-tau (109) views, answer why `dense_tiles` is 0; fold in #1417 (visible quality transitions) analysis. Output: findings in the issue/PR notes, no rework | none (notes only) | none |
| B0 ‖ | **Helio#393 item 3, lighting-model registration**: replace `helio_frame_api::LightingModel`, lights plugin registers `deferred`/`light-field`/`forward`, presets pick by id, HLFS leaves `helio-presets` | Helio `frame-api`, `presets`; `plugins/vendor/lights` | blocks B1..B4 |
| P1 ‖ | **Plugin_Blueprints#42**: replace the editor's `PrefabAsset` with `pulsar_class::prefab::PrefabAsset`; drop the raw `prefab.json` read for animation events | `plugins/vendor/blueprint_editor` | A-track Blueprint-node work (later) |

### Wave 1 — plugin root crates and catalog (serial, Track A) + rendering ownership

A1 → A2 → A3 are serial (each builds on the previous).

| ID | Task | Write scope |
|---|---|---|
| A1 | **#1608 Phase 1**: define root-crate shape and `build(&mut PluginBuilder)` with runtime/editor phases and `editor` feature; document in `.agents/PLUGIN_SYSTEM.md` with one commented example; migrate one editor-only plugin (suggest `table_editor`, which also brings it into the workspace) and one object-kind plugin (suggest `decals`, small); delete the scattered `inventory::submit!` sites; update descriptor format and `installed_plugins` generator | `pulsar_plugins`, `plugin_editor_api`, `plugins/installed`, the two plugins |
| A2 | **#1608 Phase 1b**: `FeatureRegistrar` records feature id on nodes; `Contribution` summary trait on every registration type; host catalog; unknown origins reported with provenance | `pulsar_plugins`, `plugin_manager`/host crate, render registry |
| A3 | **#1605 kernel items, one PR each, serial**: (a) physics out of core (`pulsar_physics` into `physics_plugin`/api; rapier feature out of `engine_backend`); (b) core crates name no plugin crate (`scene_inventory`, `pulsar_game`, `pulsar_package`); (c) asset categories registered by plugins; (d) one asset-reference resolver; (e) ledger rows per plugin; (f) Blueprint nodes shipped by owning plugins (`pulsar_std`); (g) layer gate (core never depends on `plugins/vendor`, shrink-only kind-name budget) | per item, stated in each brief |
| B1 ‖ | **Helio#393 item 2, sky split**: frame keeps medium, sky table, suns; clouds and stars into the sky plugin's own block | Helio frame/lighting WGSL; `plugins/vendor/sky` |
| P2 ‖ | **Quick leaf, voxel/foliage part of #1600**: `foliage_on_voxels` (no blades near eye) and `voxel_pass_graph` chunk settle (run chunks suite `--release`) | `plugins/vendor/voxel`, `foliage` |

(A3 can start items (a)–(b) while B-track runs; all B tasks avoid core crates.)

### Wave 2 — rendering ownership, continued (Track B, serial) + SKM

| ID | Task |
|---|---|
| B2 | **Helio#393 item 5**: lens settings leave post-settings ABI; blendable overrides contract; record migration for saved post volumes |
| B3 | **Helio#393 item 4**: material-cost profiler moves to the materials plugin as a debug view |
| B4 | **Helio#393 item 1**: `pulsar.bake` (Plugin_Bake): new repo, G-buffer extension targets, lighting-input contracts. Largest; may need splitting into 3 workers (repo + move, G-buffer extension targets, contracts) |
| S1 ‖ | **#1603 night-sky assets** as project assets named by the sky component (after B1; same plugin) |
| S2 ‖ | **#1600 clouds_cost** perf budget (after B1; clouds now live in the sky plugin) |
| K1 ‖ | **Plugin_SKM#13 (non-physics parts)**: animator drives several meshes; wire `reload_animation_dependencies` through an asset-change hook (touches `engine_backend` in one place); sequence-editor reload on skeleton/mesh file change (#1333, uses file events) |

### Wave 3 — quiet window: file IO + VFS (Track A)

All other writers paused (merge or park every open branch first).

| ID | Task |
|---|---|
| A4 | **Done (p21, `PLUGIN_API_REFACTOR_PLAN.md` section 13).** **#1608 traits, step 1**: read-only mounts in `engine_fs::virtual_fs`, `plugins/<id>/` mount, file drawer read-only state, `pulsar` mount replaces `builtin_traits::ensure`/`sync_one`; an on-disk `plugins/` folder is reported |
| A5 | **Done (p21, `PLUGIN_API_REFACTOR_PLAN.md` section 13).** **All file IO through `engine_fs`**: complete missing ops, migrate the 663 `std::fs` lines in 148 files, add `clippy.toml` `disallowed-methods` with a shrink-only allow list. Split mechanically by crate group across workers only if no two groups share a crate |

### Wave 4 — traits and catalog UI (Track A, mostly serial) + renderer views

| ID | Task |
|---|---|
| A6 | **Done (p21, `PLUGIN_API_REFACTOR_PLAN.md` section 13).** **Traits, step 2**: role registry replacing `ROLES`/`WorldSettingsData` fields/`EngineRole` arms; World Settings picker via extension points; role lifecycle events on the Gamma hub |
| A7 | **Done (p21, `PLUGIN_API_REFACTOR_PLAN.md` section 13).** `TraitImplementerSource` (Blueprints first, native second), one trait index, native bindings onto `ComponentMethodRegistration` checked by `trait_conformance`, dispatcher for both, plugin trait namespacing + `contractVersion` |
| A8 | **Done (p22, `PLUGIN_API_REFACTOR_PLAN.md` section 14).** **#1608 Phase 2 and 3**: Extensions viewer on the catalog (Renderer tab goes); `provides`/`requires` contract ids with named activation errors; trait/role/source entries; fixture-plugin proof (static, contract, renderer, disabled-plugin, trait+role) |
| K2 | **Plugin_SKM#13 physics part**: pose without an animator (ragdoll, folded Plugin_Physics#8) after A3(a) |
| V1 | **#1599 VG rework**, once you have decided the final design (see preconditions) |
| B5 | **Helio#390**: composed view graphs for reflections and portals (after B0..B4 and V1: touches frame contracts and the meshes plugin) |

### Wave 5 — finish

| ID | Task |
|---|---|
| A9 | **Done (p22, section 14; `Plugin_Character` repository is the user's decision).** **#1608 Phase 5** (component class API, `requires`, facade, systems, `UnresolvedComponent`, `pulsar.character` proof) |
| A10 | **Done except bundling (p22, section 14: bundling waits on a Helio runtime attach).** **#1608 Phase 4** (binary plugins: fingerprints, DLL `build`, bundling, `replaces =`, no-statics test, CI) |
| B6 | **Rendering roadmap R0** (measurement) then **R1** (ray scene), scoped once the roadmap doc is pointed at |
| B7 | **Helio#391 README**, last, once the renderer has settled |

## Dependency summary

```
A0 → A1 → A2 → A3(a..g) → [quiet] A4 → A5 → A6 → A7 → A8 → A9 → A10
B0 → B1 → B2 → B3 → B4 → B5(#390, also after V1) → B6 → B7
B1 → S1, S2          A3(a) → K2          C2 → V1
P1, P2, C1, K1 any time in their wave (own scopes)
```

## Coordinator checkpoints

- After every worker: read the diff, run the named tests myself for anything
  touching shared crates, check the shrink-only lists only shrank, and for
  rendering work confirm the four-part report is present.
- Re-pin submodules dependency-first and update
  `.github/submodule-pin-exceptions.txt`.
- Do not start a wave until the previous one's PRs are merged or explicitly
  parked; A5 additionally requires zero open writers.
