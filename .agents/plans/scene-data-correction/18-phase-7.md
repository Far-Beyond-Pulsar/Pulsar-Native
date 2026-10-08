# Phase 7: the acceptance gaps

Status: **complete**; the editor wiring of plugin components is deferred to Pulsar-Native#1083 (Pulsar-Native#1035, #1081), pending review. Builds on Phase 6 ([17-phase-6.md](17-phase-6.md)).

Phase 6 left the acceptance matrix partly unmet; its gaps were filed as Pulsar-Native#1081. Phase 7 writes the missing tests. Six of them found real defects, and Phase 7 fixes each one:

1. A read-only `get_mut` guard re-uploaded its row.
2. Disabling a `ClassInstance` did not stop its script.
3. Load errors did not name the property.
4. Every Helio debug line was projected with the wrong camera.
5. Re-imported meshes never reloaded.
6. Helio's own SceneDB pin was stale behind a `[patch]`.

## Fixed

### A read-only `get_mut` guard uploaded its row (SceneDB `7d14a4c`)
- **Problem:** a `Mut`/`MutDyn` guard dropped without a write still ran the GPU dispatch and the change-tracker record. Reading a component through `get_mut` re-uploaded its row and reported it changed for replication.
- **Fix:** every hook now fires only on a real write, as the journal, subscription and handle hooks already did. A write still re-uploads `Once` fields. The README contract is updated.
- **Found by:** `world_gpu_mirror_lifecycle.rs` `uploads_are_bounded_to_the_rows_that_changed`.

### Disabling a `ClassInstance` did not stop its script (Phase 1 open item)
- **Problem:** the script driver watched `ClassInstance` changes only. The enabled flag lives on `ComponentOwner`, so the driver never saw it change.
- **Fix:** the driver also reads a `ComponentOwner` cursor and treats a disabled `ClassInstance` as absent: `end_play` at the next tick, and a new instance when it is enabled again.
- **Test:** `pulsar_game` `disabling_a_class_instance_stops_its_script_and_enabling_restarts_it`. It fails on the old driver.

### Load errors name the property
- **Problem:** a record that failed to decode reported serde's bare message: object and class, no field.
- **Fix:** `pulsar_world_registry::decode_json` (`serde_path_to_error`) reports the field path. Users:
  - the decoder that `#[register_world_component]` generates;
  - the light and mesh custom decoders.
- **Result:** the runtime loader refuses such a level with `failed to hydrate component LightComponent on object sun: … intensity.intensity: invalid type: string "bright", expected f32`.
- **Test:** `a_load_error_names_object_component_and_property`.

### Every Helio debug line used the wrong camera (Helio `14dedf5a`)
- **Problem:** `build_default_graph_with_context`, `build_default_graph_external_with_lighting_passes` and `build_default_graph_with_user_effects_with_context` passed `ctx.camera_buffer` (the `GpuCameraUniforms` storage buffer) where the graph expects the 64-byte debug camera uniform.
- **Effect:** both `DebugDraw` passes projected every line with the wrong matrix:
  - the editor's spline curves;
  - Helio's editor volume bounds (no Pulsar caller today);
  - user lines, including the gizmo.

  Only geometry crossing the near plane (the camera marker) reached the screen.
- **Found by:** `splines_reach_the_frame`. Phase 4 had recorded splines as drawn by the editor debug pass, but no test checked the frame.

### Re-imported meshes reload in place
- **Problem:** a mesh file re-imported on disk reached its placed meshes only on their next `mesh_asset` write.
- **Fix:**
  - `helio_component::mesh_cache::import_model_to_native` now publishes `AssetUpdated(Mesh, native path)`.
  - The level editor subscribes (`core::asset_updates::subscribe_mesh_updates`, held by the panel next to the class-update subscription). Every `StaticMeshComponent` whose `mesh_asset` resolves to the file reloads it through the same `mesh_asset` write the properties panel makes.
- **Tests:** `ui_level_editor/tests/mesh_reimport.rs`; `mesh_cache` `an_import_announces_the_written_mesh`.

### Helio's own SceneDB pin
- **Problem:** Helio's workspace pinned SceneDB `8f98f83` plus a `[patch]` to `7b0414b` that drew in a second `pulsar_reflection`. The workspace did not build standalone (`helio-pass-sky`: `Reflectable` from two versions).
- **Fix:** Helio now pins SceneDB `7d14a4c` and the Pulsar-Reflection rev SceneDB pins. The patch is removed, as its own comment asked once the pins were bumped. The workspace checks standalone.
- **Pre-existing failures:** `limited_native` and `voxel_pass_graph` fail the same way at the old pins (wgpu panics under lavapipe).

### Shared game renderer
`pulsar_game::game_renderer` now builds the Helio renderer for both:
- the standalone window (project settings, own device);
- the embedded PIE viewport (host device).

Each of these used to build the same renderer inline. The parity test drives this code.

## Plugin components: shared world library

A plugin is a `cdylib` with its own static copy of `pulsar_scenedb`, `pulsar_reflection`, `pulsar_world_registry` and `inventory`. Its `#[register_world_component]` registration runs into its own registries, which the editor never reads. So a plugin's `Box<dyn EngineClass>` factory could not become a typed World component. `06-modules-and-plugins.md` left the ABI open; the decision is **a shared engine dylib**, the pattern Bevy uses for `bevy_dylib`.

`crates/core/pulsar_world_dylib` is a Rust `dylib` that contains those crates. A binary and a plugin that both link it (`use pulsar_world_dylib as _;`) use its single copy: the plugin's registration runs into the host's registries when the library loads.

Tests (`plugin_manager/tests/world_component_plugins.rs`): the fixtures under `tests/fixtures/world_plugins` build a host and two plugins that define the same `PluginWidget` class.
- `a_statically_linked_plugin_cannot_register_world_components` shows the issue. The plugin sees its class, the host does not, and the component ids differ.
- `a_plugin_linked_through_the_world_dylib_registers_live_world_components` shows the fix. The host sees the class with the plugin's component id, creates it through the registry, writes `charge` through reflection, reads it back from its World and reads the write from its change journal.

**Requirements.** The host and the plugin must name the same build of the dylib: same compiler, same sources and the same features of every crate inside it. In practice they must be built in one cargo invocation of this workspace, as the test builds its fixtures. A mismatched plugin fails to load with an undefined-symbol error. The binary needs the dylib and the toolchain's `libstd-*.so` at run time (`cargo run` and `cargo test` set the search path).

**Not wired into the editor: deferred to Pulsar-Native#1083**, together with the shared library the UI framework will need. Linking `pulsar_engine` to the dylib would also:
1. **Split the editor's allocations.** The dylib links the standard library dynamically. A binary's `#[global_allocator]` then serves only the generic code instantiated in that binary; code in `libstd` and in the dylib allocates through `libstd`'s default. `TrackingAllocator` (the memory panel) and the `dhat-heap` profiler would see part of the heap. Pinned by `a_binary_linked_to_the_world_dylib_keeps_only_part_of_its_allocations`.
2. **Change the release format.** The release ships one executable per target. It would have to ship the dylib and `libstd` beside it, with an `$ORIGIN` rpath on Linux and the libraries inside the macOS `.app`.
3. **Risk Windows debug builds.** A debug build of the dylib exports about 83,000 symbols on Linux; a Windows DLL is limited to 65,535. A release build exports about 10,000. Not checked on Windows here.

The plugin loader now warns when a plugin's component classes are not in the host's World registry, naming them.

## New tests, by acceptance group

| Group | Tests added in Phase 7 |
|---|---|
| Mutation equivalence | `gpu_mirror_derive.rs` `nested_collection_and_method_writes_match_a_typed_insert`: a nested `#[sub_props]` setter, a collection setter (normalized under the same write) and a reflected `&mut self` method each give the same value and GPU row values as a typed insert. `transform_script.rs` `a_script_write_reaches_the_gpu_row_like_a_typed_write` |
| Instance lifecycle | `scene_join_rows.rs` `every_path_addresses_one_instance_and_a_reused_index_is_a_new_one`: property path, script `ComponentRef` and duplicate each land in their own joined light row, equal to a typed insert; a recycled slot is a new instance and the old handle writes nothing. `disabling_a_class_instance_stops_its_script_and_enabling_restarts_it`. Duplicate, reorder and nest were already covered (`duplicate_lights_keep_distinct_values_when_the_live_instance_changes`, `component_parent_survives_typed_edits_and_history`) |
| Observer fanout | `change_watch.rs` `watchers_at_different_poll_rates_miss_nothing`; `scene_edit` `a_level_load_ends_the_feed_and_the_selection_follows_the_loaded_object` |
| Mirror lifecycle | SceneDB `world_gpu_mirror_lifecycle.rs`: a type first inserted after other rows uploaded; a 5000-row bulk load past capacity, read back row by row, earlier rows kept and a new epoch; a mirror rebuilt on a new device from the World (writes made while detached included); two worlds with their own mirrors; bounded uploads. Existing coverage the Phase 6 matrix had not cited: growth (`world_gpu_mirror_growable.rs`, `gpu_buffer_registry_epoch.rs`, `gpu_buffer_row_capacity.rs`), freed-space reuse (`world_gpu_mirror_var_len.rs`, `shrink_reclaims_capacity_after_a_peak_then_drop`) and cell-path compaction (`gpu_store.rs`). Var-len pools reuse freed spans and do not compact, by design |
| Real render output | `splines_reach_the_frame` (drawn, follows its owner, gone when disabled or removed) |
| Mesh/light regression | `mesh_and_light_variations_reach_the_frame`: scale; point, spot and directional lights; intensity; a spot or directional light turned away; a light's `cast_shadows`; the same frame with the inspector following the light or not |
| Asset behavior | `helio_component/tests/static_mesh_assets.rs`: missing and corrupt assets attach as an empty mesh with the path kept; rapid replacement leaves the last; two components of one asset share one GPU allocation; a movability edit neither reloads (the asset was deleted meanwhile) nor re-uploads the geometry; reassigning the path loads changed content. `mesh_reimport.rs` (hot reload). Mesh assets load synchronously inside the write that names them, so there is no in-flight load to cancel, outlive its component or complete stale |
| Persistence compatibility | `a_load_error_names_object_component_and_property`; `pulsar_package/tests/cooked_level.rs`: a level cooked for packaging loads, through the runtime loader, the same typed components as its source (old flat light migrated, nested, disabled, several of one class, unknown class kept) |
| Runtime parity | `pulsar_game/tests/runtime_render_parity.rs`: one level file through the runtime loader renders its mesh through the standalone renderer, the embedded one and the editor viewport's, headless; none draws it once disabled |
| Cost and architecture | `an_idle_scene_encodes_nothing_and_an_edit_wakes_it`: once settled, twenty viewport requests encode no frame and leave the world revision unchanged; one edit wakes one frame. `uploads_are_bounded_to_the_rows_that_changed` (SceneDB). Unrelated edits: `spline_lines_rebuild_only_when_a_spline_or_its_owner_changes`, `unchanged_inputs_record_nothing_and_keep_the_outputs`, and the movability edit above |

## Closure checklist (from the plan), updated

The two items Phase 6 left unchecked:
- [x] GPU reflection covers all mutation, removal, attachment and buffer lifecycle paths generically. Growth, late types, bulk load, device recreation and several worlds are now tested. Var-len pools reuse space and do not compact.
- [x] Every supported component's effect is demonstrated through the actual editor and runtime path. Splines now have a frame test, and the standalone and embedded runtimes render the same level.

## Not done here

- **Plugin components in the shipped editor:** Pulsar-Native#1083. See [Plugin components](#plugin-components-shared-world-library). The shared library works and is tested, but the editor binary does not link it yet.
- **GPU checks on hardware and DX12.** This environment has Mesa lavapipe only.
- **`gpu_rows` padding.** An `#[engine_class(gpu_rows)]` mirror with a field narrower than its slot (a `bool`) uploads that slot's uninitialized padding. No production class uses `gpu_rows`; production GPU rows come from `#[derive(SceneStore)]`, whose `Pod` bound forbids padding.
- **Carried from Phase 6:**
  - the `pulsar-reflection` residue (a separate repository);
  - the voxel path;
  - the unfinished classes and passes with their issues.

## Revisions

- SceneDB: `a98ff98e1b446e0f44f4200c005faa0c1f2842b2`, pinned by the root `Cargo.toml` (dependency and `[patch]`) and by Helio's own `Cargo.toml`. `7d14a4c` is the get_mut fix; `a98ff98` adds the CI fixes below.
- Helio submodule: see the Pulsar-Native branch head. The Phase 7 commits:
  - `0f6c5b5a`, `2ea90c69` and `a7b4c240` (pins);
  - `5ec13ddd` (decoders);
  - `14dedf5a` (debug camera);
  - `6e816c5e` (asset test);
  - `3fa05688` (import event);
  - `08715022` (graph materials, from main's PSGC update).

## Merge with main

Pulsar `main` was merged into the top of the stack (18 commits). Three conflicts:
- **`engine_backend/src/scene/helio_bridge.rs`.** Main's PSGC material-graph update (4980a8842) edited the CPU mesh projection that Phase 2 replaced with the GPU scene join; material resolution has lived in Helio's `StaticMeshDraw` since then. The update's two modules move to `helio_component::material_graph` and `::material_textures` (Helio `08715022`), and `StaticMeshDraw` uses them. Main's `material_graph_rendering` test now drives those modules: 13 pass, including the GPU pixel tests.
- **`ui_level_editor/src/state/scene.rs`.** Main's `world_settings` field is kept.
- **The Helio submodule.** Main's commit is an ancestor of ours.

Failures on main that also blocked this stack, fixed here:
- `plugin_editor_api`'s `surface_animation` tests did not compile (missing `ui::PixelsExt` import).
- `pulsar_script_vm` `events::declared_events_are_verified_locally` used `Vec3` as an invalid event field; registered value types are valid, so the invalid example is now a component reference.
- `pulsar_package` smoke fixtures were stale (module format 3, compiler writes 5); regenerated.
- SceneDB (`a98ff98`): four GPU test targets had no `required-features = ["gpu"]` (the `build` and `no-default-features` jobs did not compile). `DirtyTrackedSceneBuffer::reserve` grew its CPU shadow before the GPU buffer checked the device ceiling, so reserving `u32::MAX` rows aborted on a 16 GB allocation (`world_gpu_mirror_reservation_shrink`, the `all-features` job); the GPU buffer now grows first.

Failures on main that remain, outside these repositories:
- `ui` (WGPUI-Component) `cached_scrollable_panels` does not compile: it uses `gpui::headless` and `TestAppContext::with_real_text_system`, which no WGPUI branch has. CI's `cargo check --workspace --all-targets` fails on it.
- `scripts/plugin-pins.sh check`: main's `table_editor` pins Pulsar `62bb5c1` (not in this repository) and `shader_editor` pins `bbe5ecd` (not on main). The pins live in those plugin repositories.
- `plugin_editor_api` `surface_ticks_do_not_rebuild_views_and_stop_when_hidden` now compiles and fails (the leaf view re-renders on each tick against the pinned WGPUI).

## Sweep

**After the merge with main** (SceneDB `a98ff98`, Helio `a7b4c240`, Mesa lavapipe):
- Packages: `pulsar_world_registry`, `pulsar_scene_model`, `pulsar_class`, `pulsar_script_object_model`, `pulsar_physics`, `pulsar_scene`, `scene_inventory`, `pulsar_package`, `plugin_manager`, `plugin_editor_api`, `pulsar_script_vm`, `engine_backend`, `helio_component`, `pulsar_game` and `ui_level_editor`.
- Results: 70 targets, 572 tests passed. Every acceptance test passes, as do main's `material_graph_rendering` and the ledger and architecture checks.
- Failing, all in code identical to main's:
  - the gizmo hover test;
  - the two `toggle_button` doctests;
  - `plugin_editor_api` `surface_ticks_do_not_rebuild_views_and_stop_when_hidden`;
  - `voxel_block_api` `the_cached_world_follows_the_journal_and_the_settings` (main's voxel code is outdated; left for the voxel work).
- The packaging CI job's steps pass locally: script VM and runtime tests, the smoke test, `pulsar package`, and the Blueprint compile.
- SceneDB's `build`, `no-default-features` and `all-features` jobs pass locally.

**Before the merge:**

Run on the final pins (SceneDB `7d14a4c`, Helio `2ea90c69`), Mesa lavapipe, `--no-fail-fast`. About 560 tests passed in all.

**Pulsar**, by group:

| Packages | Targets | Passed | Failed |
|---|---|---|---|
| `pulsar_world_registry`, `pulsar_scene_model`, `pulsar_class`, `pulsar_script_object_model`, `pulsar_script_vm`, `pulsar_physics`, `pulsar_scene`, `scene_inventory` | 28 | 193 | 2 |
| `engine_class_derive` (with `pulsar_reflection/prims-gpui`; it does not compile alone without it, as before) | 6 | 20 | 0 |
| `engine_backend`, `helio_component`, `pulsar_package` | 25 | 163 | 4 |
| `pulsar_game` (including the runtime parity test) | 5 | 58 | 0 |
| `ui_level_editor` (all eight `render_acceptance` tests and `mesh_reimport`) | 4 | 121 | 2 doctests |
| `plugin_manager` `world_component_plugins` | 1 | 3 | 0 |

Failures fixed here and rerun green:
- The `scene_inventory` ledger: the `mirror-replay` rows still named `embed.rs` and `windowed_app.rs`. They now name `game_renderer.rs`. The ledger (7) and architecture (4) checks pass.
- `pulsar_package` `cook::tests::levels_lose_editor_data_and_absolute_paths` and `smoke::packaged_game_runs_from_a_clean_directory`. They expected the `props.mesh_asset` copy that the Phase 3 load migration removes. They now check that the path is in the component and the copy is gone.

`pulsar_package` was not in earlier sweeps.

Failing, not caused by this work:
- the gizmo hover test (`press_captures_without_hover_and_drag_continues_off_handle`);
- `pulsar_script_vm` `events::declared_events_are_verified_locally`;
- the two `toggle_button` doctests;
- `pulsar_package` `smoke::fixture_modules_are_current`, which fails on main too: the committed fixture modules are format 3, the compiler writes 5.

**SceneDB** (`--features gpu`):
- the new `world_gpu_mirror_lifecycle.rs` passes;
- the same two lavapipe failures as Phase 6 remain (`alloc_gate_gpu`, `world_gpu_mirror_reservation_shrink`).

**Helio**:
- `helio-component` passes, as do the new asset and import tests;
- `limited_native` and `voxel_pass_graph` fail the same way at the old pins.
