# Phase 3: editor, class and script producers on typed values

Status: **landed** (Pulsar-Native#1035). Builds on Phase 2 ([13-phase-2.md](13-phase-2.md)).

Phase 3 exit (from the plan): internal add/edit/clone/history/class/script paths have no serialize/hydrate round trip, no split authority and no instance-zero aliasing. Every remaining JSON use in the scope ledger has a documented boundary or unresolved-payload purpose.

| Exit criterion | Evidence |
|---|---|
| History (undo/redo, play mode) has no round trip | `commands/tests.rs` `undo_restores_component_instances_in_place`: undo and redo keep every instance's entity, id and order; an unresolved payload survives a remove and undo |
| Add and edit have no round trip | `commands/tests.rs` `typed_component_commands_are_undoable_and_report_no_ops` (typed `AddObjectWithComponents`, `AddComponent`, `SetComponentData`; values of another class refused; no-ops push no undo step); `render_acceptance.rs` panel add path |
| Clone has no round trip | Phase 1's typed `duplicate_instance(s)`; history snapshots clone through the class registration |
| Class building has no round trip | `scene_edit/tests/classes.rs` `instances_are_built_from_the_typed_template`: one template per definition, nested override diffs land on the reflected property, the placed instance carries the override |
| Script paths have no round trip | Spawning clones the cached class template (`pulsar_game` suites); `ComponentRef::get/set_property` are typed (`pulsar_script_object_model` suites); generated actors decode a prefab default once per class (`component_dispatch.rs` `generated_defaults_decode_once_and_each_actor_gets_its_own_value`) |
| No split authority | Object `props` no longer hold copies of component values (`scene_edit/tests/components.rs` `object_props_carry_no_component_copies`); stale copies in old levels are removed at load |
| Old shapes migrated, unknown data preserved, invalid known data reported | `pulsar_class::records` tests; `runtime_level.rs` `a_legacy_flat_light_is_migrated_and_loads`; the editor's load warns for each registered-class payload that still does not decode |
| Every remaining JSON use is documented | `cargo test -p scene_inventory`; no Phase 3 ledger row is left `at-risk`. `NativeScriptComponent` stays `unverified` because nothing consumes it yet (see open points) |

## Decisions

The defaults stated when Phase 3 started, as implemented:

- **Undo is typed and in memory.** History and play-mode snapshots hold `InstanceSnapshot`s: clones of the live values, through the class registration's `clone_value`. Restoring reconciles each object's instances by id, in place.
- **The `.level` JSON stays the file boundary.** A versioned archive codec is deferred. Load migrations run on the raw JSON before anything is decoded.
- **Class overrides: changed from the stated default.** The default was to persist overrides by field name, with aliases. Overrides instead keep their persisted form, a JSON diff per slot against the slot default. The diff is applied through the reflected setters, using where each property sits in the class's serialized shape. Two reasons:
  - Levels need no override migration.
  - Reflection has no alias metadata to build aliases from.

  A diff that names a field no reflected property covers is applied by decoding the patched default, as before, so nothing an override says is lost.
- **Erased writes are `Box<dyn Any>`.** They go through reflected setters, then the class's `property_written` (unchanged).

## Stage 1: history and editor producers

`pulsar_world_registry::instances`:
- `InstanceSnapshot { meta, enabled, value: InstanceValue }`, with `InstanceValue` either `Value(Box<dyn Any>)` or `Unresolved(UnresolvedComponent)`;
- `snapshot_instance(s)` and `restore_instances`. Restore detaches instances the snapshot lacks and writes kept instances' values back through the class's insert, so every write hook runs. It re-attaches missing instances with their id, then applies order, enabled flag, slot and parents.
- `set_instance_value` replaces one instance's value in place.

Editor (`ui_level_editor`):

- `SceneHistorySnapshot` holds typed snapshots. Undo no longer re-decodes every component, which used to reload meshes from disk.
- Commands:
  - `AddComponent { value: Option<Box<dyn Any>> }`: no value means the class default.
  - `SetComponentData { data: ComponentData }`: a typed value, or a new payload for an unresolved instance.
  - `AddObjectWithComponents`: an object and its typed components in one undo step.
- Component edits report whether they changed anything. The fingerprint that encoded the whole component list to JSON twice per command is gone. `SetMovability` reads the typed movability.
- These now go through commands (one undo step each). Before, all of them bypassed undo:
  - the properties panel's Add Component (it built a flat JSON map from a default instance; it now adds the class default);
  - the viewport asset drop (object and mesh in one step; Helio's `AssetComponentRegistration::value_for` builds the typed mesh);
  - the component hierarchy's toggle, duplicate, delete, nest and reorder.
- The AI tools keep JSON at their protocol boundary and decode once there; the commands they issue are typed.
- Typed values for creation and edits:
  - curves are written as typed `SplineComponent` values;
  - the voxel terrain is created with a typed value;
  - foliage instances are created with `StaticMeshComponent::for_mesh_asset` (the geometry loaded, as decoding `{"mesh_asset": path}` did).
- `get_object`/`get_all_objects` no longer copy every component's JSON into the object's `props`. Nothing in the editor read those copies, but they caused three problems:
  - the save wrote them;
  - for lights they were defaults, because the light projector reads flat keys from nested data;
  - `update_object` wrote them back into `RenderProps`.
- Unresolved component cards show their own payload. They read the metadata-only records before, so they always showed class defaults.

## Stage 2: typed class templates

- `pulsar_class::template`: a `ClassTemplate` is a definition with every slot default decoded once. It is cached by definition fingerprint and rebuilt when the prefab changes. `slot_value(slot, overrides)` clones the typed default and applies the override diff through reflection (`apply_diff`, `PropertyLayout`). A slot with no typed default yields an unresolved payload, the patched default data.
- Expansion builds instances from the template with typed slot provenance. Nothing is decoded per instance, and a mesh slot's asset is loaded once per class definition instead of once per instance.
  - `expand_roots` (level load) reads each class once per call.
  - The script driver caches templates per class and drops them on a class reload or a registry rescan. `world::spawn` used to read the prefab from disk and decode every slot on every spawn.
- `WorldComponentRegistration` can view an owned value as `EngineClass` (`value_as_engine_class[_mut]`). `set_value_property` writes a property on an owned value and runs `property_written`.
- The editor's class slot defaults read the template instead of decoding the default JSON per object on every panel refresh.

## Stage 3: load migrations and API removal

`pulsar_class::records` runs inside `migrate_level_value`, which the editor loader, the runtime loader and `level_migrate` share. It runs in this order and reports each change:

1. `MaterialOverrideComponent` folds into the mesh's `legacy_material_override`. Only the runtime did this before, ad hoc; the editor loader did not.
2. Data saved as flat reflected properties for a class that nests them in `#[sub_props]` groups is rewritten to the class shape. The 2026-10-04 flat light used to fail the runtime load; it now loads with its saved intensity.
3. A bare `props.mesh_asset` on an object with no component list becomes a `StaticMeshComponent`. This moved out of `add_object`, where every add, including duplicates of objects carrying a stale copy, could conjure a mesh.
4. Component copies a registered scene-props projector manages are removed from object props.

Data of a registered class that still does not decode is reported:
- the editor keeps it unresolved and logs a warning naming the object, class and reason;
- the runtime refuses the level, as before.

Removed or narrowed:
- `set_instance_data`.
- The editor's `update_component` and `update_component_property`. An unresolved payload is edited by `set_unresolved_property`, with nothing decoded.
- `json_args_to_method_args`, which had no callers.
- `ComponentRef::get/set_property` are typed. The registry's JSON property accessors remain as a documented tool-boundary API.
- Generated actors attach prefab components through `attach_cached_default`: one decode per class, a clone per actor.

## Tests

All on Linux; GPU cases on Mesa lavapipe:

- `ui_level_editor` lib: the history, command, class, component, spline, AI tool and HLFS tests above.
- `ui_level_editor --test render_acceptance` passes. Its light case now adds the light through the panel's real path: `AddComponent`, then the intensity edit.
- `pulsar_class`: template, planner, record migration and instance tests.
- `pulsar_world_registry`, `pulsar_script_object_model`, `pulsar_script_codegen`, `pulsar_game`, `engine_backend`, `level_migrate`, `scene_inventory`.
- Known failures that predate this work: the `engine_backend` gizmo hover test and the `helio_component` light-mapping intensity test (both recorded in Phase 2).

The sweep's results are recorded in the PR.

## Branches

- Pulsar-Native `claude/cool-hypatia-ict23g-phase-3`, stacked on Phase 2.
- Helio `claude/cool-hypatia-ict23g-phase-3` at `50aaabf5`, on Phase 2's branch: the asset registration change.
- SceneDB is unchanged; the pin stays Phase 2's `999373e`.

## Open points

- `collect_overrides` encodes each class slot's live value to diff it against the default. That is right at save, but the details panel's class view also runs it after each change to the instance.
- A foliage stamp captures a full-scene history snapshot (a typed clone of every component) for its single undo step; a delta snapshot covering only the new objects would do.
- Plugin-only component classes (no `WorldComponentRegistration`) still cannot be added; the panel's Add Component refused them before too.
- `NativeScriptComponent` has no runtime consumer. The editor's record helpers for it are dead code.
- `pulsar_scene::format`'s light accessors (the deprecated `SceneLoader` path) still read flat keys through the scene-props projector. That is Phase 6 residue.
- `add_component` (JSON) remains for tests and the HLFS demo generator; the level loaders use `replace_components`/`attach_records`.
- Override aliases (a renamed property) are not supported; reflection carries no alias metadata.
