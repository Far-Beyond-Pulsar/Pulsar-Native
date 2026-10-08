# Porting the voxel branches onto the corrected scene data

Status: notes for the owner of Pulsar-Native#994 and Far-Beyond-Pulsar/Helio#314. The corrective-plan branches are authoritative, and the voxel branches are rebased onto them after the plan lands.

Both voxel branches were written on the pre-correction base. They use three things the plan deleted:
- `PendingWorldWrites` with the behavior dispatch;
- the CPU light/mesh rows and their subscriptions;
- the destructive `take_component_change_events` queue.

Everything else in them (streaming, residency, appearance, caves) is voxel-internal and unaffected.

## Replace

| Voxel-branch code | Uses | Corrected equivalent |
|---|---|---|
| Helio `helio-component/src/components/atmosphere_component.rs`: `#[register_runtime_behavior]`, `sync_component` pushing `helio_pass_sky::AtmosphereComponent` through `PendingWorldWrites` | behavior dispatch (deleted, Phase 4) | A derived source row: `derived_row!(AtmosphereComponent, AtmosphereSourceRow, ...)` in `environment_rows.rs`. `environment_join` (`helio-default-graphs`) writes the pass's `"atmospheres"` row from it. `derived_row!` sees only the authored value, so `PlanetAtOwner`'s centre comes from the owner transform **in the join**, gated on enabled, live owner and hidden, as local fog and water volumes are (see [15-phase-4.md](15-phase-4.md), Stage 1). A disabled component writes an inert row instead of removing one. |
| Pulsar `engine_backend/src/scene/component_rows.rs`: `sync_component_rows`, `arm_component_row_subscriptions` | behavior dispatch, `PendingWorldWrites::drain_and_apply`, `World::subscribe_id`, `take_component_change_events` | Delete the file. Derived rows are written by the World's own write path, and the graph joins them every frame. Nothing re-derives on the CPU, and nothing subscribes. |
| Pulsar `scene/editor_rows.rs` (`is_editor_light_row_marker`), `helio_bridge::arm_render_row_subscriptions[_for_entity]` | CPU light rows and render subscriptions (deleted, Phase 2) | Lights are `LightSourceRow`s joined into `"scene_lights"` on the GPU (`scene_join`). There is nothing to arm. |
| `helio_bridge::ensure_gpu_mirror`: `helio_pass_sky::AtmosphereComponent::register_gpu_columns_growable` | the pass row as a mirror column | Register the source row (`AtmosphereSourceRow::register_gpu_columns_growable`), as the other `*SourceRow`s are. The pass row becomes a join output. |
| Pulsar `renderer.rs` scene-sync hunk: `dirty_lights` from `take_component_change_events`, `sync_static_mesh_rows`, `render_row_subscriptions_armed` | CPU projection (deleted, Phase 2) | Drop it. The renderer steps SceneDB when the world revision moves (no forced resync either, Phase 5). |
| Pulsar `renderer.rs` `relative_camera_world_compatible` / `RelativeCameraGate` | queries `helio_pass_forward_lit::LightComponent`, `LightComponentGpuMirror` and `editor_rows`; allowlists `scene_lights`, `static_objects`, `materials`, `billboard_instances`, `camera_postprocess` as mirror keys; assumes components sit on the owner entity | Rebuild it against the current mirror keys. These are source rows and owner columns: `light_sources`, `component_owners`, `object_hidden`, `static_mesh_draw_*` and the `*_sources` rows. Join outputs are not mirror keys. Components sit on **instance entities** (`attachments`), not the owner. As written, the gate would always fail closed. It would also reject the branch's own `voxel_planet.level`, which has an unbound `PostProcessVolumeComponent`. |
| Pulsar `voxel_backend.rs` / `voxel_frame.rs` tests that insert terrain and layers on one raw entity | pre-instance-entity model | Attach through `attachments::attach(...)`. `project_voxel_entries` reads `attachments::enabled_components`. |
| Helio `voxel_component_runtime.rs` hunk | context still has `#[register_runtime_behavior]` | Re-apply only the `VoxelLandform`/`VoxelFlatTerrain` → `VoxelTerrainLayersComponent` rename on the current file. That file uses `#[register_world_component]` only and keeps `declare_unfinished_component!("VoxelComponent", ...)` (#1056). |
| Any test reading `take_component_change_events` | destructive queue (removed from Pulsar, Phase 5) | Read a `ChangeCursor`, or `pulsar_world_registry::ComponentWatch` for keyed watching. |

The `scene_inventory` site patterns (`pending-world-writes`, `render-arm`, `cpu-projection`, `destructive-drain`) flag each of these call sites, so `cargo test -p scene_inventory` lists whatever is left.

## Keep (compatible with the corrected path)

- **Sun = the first enabled directional light**, read on the GPU from the join's `"scene_lights"` output (128-byte `GpuLight`). Derivations run before the graph, so the atmosphere pass sees this frame's lights. One caveat: Pulsar's CPU `VoxelSceneRead::sun` picks "first" by query order, and the GPU picks by join row order. With several directional lights, voxel shadows and the atmosphere could disagree. Pick one rule.
- **Deferred lighting's atmosphere binding** (`AtmosphereFrame`, transmittance and sky irradiance) and the atmosphere passes read only pass-published resources. They are independent of how the row is produced.
- **TSR's `rebase_previous_view`**, the history resets, and the debug-draw origin rebasing.
- **Camera-relative frames.** Helio's `set_world_origin` / `PrepareContext.world_origin` exist on the corrected base. `scene_join` and `environment_join` do not subtract `world_origin`, so a relative-frame gate (rebuilt as above) or join-side rebasing is required before enabling it beyond voxel scenes.
- **Voxel frame inputs:** `appearance_parameters`, `terrain_fingerprint` / `sync_edit_journals`, `local_up`, `camera_relative_frames`, `VoxelView { sun, .. }`, and `configure_appearance` resetting TSR history.

## Decide

- **Sky as a component.** Phase 4 kept the sky a project/renderer setting and marked a sky component unfinished (#1057). An authored `AtmosphereComponent` is that component. Porting it as a derived row closes #1057, and the ledger's sky rows move to verified with a rendered-frame test.
- **Removing the fallback sky / ambient.** Helio#314 removes `set_fallback_sky_enabled` and the hemisphere ambient. On the corrected base the editor renderer still calls those for scenes without an atmosphere. Removing them leaves such scenes with no outdoor sky unless a default atmosphere row is supplied.
