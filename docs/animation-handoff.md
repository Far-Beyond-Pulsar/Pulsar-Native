# Animation system: Phase 5 hand-off

Status of the animation system after Phases 0–5 of
`docs/animation-system-implementation-plan.md`. Everything is local: nothing has been pushed,
merged or released. Performance measurement was waived by the user, so there are **no
before/after numbers**; the harness that would produce them is defined below, not run.

## 1. What was checked, and what was not

Headless checks that ran (details and results in section 7):

- Workspace compile: `cargo check --workspace`.
- Tests: `skeletal_assets`, `skeletal_animation_plugin` (the `skm_editor` plugin crate),
  `helio_component`, `engine_fs`, `engine_backend`, `pulsar_game`, `physics_plugin`,
  `blueprint_compiler`, `asset_import`, `ui_common`, `ui_level_editor`; `graphy` (its own
  workspace) for the new bridge.
- Clippy on the crates touched in this phase.

**Not run, and why**

- Anything in the running app, including every screen listed in section 5. Nothing was launched.
- GPU-dependent tests were not excluded by me: the skinned-preview and timeline render tests
  run when an adapter exists and skip themselves otherwise. The nextest `ci` profile keeps its
  existing exclusions. Platform runs (macOS/Windows cross-platform job) were not triggered.
- Performance baselines and post-change tables (waived).
- A Blueprint-editor GUI pass and a real-project load of the editors.

## 2. The end-to-end flow, as tests

The plan's flow is covered by two deterministic headless tests, one per side:

| Plan step | Test | Where |
|---|---|---|
| choose component refs; component properties and typed pickers | `skeletal_mesh_component_properties_all_have_editors`, `asset_ref_properties_round_trip_through_the_runtime_registry` | `crates/editor/ui_level_editor/tests/skeletal_component_property_editors.rs` |
| edit/save/reopen a Sequence notify and state | `phase5_sequence_notify_and_state_survive_edit_save_and_reopen`, `sequence_notifies_and_states_are_edited_selected_and_saved` | `skm_editor/src/editor/hierarchy_tests.rs` |
| preview locomotion graph and 2D blend | `a_blend_space_plays_at_its_input_through_the_player_node`; runtime side in the next row | `skm_editor/src/anim_editors/skin_preview/tests.rs` |
| locomotion graph + 2D blend on a component, Montage layered over the upper body, section jump, root motion, per-component event payload, physics/GPU pose agreement | `the_character_flow_runs_end_to_end_on_the_real_component_runtime` | `helio/crates/helio-component/src/components/skeletal_phase5_tests.rs` |
| preview never fires gameplay events | `a_montage_plays_on_the_runtime_and_scrubbing_reports_nothing`, `a_preview_scrub_raises_nothing_and_leaves_active_states_alone` | skm_editor, helio (Phase 3 file) |
| every format round-trips from committed fixtures; ids stable; unknown fields survive; no reverse or self refs | `skeletal_assets/tests/phase5_roundtrip.rs` plus the Phase 1 fixture tests | skm_editor |
| reverse dependency (Used By) updates, no serialized cycles | `closure_*`, `dependency_cycles_between_assets_are_found`, `extractors_list_typed_edges_for_every_kind` (Phase 1) | skm_editor `skeletal_assets/tests/animation_assets.rs` |

Honest limits of the combination: the editor test and the runtime test are separate (they do not
share one project on disk); the component test binds the compiled graph directly instead of
resolving `AssetRef` through SceneDB (the SceneDB binding has its own Phase 2/3 tests); the
physics/GPU check compares the pose physics reads with the skin palette the component builds, not
pixels from a GPU.

## 3. Repositories, branches and merge order

Merge **dependency first**. Every branch is local and named `animation-phase4` unless stated.

| # | Repo (path) | Branch / head | Depends on | Notes |
|---|---|---|---|---|
| 1 | Pulsar-Reflection (`crates/external/pulsar-reflection`) | `animation-phase4` `cf95c79` | its Phase 1 branch `animation-phase1` | kinds (Phase 1) + optional-ref codecs |
| 2 | Graphy (`crates/third-party/graphy`, worktree `D:\GitHub\graphy-anim-p4`) | `animation-phase4` `342121d` | Graphy `main` at `6cfaa16` | **includes the user's zip-tie commits (`ab382b5`..`6cfaa16`) that are not on upstream main; they merge with, or before, this branch** |
| 3 | skm_editor (`plugins/vendor/skm_editor`) | `animation-phase4` `9997c3a` | 1, 2 | schemas, runtime, editors, tests |
| 4 | blueprint_editor (`plugins/vendor/blueprint_editor`) | `animation-phase4` `1af36d8` | 2, 3 | uses `graphy::authored`; Montage events |
| 5 | Helio (`crates/renderer/helio`, worktree `D:\GitHub\helio-anim-p4`) | `animation-phase4` `b8ba2a5` (cut from `animation-phase3-clean`) | 1, 3 | component runtime |
| 6 | Pulsar-Native | `animation-phase4` | all of the above | pins, `ui_common`, docs, tests |

Cautions that affect merging:

- **Helio history contains `f7f76dd9` "Water work"** (87 water-sim files). It is the head of
  `animation-phase2` and so is underneath every animation commit in Helio, including the
  "clean" Phase 3 branch. The animation commits must be rebased off it (or the water change
  merged separately first) before a Helio PR. The polluted `animation-phase3` branch was not
  touched.
- The shared Graphy checkout (`crates/third-party/graphy`) is on `main` with the user's
  uncommitted work. To build Pulsar-Native from this branch it must be at the pinned
  `animation-phase4` commit; until then it carries an untracked copy of `src/authored.rs`, a
  `graphy-blueprint` dependency line in `Cargo.toml` and a `pub mod authored;` line in
  `src/lib.rs` (the same change as commit `a1941e3`/`342121d`, applied so the root workspace
  builds). Check those out cleanly when switching.
- Uncommitted work of the user's was left alone: Graphy `crates/graphy-render/src/scene.rs` and
  zip-tie edits, `render_bps`, `blueprint_editor/src/editor/graph_canvas_panel.rs`, and the
  zip-tie inspector in `skm_editor/.../anim_graph_view.rs`. In `blueprint_editor/src/ui_components/properties.rs`
  I fixed the two compile errors in the zip-tie properties code after they were pasted to me;
  that edit is also uncommitted.
- `Cargo.lock` in Pulsar-Native was already modified before Phase 4; the new `skeletal_assets`
  dependency of `ui_common` and the `image`/`smallvec` dependencies of `skm_editor` will change it
  further. It was not committed.
- `.github/submodule-pin-exceptions.txt` lists the five pins above with their merge order; each
  line must be deleted when its branch lands on its upstream default branch and the pin moves
  there.

## 4. Files and crates, by repository

Counts are against each repo's `main` (Helio against its `main` including the water commit).

### skm_editor (`plugins/vendor/skm_editor`)

- **Added** (92 files): `skeletal_assets` formats and runtime (`blend_space`, `montage`,
  `anim_graph/*` incl. `catalogue`, `validate`, `builder`, `ik_rig`, `ik_retarget`, `retarget`,
  `pose_db`, `bone_mask`, `notify_schema`, `dependencies`, `picker`, `runtime/*` incl. `graph/*`,
  `montage`, `posesearch`, `blendspace`, `retarget`, `rootmotion`), 15 fixtures
  (`tests/fixtures/v1`, `v2`), the editors (`src/anim_editors/*` and `views/*`:
  `document`, `selection`, `refs`, `kinds`, `store`, per-asset models and views, `notify_edit`,
  `bone_mask_edit`, `skin_preview`, `pose_db_build`, `graph_preview`, `retarget_preview`,
  `skeleton_proj`, `skin_pane`, `bone_mask_panel`), `core/asset_index.rs`.
- **Changed**: Sequence editor (`editor/panel.rs`, `rendering/timeline/*`,
  `ui_components/properties_panel.rs`), viewport scene (`SceneInput::world_override`),
  `rendering/preview.rs` (no longer test-only).
- **Deleted**: `skeletal_assets/src/animation_sets.rs`, `src/animation_assets_editor.rs` (the
  interim typed form editor, replaced by the editors above).
- **Moved**: none.

### Pulsar-Native

- **Added**: `.agents/ANIMATION.md`, `docs/animation-phase0-audit.md`,
  `docs/animation-handoff.md`,
  `crates/editor/ui_common/src/asset_ref_editor.rs`,
  `crates/editor/ui_level_editor/tests/skeletal_component_property_editors.rs`.
- **Changed**: `AGENTS.md`, `.agents/CRATES.md`, `.agents/GRAPH_CANVAS.md`, `docs/mockups/animation-editor.html`,
  `crates/editor/ui_common` (`lib.rs`, `Cargo.toml`), `crates/core/engine_fs` (`file_index.rs`, `lib.rs`,
  `watchers.rs`: typed asset ids, reference edges, change watching), `crates/core/engine_backend`
  (`lib.rs`, `scene/mod.rs`), `crates/core/pulsar_game` (`tick.rs`, `scripting/driver.rs`, tests: the
  animation phase and notify flush in the tick), `crates/subsystems/pulsar_physics` (`physics_component`,
  `water.rs`), `crates/editor/ui_core/src/builtin_editors.rs` (editor registration),
  `.github/submodule-pin-exceptions.txt`, submodule pins (also `plugins/vendor/physics_plugin`).
- **Deleted / moved**: none.

### Graphy

- **Added**: `src/authored.rs`. **Changed**: `src/lib.rs`, `Cargo.toml` (`graphy` now depends
  on the leaf `graphy-blueprint`). Deleted: none.

### Pulsar-Reflection

- **Changed**: `asset_refs.rs` (kinds in Phase 1; JSON codecs for `Option<AssetRef<SkeletalMesh>>`
  and `Option<AssetRef<AnimationGraph>>` now), `lib.rs`.

### blueprint_editor

- **Changed**: `src/io/canvas_doc.rs` (209 lines of conversion deleted, now a 40-line wrapper),
  `src/features/events/engine_events.rs` (Montage and clip events as entry points), `compiler/*`.

### Helio

- **Added**: `helio-component/src/anim_source.rs`, `components/skeletal_phase3_tests.rs`,
  `components/skeletal_phase5_tests.rs`. **Changed**: `skeletal_mesh_component.rs`
  (component properties, runtime), `skeletal_notifies.rs`, `lib.rs`. Deleted: none.

## 5. What the user must review in the running app

None of these have been seen on screen. Each line names what to look for.

1. **Sequence editor**: the Notifies lane at several zooms. Dense notifies stack in up to three
   rows without label overlap; `+N stacked, zoom in` appears when more are needed; a state bar
   shows resize handles, a disabled state is muted and struck through; hover/selected looks;
   drag a notify, slide a state, drag a state's ends, Delete; the inspector's Start/End/Tick/
   Enabled/payload; save, close, reopen. The state button (timeline icon) next to `+`.
2. **Montage editor**: the skinned preview at the top of the inspector follows the playhead;
   masked upper-body tracks move only their bones; scrubbing back looks identical to going
   forward; no event is logged.
3. **Blend Space editor**: the skinned preview follows the probe point; Play/Pause animates;
   unsaved sample changes show without saving.
4. **Animation Graph**: the Layered Blend per Bone inspector: layer cards, branch bone/weight/
   depth edits (a bad bone name is refused with a message), reorder, presets, per-bone weight
   list; undo/redo of each; the preview with a bone-masked layer.
5. **Pose Database**: Build index shows `Building index… NN%` and a working Cancel without
   freezing the window; the previous index stays after Cancel; the Fallback panel shows the
   fallback pose and the explanatory note.
6. **Component details** (level editor and Blueprint): the Skeletal Mesh component rows
   `mesh`, `animation_graph`, `playing`, `playback_rate`, `root_motion_mode`,
   `root_motion_rotation`, `root_motion_vertical`; the pickers list only `.skmesh` /
   `.animgraph` files; Clear works; the mesh row shows the skeleton name resolved from the mesh;
   a mesh whose skeleton is missing says so.
7. **Graph canvas** (Animation Graph and Blueprint): zoomed node text, transition selection,
   zip ties after the shared conversion (open, edit, save, reopen a `.blueprint` and an
   `.animgraph` with zip ties; comments; Blueprint custom events).
8. Still to compare with `docs/mockups/animation-editor.html`: Retarget Profile, IK Rig, IK
   Retargeter, Pose Search query debug, retarget failures, IK goals, active Montage layers,
   Blend Space boundaries. Offscreen frames for the skinned previews and the timeline can be
   regenerated with `SKM_PREVIEW_DIR=<dir> cargo test -p skeletal_animation_plugin --lib render
   skin_preview` (needs a GPU adapter).

## 6. Adapters, and when each goes away

| Adapter | Where | Removal condition |
|---|---|---|
| `legacy_animation` / `looping` fields and the internal single-sequence player | `SkeletalMeshComponent` | one release after scenes saved before the graph runtime are re-saved; delete the fields and the `play_sequence` shim |
| `legacy_skeleton` (disagreement report with the mesh) | `SkeletalMeshComponent` | when no saved scene carries a skeleton ref on the component |
| `parse_migrating` and the `v1` readers for `.blendspace`, `.montage`, `.animgraph`, `.animseq` v1–v3 | `skeletal_assets` | when no supported project holds a v1 file; keep the `v1` fixtures until then |
| `to_canvas` / `node_in` aliases of `graphy::authored` | `skm_editor/.../anim_graph_canvas.rs` | when callers use `graphy::authored::*` directly |
| `AuthoredSidecar` | `blueprint_editor/src/io/canvas_doc.rs` | when `custom_event_defs` become part of the canvas document |
| `Option<AssetRef<K>>` JSON codecs for two kinds | `pulsar_reflection::asset_refs` (macro `reflect_optional_asset_ref!`) | when `Reflectable` is implemented generically for `AssetRef<K>`; then register by kind list |
| `OverlaySource` / `WithoutPoseDb` | `skm_editor/.../skin_preview.rs` | permanent (editor-only); the overlay disappears if the compiler takes a document instead of an asset source |
| Offscreen skinned preview shown as an image | `skin_preview.rs`, `skin_pane.rs` | when the Sequence viewport entity can be hosted with a runtime pose source (then the previews share its camera and passes) |
| `SceneInput::world_override` | `rendering/viewport/scene.rs` | same as above |
| Per-layer authored bone masks instead of a named mask library on the Skeleton | Animation Graph inspector | when named Skeleton masks are added to the Skeleton asset (plan item, not built) |

## 7. Check results

Run on 2026-10-29 on the author's Windows machine with a GPU adapter, debug profile.

| Check | Result |
|---|---|
| `cargo check --workspace` | passes (after the zip-tie compile fix in `blueprint_editor/.../properties.rs`, uncommitted) |
| `skeletal_assets` | 167 lib + 38 + 24 + 4 integration tests pass (includes `phase5_roundtrip`) |
| `skeletal_animation_plugin` | 181 pass (includes the GPU skin-render test, which ran) |
| `helio_component` | all pass: 171 lib (1 ignored) and the integration binaries, including the Phase 5 end-to-end test. Run with the Phase 5 files applied to the shared checkout, then removed; the committed Helio branch has them |
| `engine_fs` | all pass |
| `engine_backend` | all pass |
| `physics_plugin` | all pass |
| `blueprint_compiler` | all pass |
| `asset_import` | all pass |
| `ui_level_editor` | 133 pass, **1 fails**: `workspace::panels::properties::perf_tests::sculpt_stamps_do_not_rebuild_the_properties_section` (left 1, right 0). Sculpt/terrain properties; not investigated against a clean baseline |
| `pulsar_game` | all pass except **`tests/random_world_blueprint.rs`**: `seed 1: a terrain has at most 8 enabled layers`. Terrain content; not investigated against a clean baseline |
| `ui_common` | unit tests pass; **a doctest fails** (`command_palette/mod.rs` line 15: the example uses undefined `cx`/`window`); the file is unrelated to this work |
| `graphy` (`cargo test --lib authored`, in its own workspace) | 3 pass |
| `just clippy` (`-D warnings`) | **not clean**: `cargo clippy` on `skeletal_assets`, `skeletal_animation_plugin`, `ui_common`, `helio_component` reports many warnings in the whole dependency tree (duplicated attributes, collapsible `if`s, complex types, ...), so `-D warnings` fails the way it did before. I did not separate the warnings of the files changed here from the rest |
| `just check`, `just test` (whole workspace) | `just` itself was not run; the workspace was checked with `cargo check --workspace` and the named packages tested with `cargo test -p` |

Skipped: the cross-platform CI job, nextest `ci` profile run, `cargo fmt`, GPU tests excluded by the
`ci` nextest profile, all GUI checks, all performance measurement.


## 8. Remaining limitations and open items

- No GUI verification of anything (section 5).
- The plan's native UI screenshot step (4) and the mockup comparison are not done.
- Named Skeleton masks (stored library) are not built; masks are authored per layer.
- The notify lane stacks three rows; more share the last row until zoomed.
- The skinned previews are an image rendered offscreen; no camera control, no GPU-free fallback
  other than the message and the bone drawing.
- The editor and component end-to-end tests are separate tests, not one run over one project.
- SceneDB-level binding of component refs is covered by earlier-phase tests, not re-driven in the
  Phase 5 test.
- Helio's base contains the unrelated water commit (section 3).
- A Blueprint GUI pass for the Montage event entry points has not been done.
- `render_bps` has its own zip-tie conversion in `document.rs`; it is the user's in-progress work
  and was not moved onto `graphy::authored` (the render graph is a different schema).

## 9. Baseline harness (defined, not run)

From `docs/animation-phase0-audit.md` section 7. Build before any optimisation: a bench target in
`skeletal_assets/benches/` for the CPU-only workloads and one `helio-component` integration binary
for the component and SceneDB workloads, both writing `target/anim-baseline/<run-id>.json` with a
machine header (CPU, GPU, driver, OS, rustc, profile, features, git SHAs of the six repos).

| ID | Workload | Measures |
|---|---|---|
| W1 | synthetic rigs of 68 / 200 / 500 bones, 4–6 s clips at 30 fps | single-sequence evaluation p50/p95, allocations per tick |
| W2 | 1 / 100 / 1 000 components sharing one clip | tick time, scaling, memory |
| W3 | clips of 1 / 10 / 60 s | sampling cost against key count |
| W4 | 10 / 100 / 1 000 notifies per clip, loop wraps | `NotifyIndex::collect` p50/p95, dispatch latency |
| W5 | 1D (5) and 2D (9, 25) blend spaces | weight solve and blend |
| W6 | 1 / 2 / 4 layered Montages on 68 bones | blend cost |
| W7 | asset resolve cold/warm, catalog build over 100 / 1 000 / 10 000 asset projects | ms, file reads |
| W8 | root-motion extraction and physics hand-off, 68 bones | ms, copies |
| W9 | GPU palette upload (GPU, user-run) | bytes/frame, ms |
| W10 | timeline scrubbing: layout, hit test and draw of 68 tracks × 300 keys at several zooms | frame ms |

Method: release profile, 30 warm-up and 300 samples, fixed seed, pinned thread count, one machine,
the same commit pair before and after, p50/p95/max, allocation counts from a counting global
allocator, no line-count claims.

## 10. Finish pass (supersedes sections 3, 6 and 8 where they disagree)

Done after the Phase 5 hand-off, all merged to the upstream default branches:

- **Branches**: Helio carries the animation commits on `main` as `animation-phase4-clean` (the
  water commit is gone) plus the user's `helio-graph-asset` zip-tie commit that `render_bps` needs.
  Graphy's zip-tie commits were already on upstream `main`, so its branch is only `graphy::authored`.
- **Named Skeleton bone masks** (`SkeletonAsset::masks`): Layered Blend layers and Montage slot
  tracks name them (`BoneMaskSpec::named`); editors: Skeleton Masks section, per-layer *Mask from*
  row, Montage slot *Mask* row; `mask_library::promote*` migrates inline masks. See `.agents/ANIMATION.md`.
- **State machine events**: entered / exited / transition started / completed, component-scoped,
  canonical `animgraph::<graph id>.state_<kind>`, Blueprint entry points.
- **Whole-skeleton physics ownership** no longer holds the clock; it blends by a physics blend weight.
- **Root-motion authority** (`root_motion_authority`, move log). Missing: a gameplay networking layer
  (net roles, entity replication, prediction and correction, a Montage-play RPC). Nothing sends the moves.
- **Looping Montage segments** keep seam-spanning notify states running unless
  `restart_states_on_loop` is set.
- **Unified test**: `the_whole_character_flow_runs_over_one_project_on_disk` (helio_component).
- Blueprint plugin standalone build: `graphy` is a path dependency like the other `graphy-*` crates;
  the Pulsar-Native revs were re-pinned with `just bump-plugin-pins`.
- Adapters: the "per-layer authored masks" row of section 6 is gone: inline masks still load, and
  promote to named masks on request.
- Not run in this pass: full `just test` / `just clippy` (the user ran the app and waived them);
  `pulsar_game random_world_blueprint`, `ui_level_editor sculpt_stamps...` and the `ui_common`
  `command_palette` doctest still fail; none of the code they exercise is touched by this work
  (checked by diff against the merge base; not re-run on a clean checkout).
