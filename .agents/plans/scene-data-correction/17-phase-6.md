# Phase 6: residue removed, ledger closed

Status: **complete** (Pulsar-Native#1035), pending review. Builds on Phase 5 ([16-phase-5.md](16-phase-5.md)).

Phase 6 exit (from the plan): every ledger row is closed with evidence, all acceptance groups pass, dependency and submodule revisions are reproducible, and documentation describes the implemented architecture.

Result:
- Every ledger row is closed.
- Revisions are pinned.
- The docs describe the architecture as built.
- The acceptance matrix is **not** fully met: [Acceptance matrix](#acceptance-matrix) lists each group's evidence and its gaps, which are tracked in Pulsar-Native#1081.

## Decisions (approved)

1. **SceneDB's shared-queue subscription API is removed**, not kept for compatibility. Removed: `subscribe`, `subscribe_id`, `unsubscribe`, `unsubscribe_for_entity`, `take_component_change_events`, and the pending/dropped counters.
2. **Subscriptions run against the normal data flow.** Data flows from an edit straight into the `World`, from the `World` to the GPU, and from there to readers. Renderers and other bulk readers never subscribe. A view that shows an object subscribes so it follows writes made elsewhere (a gizmo, a script, undo) without polling. Example: the properties panel showing the selection.
3. **Delivery is a callback with the value.** The callback runs inside the write and receives the component's full new value. `GpuHeavy<T>` fields carry only their reference, so heavy GPU data is never copied.
4. **Scope is a whole object**: the object entity and all of its component-instance entities, including ones attached later. Per-component scope can come later.
5. **Pass rows close by test input.** A pass is verified when a rendered-frame test drives authored input through it. A pass in the editor graph with no authored input is unfinished, with an issue. A pass outside every production graph is out of scope.

Decision 1 of Phase 5 ("panels invalidate and re-read") is superseded for panels by decisions 2–4: the panel now receives values. Script watches keep Phase 5's cursor semantics.

## Stages

1. SceneDB: object subscriptions replace the shared queue; repin.
2. Compatibility residue removed from Pulsar and Helio.
3. Every remaining ledger row closed.
4. Architecture checks, docs, this record, PRs.

## Stage 1: object subscriptions (SceneDB `bb7c8aa`)

- `World::subscribe_object(object, callback)` returns an `Option<SubscriptionId>`; `unsubscribe_object` ends one.
- `set_object_resolver` maps an entity to its object; the default maps each entity to itself. `object_of` reads the mapping.
- The callback receives an `ObjectEvent`:
  - `Changed(ObjectChange { entity, component, kind, value })` for each write. `value` is `Option<&dyn Any>`.
  - `Despawned`, which ends the subscription.
- Every write path delivers:
  - insert, overwrite, `push_new_value`;
  - `remove` and `remove_dyn`, with the removed value;
  - despawn: a `Removed` for each component, then `Despawned`;
  - `Delta::apply` (replicated writes);
  - dropping a `Mut` or `MutDyn` guard.
- With no subscriptions, a write costs one `Option` check.
- Replicated writes now record journal `Mutated` entries. They used to be invisible to cursor readers.
- `ComponentChangeKind` moved to `change_journal`.
- Subscription ids are unique across every `World` (SceneDB `1981d7c`). A per-world counter let an id kept past a world replacement end an unrelated subscription on the new world. The editor never swaps its `World` (a level load clears it in place, so the panel sees `Despawned` and re-follows), but the id must not depend on that.
- Tests:
  - `tests/world_object_subscriptions.rs`: `every_write_path_calls_back_with_the_new_value`, `erased_writes_call_back_like_typed_ones`, `an_object_covers_its_component_entities_including_later_ones`, `despawning_the_object_ends_its_subscriptions`, `unsubscribing_stops_only_that_subscriber`, `a_replicated_write_calls_back_with_the_new_value`, `a_world_without_subscriptions_never_resolves_objects`, `an_id_from_a_replaced_world_ends_nothing_on_the_new_one`.
  - `tests/world_change_journal.rs`: `bundle_inserts_and_despawns_record_each_component`, `a_replicated_write_is_recorded`.

In Pulsar, `pulsar_world_registry::ObjectFeed`:
- installs `attachments::object_of` as the resolver;
- copies each delivered value (the registered `clone_value`, or `Transform`/`Name`/`Visibility`);
- queues an `ObjectUpdate` and calls the view's `wake`.

The properties panel follows the selected object's feed. A smol channel wakes it, and it applies transform, header and component-card values without re-reading the `World`. A focused input is not overwritten. When a full restore despawns the selected object, the panel follows the respawned object.

Tests:
- `object_feed.rs`: `a_transform_written_elsewhere_arrives_with_its_value`, `a_component_instance_arrives_with_its_full_value`, `despawn_ends_the_feed_and_unsubscribe_stops_it`.
- `scene_edit/tests/components.rs` `every_panel_feed_receives_edits_with_their_values`. It replaces Phase 5's `every_watcher_sees_an_edit_whichever_polls_first`.
- `commands/tests.rs` `a_component_property_command_reaches_the_object_feed`.
- `render_acceptance.rs`: the meshes-and-lights test renders with a panel subscribed.

## Stage 2: compatibility residue removed

Pulsar:
- The panel's shared property-change queue (`scene_edit/changes.rs`) and the orphaned terrain files are deleted.
- **A bug found and fixed:** component lifecycles drained the change tracker's removal list. When another reader drained the list first, an instance's `end_play` never ran. Lifecycles now read removals from their own change cursors. Test: `component_lifecycle.rs` `a_removal_drained_by_another_reader_still_ends_the_instance`.
- `#[register_world_component]` now takes an inherent `impl Type {}` and names the class after the type. The `on_removed` adapter and the `ComponentRuntimeBehavior` stubs that only carried a class name are gone. The ledger's `runtime_behavior` column is gone too.
- Deleted:
  - `helio_bridge` registrations for `RenderGroup`, `Sublevel` and `SectionedObject`, which had no consumer;
  - `read_component_properties_batch`, `build_transform_parts` and `subscriptions_epoch()`;
  - the scene-props projection half: the physics `scene_props` projection, `pulsar_scene` `projected_props`, and the prop helpers.
- The half that strips old projected keys from file props stays as a load migration (`pulsar_class::records`).
- Comments that named deleted types (`SceneDatabase`, `WorldSceneStore`, …) now name the code that exists.
- Editor component tests read live values typed, not through the save encoder.

Helio:
- Components register on inherent impls.
- `SublevelComponent` (deprecated, no consumer) is deleted.
- The scene-props projection half is removed.
- The light mapping test is fixed: lumens → candela is `lm / 4π`, and `Candelas` passes through. This test had failed since Phase 3.
- A forward-lit shader comment no longer names a deleted function.

Kept, with reason:
- `RendererCommand::ToggleFeature`: the toolbar's command channel to the render thread. It carries no scene data.
- `StaticMeshComponent::legacy_material_override`: a one-load migration field for old levels. Hydration folds it into the material slots and clears it.

## Stage 3: every ledger row closed

| Table | Rows | Verified | Unfinished (issue) | Out of scope |
|---|---|---|---|---|
| class | 20 | 12 | 5 | 3 |
| schema | 62 | 46 | 11 | 5 |
| buffer | 47 | 31 | 11 | 5 |
| pass | 49 | 23 | 6 | 20 |
| site | 24 | 23 | 1 | 0 |

- **Passes:**
  - Verified: the pass runs in `render_acceptance.rs` `every_pass_of_the_editor_graph_runs` with authored input, or in a named frame test.
  - Unfinished: in the editor graph with no authored input. Sky #1057, decal #1058, corona #1059, billboard #1060, portal cull/instances #1055.
  - Out of scope: in no production graph, or run only as editor/debug passes (ssr, planar reflection, tsr, forward-lit, simple-cube, hlfs).
- **Classes:**
  - `LightComponent` and `StaticMeshComponent` are verified by the frame tests.
  - Unfinished: `NativeScriptComponent` (new issue #1079: no runtime consumer), LOD #1053, portals #1055, reflection captures #1054, voxel #1056.
- **Schemas and buffers:**
  - Unfinished: water hitboxes (new issue #1080: no authored producer), `SubLevelActor`/`sublevel_actors` #1055, foliage interactors #1063.
  - Verified: `MeshComponent`, `Transform`, Helio's `LightComponent` schema, `component_owners` and `builtin_mesh_*`.
- **Sites:** one unfinished, the HLFS acceleration-structure reader location (#1066).

`cargo test -p scene_inventory` checks every row against the linked registries and the source tree.

## Stage 4: architecture checks and docs

`crates/core/scene_inventory/tests/architecture.rs` (`scene_inventory::architecture`):

- **`retired_mechanisms_have_no_call_sites`:** these site patterns must have no production call site:
  - destructive-drain, panel-drain, script-drain, subscribe;
  - gpu-refresh, render-mark, render-arm;
  - pending-world-writes, cpu-projection, render-props-sync;
  - behavior-dispatch, force-resync.
- **`json_records_decode_only_at_the_boundary`:** JSON hydration and record attachment happen only in the listed boundary files:
  - level load (`runtime_level.rs`);
  - `level_migrate`;
  - the record API (`instances.rs`);
  - `pulsar_class::attach_components`;
  - the editor's record add/replace.
- **`renderers_do_not_subscribe_to_objects`.**
- **`renderer_world_queries_are_the_listed_ones`:** every `World::query` in the editor's render bridge and the Helio crates is listed with its reason:
  - picking, on a pointer event;
  - the bake input;
  - spline debug lines, gated by a change cursor;
  - the HLFS acceleration build: one build, then cursors;
  - the voxel path, outside this plan.

  A new site or a changed count fails. To check the check, a probe query was added to `helio/src/lib.rs`: the test failed, and the probe was removed.

Boundary codecs (reflection save codecs, asset files) and gameplay events are not checked, as the plan requires.

Docs:
- `SCENEDB_MIGRATION.md` is rewritten as the architecture as built: ownership, component instances, data flow, JSON boundaries, observers, renderer, checks.
- `ECS.md` is rewritten for SceneDB's `World`. The `pulsar_ecs` crate it described no longer exists.
- `REFLECTION.md`, `COMPONENT_RUNTIME_GAPS.md`, `ROADMAP.md` and `CRATES.md` no longer describe the runtime-behavior dispatch, `pulsar_ecs` or `ComponentStore`.
- `SCENEDB_CORRECTIVE_PLAN.md`, `HELIO_SCENE_API_MIGRATION.md` and `SCRIPTING_EPIC.md` are historical records. Each gets a status note pointing at what replaced its findings.
- `AGENTS.md` indexes the rewritten docs.

## Acceptance matrix

Evidence named in the phase records (12–17), checked to exist. **Gaps** are required cases with no test; they are tracked in Pulsar-Native#1081.

| Group | Evidence | Gaps |
|---|---|---|
| Mutation equivalence | SceneDB `world_erased_insert.rs`; `component_instances.rs` `factory_decode_and_typed_inserts_are_equivalent`; `gpu_mirror_derive.rs` (reflected property writes reach the GPU row); `commands/tests.rs` `typed_component_commands_are_undoable_and_report_no_ops`; `render_acceptance.rs` `meshes_and_lights_reach_the_frame_from_every_producer` (asset drop, panel add, typed insert); `classes.rs` `instances_are_built_from_the_typed_template`; `component_dispatch.rs` `generated_defaults_decode_once_and_each_actor_gets_its_own_value` | reflected method; nested/collection edit; plugin factory (plugin-only classes cannot be added yet); script write reaching the GPU row |
| Instance lifecycle | `several_and_disabled_instances_are_separate_typed_values`; `scene_join_rows.rs`; `gpu_mirror_row_follows_a_plain_insert_and_removal`; SceneDB `the_generation_buffer_handle_follows_spawns_and_despawns`; `hiding_the_owner_a_stale_generation_or_a_disabled_light_removes_the_rows`; `despawn_reports_changed_and_later_writes_are_typed_errors`; `undo_restores_component_instances_in_place`; `redo_and_undo_keep_working_after_a_round_trip`; `history_level_replacement_and_viewports_need_no_resync` | duplicate/reorder/nest; index reuse; one instance addressed by method, property and GPU join; disabling a `ClassInstance` does not stop its script (Phase 1 open item) |
| Observer fanout | Cursors: `change_watch.rs` (two watchers in either order, watch-then-read, unwatch, rewatch, overflow, replaced world); `two_scripts_each_see_the_change`; `an_overflowed_class_instance_journal_rescans`; `a_replaced_world_is_rescanned`; `StaticMoveWatch` overflow and replaced world; SceneDB `a_cursor_survives_its_world_being_replaced`. Object subscriptions: Stage 1 tests. Zero subscribers: every frame test | no test varies poll frequency explicitly; the panel across a level load is untested at panel level (the feed re-follows on `Despawned`; stale ids are covered in SceneDB) |
| Mirror lifecycle | SceneDB `attaching_after_population_writes_every_existing_gpu_row`, `attaching_a_new_mirror_replays_into_it_and_reattaching_a_clone_does_not`, `erased_insert_and_remove_reach_the_mirror`, derived-row tests; `a_mirror_attached_after_population_sees_every_instance`; `re_insert_with_different_length_data_still_round_trips_through_world`; frame tests (attach before populate, several viewports, empty level) | late type registration; bulk load; buffers past initial capacity; pool compaction; device recreation; several worlds; generation checks |
| Real render output | `meshes_and_lights_reach_the_frame_from_every_producer`, `environment_components_reach_the_frame`, `foliage_reaches_the_frame`, `every_pass_of_the_editor_graph_runs`; `environment_join_rows.rs`; `toolbar_bloom_toggle_updates_the_resolver_baseline`. Unsupported classes report themselves (`unfinished_components_declare_why_and_their_issue`) | splines have no frame test; unfinished classes #1053–#1060 |
| Mesh/light regression | `meshes_and_lights_reach_the_frame_from_every_producer` (insert, move, hide/show, disable, remove, movable/static, panel open, camera at rest, late geometry); `scene_join.rs` (placement, stale rows, `unchanged_inputs_record_nothing_and_keep_the_outputs`); `a_legacy_flat_light_is_migrated_and_loads` | scale; light-type and intensity variation; shadow flags; inspector closed vs open as a pair |
| Asset behavior | late geometry arrival (frame test); shared pool dedup (`content_dedup.rs`) | missing/corrupt asset; reload; rapid replacement; cancellation; removal while loading; stale completion; property edit causes no reload |
| Persistence compatibility | `a_legacy_flat_light_is_migrated_and_loads`; `records_round_trip_and_keep_undecodable_payloads`; `a_refused_attach_writes_nothing`; `object_props_carry_no_component_copies`; `save_writes_only_overrides_and_load_follows_class_edits` | errors naming object/component/property; package load |
| Runtime parity | `spawn_five_destroy_two_same_in_standalone_and_pie`; `a_level_without_class_instances_runs_no_scripts`; `pie_play_stop_play_leaves_nothing_behind`; `a_replaced_world_is_rescanned`; frame tests for level replacement and late GPU attach | rendered-frame parity for standalone and embedded (needs a window; structural only) |
| Cost and architecture | `unchanged_inputs_record_nothing_and_keep_the_outputs`; `generated_defaults_decode_once_…`; no-op commands push no undo step; `scene_inventory` architecture checks | idle scene does no CPU discovery or JSON (checked structurally, not measured); unrelated edits rebuild nothing; bounded dirty uploads |
| Build and ownership | Sweeps per phase (below); `scene_inventory`; SceneDB with `--features gpu` | GPU checks ran on Mesa lavapipe only; DX12 and hardware unverified |

## Final closure checklist (from the plan)

- [x] Every registered class and every render pass has a completed source/consumer ledger entry.
- [x] Every production component producer uses typed writes after any external decode.
- [x] Disabled and duplicate components have one typed authority and stable identity.
- [x] No internal history/class/script path uses JSON to clone or update known live values.
- [x] No renderer locks/scans the CPU scene to discover or project render components. The renderer locks the scene to flush the mirror, pace frames, pick and select. Its listed queries are not projections; the voxel path is outside this plan.
- [x] No renderer state-change subscriber, manual refresh/arming hook, or force-resync repair remains.
- [x] No undrained component write queue or orphaned behavior producer remains.
- [ ] GPU reflection covers all mutation, removal, attachment and buffer lifecycle paths generically. Mutation, removal and attachment are covered; buffer growth, compaction and device recreation have no test (#1081).
- [x] Every subscriber independently observes state and can recover after lag/world replacement.
- [ ] Every supported component's effect is demonstrated through the actual editor and runtime path. Splines and the standalone/embedded runtimes have no frame test (#1081). Unsupported classes report themselves.
- [x] Remaining serialization and genuine event streams are explicitly classified and documented.
- [x] Historical migration claims, examples and tests agree with the final architecture.

## Revisions

- SceneDB: `1981d7ccf5607ffe816d6f123f09934ec06ad7c2`, pinned by the root `Cargo.toml` (dependency and `[patch]`).
- Helio submodule: `61612f94`.
- pulsar-reflection submodule: `2761936b` (unchanged).
- Helio's own `Cargo.toml` still pins SceneDB `8f98f83`. Inside Pulsar, the patch resolves it to `1981d7c`. A standalone Helio build uses the older pin.

## Left open (recorded, not done here)

- **Acceptance gaps** above: Pulsar-Native#1081.
- **`pulsar_reflection` residue:** `ComponentRuntimeBehavior`, `RuntimeBehaviorRegistration`, `apply_runtime_behavior_for_class` and `ScenePropsProjector`'s projection side are still defined in the separate `pulsar-reflection` repository, which this session cannot change. Pulsar calls no dispatch, and the architecture check fails if one returns.
- **Helio's standalone SceneDB pin** (above).
- **The voxel path** reads the scene when the world revision changes (`VoxelSceneRead`). It is outside this plan; [voxel-branch-porting.md](voxel-branch-porting.md) lists what the voxel branches must change.
- **Unfinished classes, passes and buffers** keep their issues: #1053–#1060, #1063, #1066, #1079, #1080.

## Sweep

Running; results are added when the sweep finishes.
