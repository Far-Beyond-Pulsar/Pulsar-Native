# Phase 2: the complete mesh and light path

Status: **landed** (Pulsar-Native#1035). Builds on Phase 1 ([12-phase-1.md](12-phase-1.md)). D2 (layout ownership, [11-phase-0-closure.md](11-phase-0-closure.md#d2-layout-ownership)) is approved and implemented here for meshes and lights.

Ground rule, unchanged: nothing here adds a subscription, refresh, mark or resync. The CPU projection and every workaround that kept it fed are deleted, and meshes and lights reach the frame from their own rows.

Phase 2 exit (from the plan): direct insertion and editor insertion both produce visible output without an inspector or notifications; editing, removal, enabled state, asset completion and static/movable transitions work. A rendered-frame probe validates the actual pass path.

| Exit criterion | Evidence |
|---|---|
| Direct and editor insertion visible, no inspector or notifications | `render_acceptance.rs`: editor insert before and after the first frame, with the properties panel open and draining the shared queue first, and a typed value inserted from code |
| Editing | `render_acceptance.rs`: move (depth changes); `scene_join_rows.rs`: owner moved, rows follow |
| Removal | `render_acceptance.rs`: component removed, frame equals the empty scene |
| Enabled state | `render_acceptance.rs`: mesh and light disabled and re-enabled; `scene_join_rows.rs`: instance disabled, light switched off |
| Asset completion | `render_acceptance.rs`: a mesh instance attached without geometry draws nothing, then draws when its geometry is written |
| Static/movable transitions | `scene_join_rows.rs`: a movability edit through the reflected property path sets and clears the movable flag of every draw row; `render_acceptance.rs`: the mesh stays drawn across both edits |
| Rendered-frame probe on the actual pass path | `render_acceptance.rs` renders the editor's `HelioRenderer` (the full default graph) and compares scene depth and color with the empty scene, camera at rest and nudging |

## Design (D2)

- The **component crate** declares the authored value and what it uploads. `StaticMeshComponent` mirrors its geometry itself and derives a second row, `StaticMeshDraw` (bounds, movable flag, one section per material section with the slot's resolved material). `LightComponent` derives `LightSourceRow` (the `GpuLight` its own mapping produces, in light space). `Visibility` derives `ObjectHidden`. `ComponentOwner` and `Transform` were already rows. Each derived row is an ordinary SceneDB GPU registration on the authored component, so it follows every insert, guarded write, removal, despawn and mirror replay.
- **SceneDB** packs and uploads all of it, generically.
- The **render-graph crate** (`helio-default-graphs::scene_join`) owns the mapping from those schemas to the pass inputs: a GPU join, run as a scene derivation between the SceneDB snapshot and the graph, that writes `static_objects`, `materials`, `scene_lights` and (editor) `billboard_instances`, the keys the passes already read. No pass changed.
- **Passes** keep their algorithms and input ABIs.

An instance is drawn when it is attached and enabled, the generation it recorded for its owner matches the owner's current one (SceneDB's generation mirror), the owner is not hidden and has a transform, and (meshes) it has geometry. Mesh instances write one object row and one material row per section, at the section's slot in the sections pool, so the row index is stable while the instance's sections are. Lights are written at their instance's row. The join reruns only when one of its inputs was rewritten, reallocated or resized; otherwise it records nothing and its outputs keep their generation, so the object-batch pass's own skip still works.

## SceneDB

Branch `claude/cool-hypatia-ict23g-phase-2` at `999373e`, on top of Phase 1's `358ec86`; not merged upstream, so the root `Cargo.toml` pin says so.

- A component may carry several GPU registrations (dispatch, clear, var-len release). Every registration runs; before, a second one for the same component was silently dropped. This is what lets a component crate add a derived row to a `SceneStore` type (`StaticMeshComponent`) or to a plain type (`Visibility`) without reshaping it.

Test: `world_gpu_mirror_replay.rs` `a_derived_row_registered_next_to_a_types_own_columns_runs_with_them` (insert before and after attach, guarded write, in-place insert, removal, despawn).

## Helio

- `helio_core::scene_derivation`: the `SceneDerivation` trait and `run_scene_derivations`. The renderer runs its derivations after building the frame's `SceneBufferProjection` and submits their work ahead of the graph; `RendererBuilder::with_scene_derivation` installs one. The renderer also publishes SceneDB's entity-generation mirror under `helio_core::ENTITY_GENERATIONS_KEY`.
- `helio-default-graphs::scene_join` (`SceneJoin`, `SceneJoinKeys`, `shaders/scene_join_*.wgsl`). The frontend names its buffers; the expected row sizes are constants checked at runtime (a mismatch is reported once and that half of the join draws nothing) and in a Pulsar test. Needs up to 13 storage buffers in one compute stage.
- Helio `main` merged (the Hi-Z reuse fix, Helio#317, and the lens-response shader update). Phase 0's "camera at rest culls everything" no longer reproduces: every acceptance case passes with the camera at rest.
- `helio-component`:
  - `StaticMeshDraw` (above). Material resolution moved here from `helio_bridge` unchanged (surface override, scalar surface asset, shader-graph folder with its textures registered in the mirror's texture store, imported surface); it now runs when the mesh is written, with the mirror the dispatch receives.
  - `LightSourceRow` (above).
  - `object_movability(world, object)`: the least mobile of a `helio::Movability` on the object and what its mesh and light instances author. Used by the motion gate, the play-mode static-move watch, the hierarchy's static badge and the gizmo's static-drag warning.

Test: `helio-default-graphs/tests/scene_join.rs` (handcrafted rows: object and material rows match `StaticObjectComponent::new` within 1e-4, a light's position and direction match glam, disabled/hidden/stale-generation instances write nothing, unchanged inputs record nothing). It runs in Helio's workspace; it was run here through a temporary copy in `engine_backend` and passed on lavapipe.

## Pulsar-Native

- `pulsar_scene_model::ObjectHidden`, `Visibility`'s derived row.
- `engine_backend::scene::helio_bridge` keeps `ensure_gpu_mirror` (now registering the join's inputs instead of the pass rows) and adds `scene_join_keys`/`scene_join`. Removed: `sync_static_mesh_rows`, `editor_rows::sync_editor_light_rows`, `arm_render_row_subscriptions*`, `mark_render_components_changed`, `dirty_render_instances`, `retire_gpu_rows_for_entity`, `project_movability`, the `MeshSectionDraw` helper entities, the editor row markers and the bridge's material resolution.
- The editor renderer and both `pulsar_game` renderers install the join; none of them drains the shared change queue or syncs rows per frame (the windowed game projected every mesh and light every frame).
- The editor no longer arms subscriptions after `AddObject`/`DuplicateObject`/`InstantiateClass` or marks rebuilt class instances after an asset update.
- The voxel sun and "are there any meshes" checks read the authored components.

Found and fixed on the way:

- Since Phase 1 Stage 2 the motion gate and the hierarchy's static badge looked for `helio::Movability` on the object while the projection put it on the instances, so authored-static objects could be moved by scripts and showed no badge. `object_movability` reads the authored values instead.
- Phase 0's light baseline used the default 1000 lm at ~400 units in a centimetre-scale scene (a fraction of a lux), so "the frame does not change" there was partly the setup. The acceptance test places the light near the mesh and gives it a test-scale intensity.
- `content_dedup_benchmarks` looked the vertex pool up under the stale key `StaticMeshComponent::vertices` (the same fix as `content_dedup` in Phase 1).

## Tests

RESULTS

## Ledger

Site rows removed with their code: `cpu-projection` (bridge, editor rows, both game renderers), `render-arm` (bridge, renderer, executor, asset updates), `render-mark` (asset updates), `subscribe` (bridge), `destructive-drain` (renderer). The renderer's remaining `cpu-projection` site is `sync_editor_postprocess` (Phase 4). The pass-owned `StaticObjectComponent`, `MaterialComponent`, forward-lit `LightComponent` and `BillboardComponent` are no longer registered or written by Pulsar; their rows are `verified` as replaced, and the types remain for Helio's standalone demos. New rows: the join's inputs (`StaticMeshDraw`, `LightSourceRow`, `ObjectHidden`, `Visibility` and their buffers, the handle tables, `Transform::packed`, `world_entity_generations`). `component_owners`, `static_objects`, `materials`, `scene_lights`, `billboard_instances` are `verified`. `cargo test -p scene_inventory` passes.

## Open points

- Helio's own workspace pins an older SceneDB and Pulsar-Reflection. The `helio` and `helio-default-graphs` changes use only API that pin already has; repinning Helio is separate work.
- The pass-owned scene types above remain for Helio's demos, and `helio-pass-hlfs` still reads them from a CPU `World` (Helio examples only). Phase 4.
- `LightComponentGpuMirror` (the generic reflection of the light's `#[gpu]` fields) has no consumer now; the lighting input is `LightSourceRow`. Phase 4 decides whether the companion stays.
- Material resolution runs on mesh writes under the world lock: shader graphs are cached by file fingerprint, scalar surface assets are read per write. A material asset edited on disk reaches the mesh when the mesh is next written (there was no watcher before either).
- The renderer still reads authored lights on the CPU for the voxel sun direction. Phase 4.
- DX12 is not verified here (no Windows device); Vulkan/lavapipe is.
- `voxel_block_api` `the_cached_world_follows_the_journal_and_the_settings` can fail when run in parallel with its siblings: `terrain_world`'s process-wide cache is keyed by entity bits alone, which every test world shares. Not changed here.
