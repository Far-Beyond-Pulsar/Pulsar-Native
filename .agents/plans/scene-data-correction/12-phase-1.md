# Phase 1: typed instance storage and intrinsic write semantics

Status: **Stages 1 and 2 landed** (Pulsar-Native#1035). Builds on the Phase 0 closure ([11-phase-0-closure.md](11-phase-0-closure.md)). D1 (one SceneDB entity per attached component instance) is adopted as approved; the `REVIEW:` recommendations recorded under D1 are taken as the working answers (see [D1 answers](#d1-answers-adopted)).

Ground rule, unchanged from Phase 0: nothing here adds a subscription, refresh, mark or resync to make data visible. Each step removes a workaround by making the write path itself correct, and the ledger rows move with it.

Phase 1 exit (from the plan): a component created by a generic factory and inserted through erased APIs behaves identically to a direct typed insert, including mutation and removal, with no render-specific helper call; disabled and multiple instances, and late mirror attachment, are represented correctly.

## Stage 1: intrinsic GPU reflection and erased values (landed)

### SceneDB upstream

SceneDB branch `claude/cool-hypatia-ict23g` at `358ec86` (main `f18ea6e` plus one commit; not yet merged upstream, so the root `Cargo.toml` pin says so):

- `World::insert_dyn(entity, Box<dyn Any + Send + Sync>)` and `World::remove_dyn(entity, ComponentId)`. Typed `insert`/`remove` and the erased pair share one implementation (`insert_core`/`remove_core`): GPU mirror dispatch and auto-registration, handle ledger, change tracker, journals and subscriptions cannot drift apart. A type becomes erased-insertable through `register_component::<T>()`, which captures its column constructor; erased insertion of an unregistered type, or on a dead entity, returns the value in an error.
- Bundle spawning goes through the same first-insert path, which also gives it the handle-ledger acquisition it lacked.
- `attach_gpu_mirror` initializes the mirror from the world's current contents, for every component type with a GPU dispatch, as first inserts (including `Once` fields and liveness rows). Re-attaching a clone of the attached handle is a no-op; a new handle (new store, recreated device) is replayed into.
- `gpu::write_derived_row::<M>` / `clear_derived_row::<M>`: an authored component's own dispatch can write a derived GPU type's row.

Tests (`crates/pulsar_scenedb/tests/`): `world_erased_insert.rs` (typed and erased writes leave identical values, archetypes, ledger counts, journal entries and subscription events; refusals change nothing), `world_gpu_mirror_replay.rs` (populate-then-attach for per-field, packed, `Once` and var-len fields plus liveness; re-attach; erased insert/remove; a derived row following its authored component). The full `--features gpu` suite passes on lavapipe except `alloc_gate_gpu::view_token_buffers_upload_alloc_count_independent_of_token_count` and `world_gpu_mirror_reservation_shrink` (a 16 GiB host allocation); both fail identically on unmodified SceneDB under lavapipe.

### Pulsar-Native and Helio

- **Mirror replay list removed.** `helio_bridge::ensure_gpu_mirror` keeps its capacity registrations and attaches the mirror; its snapshot-and-reinsert list of eleven types is gone.
- **GPU companions are internal.** `#[engine_class]` still generates the `{Struct}GpuMirror` layout, but it is no longer a component. The authored struct gets its own SceneDB GPU dispatch that derives the companion and writes its row, plus the matching clear, so the row follows every typed or erased insert, guarded write, removal, despawn and replay. Removed: `GpuMirrored::sync_gpu_mirror`/`remove_gpu_mirror`, `#[register_world_component(gpu_mirror)]`, `refresh_gpu_mirror`, `WorldComponentRegistration::refresh_gpu_mirror`, `refresh_world_component_gpu_mirror_for_class` and all four of its callers (properties panel, script setters and methods, `mark_render_components_changed`). A `scene_store` struct mirrors through its own `SceneStore` derive and gets no companion (before, its scalar `#[gpu]` fields registered two dispatches for one component and one was silently dropped).
- **Erased registry values.** Every registration now carries a default factory, a JSON boundary decoder, a clone and its SceneDB erased registration (`pulsar_world_registry::values`). Hydration is decode + `World::insert_dyn`; nothing class-specific happens at insertion. `insert_world_component_value` checks the value's type against the class before writing.
- **Property writes are one write.** `set_world_component_property` applies a reflected setter and the class's `property_written` normalization under one SceneDB guard; the properties panel and script natives use it. `StaticMeshComponent` loads its mesh asset there only when `mesh_asset` was written, and in its decoder; `LightComponent` keeps only a decoder (nested and legacy flat shapes). The light's `general.enabled` is now part of its GPU row, so a disabled light keeps its row marked absent.

Tests: `engine_class_derive/tests/gpu_mirror_derive.rs` (a companion row follows insert, guarded write and removal with nothing else called; factory + erased insert, boundary decode and typed insert land identical values and GPU rows; a property write and its normalization are one change event and reach the GPU row), `helio-component/tests/light_component_gpu_mirror.rs` (insert, edit, disable, removal and populate-before-attach, on the GPU), `pulsar_world_registry` unit tests. `cargo test -p scene_inventory` passes with the ledger updated: four `gpu-refresh` site rows removed, three `mirror-replay` rows `verified`, eight classes now own GPU columns, the companion rows re-described, and fourteen schema rows added for the authored dispatches (six of them inert sub-props registrations, `out-of-scope`).

Found and fixed on the way (pre-existing, blocking the editor test suite): `execute_command(DuplicateObject)` with a position offset deadlocked (the AI-tools test hung): scene guards taken in two `if let` scrutinees lived through blocks that lock the scene again.

Pre-existing failures left as found (not caused by this work, reported for their owners):

- `helio_component` unit test `mapping::tests::mirror_carries_color_and_intensity_with_a_zeroed_position_placeholder` expects raw intensity 42 while the mapping applies the lumens conversion added in Helio `0cb21676`.
- `helio_component` test binary `static_mesh_component_gpu_mirror` did not compile (it used `helio::Scene`, removed in Helio `c9eb57b1`, and predated the material-slot fields). Since Phase 0 made `helio_component` a workspace member, CI's `cargo check --all-targets` reached it; ported in Stage 2 (its two World-level cases pass on lavapipe, the `helio::Scene` case is removed with that API).

## Stage 2: component instances as entities (landed)

The data model lives in `pulsar_scene_model::attachments`; class-aware attachment and the JSON record boundary in `pulsar_world_registry::instances`.

| On | Component | Purpose |
|---|---|---|
| instance entity | the registered value, typed (enabled or not) | the one authority for the instance's data |
| instance entity | `UnresolvedComponent { data, reason }` instead of a value | unregistered class or data that does not decode: kept verbatim, explicitly not live |
| instance entity | `ComponentOwner { owner_index, owner_generation, enabled }` | owner link and enabled flag; a packed GPU row (`component_owners`) keyed by the instance, the owner-key join for Phase 2 |
| instance entity | `ComponentMeta { id, class_name, parent, class_slot }` | stable `ComponentInstanceId`, presentation parent, class-slot provenance |
| owner object | `ComponentAttachments(Vec<Entity>)` | order (presentation only) |

Rules: attaching validates everything before writing (a refused attach leaves nothing behind, and never an instance that looks attached without its value); loaders and history restore, which must not lose data, attach undecodable payloads as `UnresolvedComponent` explicitly, while an interactive add or data edit that does not decode is refused; despawning an object despawns its instances; lookups that need exactly one instance of a class return an ambiguity error instead of picking the first; JSON records (`ComponentInstance`, carrying the instance id and parent index as `__`-keys) exist only at file, history and tool boundaries (`attach_records`/`replace_records`/`component_records`, `set_instance_data`).

Every consumer that read a component value from the object entity now reads the instance entity and joins its owner through `ComponentOwner`; nothing reads both representations.

- **Registry and scripts.** `resolve_instance` addresses a component as the instance itself or an owner plus class-local ordinal; dispatch, property natives and `instance_of` use it. The script VM's component adapters (`X::of`, `exists`, `entity`, reflected fields and methods) resolve through a `ComponentAddressing` hook that the world registry supplies (`attachments::holder_of`/`object_of`), so `X::of(object)` reaches its first instance and `X::entity()` returns the object; `script_component!` types keep direct addressing. The tick shim queries `(&mut T, &ComponentOwner)`, skips disabled instances and runs callbacks with the owner. Component events are published on the instance channel and on the owner's (as they were when both were one entity). `pulsar_script_object_model` lost its own instance store and routing (`instances.rs`, `routing.rs`): access goes through registry dispatch.
- **Classes and the script driver.** `pulsar_class` keeps `ClassInstance` and slot components as instances (`SlotHandle.entity` is the instance); the driver maps `ClassInstance` changes back to their object, including for a holder already despawned. Generated actors attach absent prefab components as instances (`attach_record`) instead of hydrating onto the actor entity.
- **Engine backend.** The level loader attaches records (`attach_records`); the render-row projection, subscriptions and dirty mapping work per instance (`dirty_render_instances`: a mesh/light/`ComponentOwner` event names its instance, an owner `Transform`/`Visibility` event dirties its render instances); `light_frame.rs`/`mesh_frame.rs` (unused per-entity frames) are deleted; voxel projection, source sessions and brush commits address the terrain instance and place it by the owner's transform. The project template attaches its cube mesh with `attach_value`.
- **Editor.** `scene_edit::components` addresses instances by list index: enable, reorder, parent and duplicate are metadata operations on the instance, never a JSON round trip; property edits run the reflected setter on that instance's own value. Removed: the `RenderProps.component_instances` projection and `sync_registered_component_props_to_scene_db` (14 ledgered call sites), the "first enabled instance is live-typed, duplicates are JSON" split (#519) with its scratch-world edit path, and the JSON retained on a failed hydrate. History snapshots keep instance ids, so a restore keeps component identity; voxel stroke journals name the terrain instance by id. Object duplicates copy instance values (`duplicate_instances`), not records.

Ledger: the `json-hydrate` rows for runtime level, `pulsar_class`, generated actors and the editor became `record-attach` rows (a new site pattern for the remaining JSON record boundary, plus a row for the registry's own); the editor row, Phase 0 `broken`, is `unverified` (the baseline's legacy flat light payload is now refused instead of staying attached); the `render-props-sync` rows (classes, components, objects), the classes `add-component` row and the script routing `json-hydrate` row are removed with their code; `property-edit` in the editor 4 → 1, `subscribe` in `helio_bridge` 6 → 5; `component_owners` buffer and `ComponentOwner` schema rows added.

Tests: `engine_class_derive/tests/component_instances.rs` (factory/decode/typed equivalence on instances, several and disabled instances, a records round trip with unresolved payloads and parents, a refused attach writes nothing, mirror replay including `ComponentOwner` rows on lavapipe), `pulsar_scene_model` attachment tests, and the migrated suites: `engine_backend` lib, `ui_level_editor` lib, `pulsar_game` (lib, `memory_over_ticks`), `pulsar_class`, `pulsar_script_object_model`, `pulsar_world_registry`, `pulsar_script_vm`, `pulsar_script_codegen`, `helio_component` (`content_dedup`, `light_component_gpu_mirror`), `scene_inventory`. `phase0_render_baseline` passes on lavapipe and reproduces the Phase 0 mesh table case for case; the "added later" render failures are Phase 2's.

Open points carried forward:

- The tick shim's `query::<(&mut T, &ComponentOwner)>` writes through SceneDB's query path, not the `get_mut` guard; whether that path reports writes to every hook is Phase 3's typed-write audit.
- Disabling a `ClassInstance` instance does not stop its script (the driver watches `ClassInstance` changes, not `ComponentOwner`); no editor path disables it today.
- Pre-existing failures seen while testing, not caused by this work: `engine_backend` `interaction::tests::press_captures_without_hover_and_drag_continues_off_handle` (gizmo hover; this change does not touch the gizmo code, not re-run on main), `pulsar_script_vm --test events` `declared_events_are_verified_locally` (fails identically with the VM changes reverted), two `ui_level_editor` doctests in `toggle_button.rs`, and the `plugin_editor_api` lib test and `ui` `cached_scrollable_panels` test, which do not compile against the current gpui. `helio_component`'s `content_dedup` looked up a stale pool key (`StaticMeshComponent::vertices` instead of the declared `builtin_mesh_vertex`); fixed here.

## D1 answers adopted

| `REVIEW:` question (01-authority-and-identity.md) | Answer used |
|---|---|
| Cascade on object despawn | Automatic, in the same operation (`despawn_tree` despawns owned instances) |
| Nesting | Presentation-only metadata (`ComponentMeta::parent`), cycle-checked, same owner only |
| ID format and scope | Opaque 128-bit (`ComponentInstanceId`), random, unique per scene, hex string in JSON |
| Script reference shape | The component-instance entity/id; an object+type facade errors on ambiguity |
| World/global components | Allowed on a world entity, same lifecycle (no special case needed) |
