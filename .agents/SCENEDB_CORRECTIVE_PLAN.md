# SceneDB corrective plan: typed state through to rendering

Review this initial plan alongside the contract packet in [plans/scene-data-correction/README.md](plans/scene-data-correction/README.md). The packet contains proposed decisions for identity, typed writes, GPU consumption, notifications, persistence, modules, and runtime lifecycle; none is approved until reviewed.

Contract clarification: components own data and may opt fields into SceneDB GPU reflection/upload. They never produce pixels or declare rendering capabilities; the renderer chooses how to consume the uploaded buffers. Concrete pass references belong only in the graph owner.

Status: proposed implementation plan, based on source and runtime-log inspection on 2026-10-04. No implementation phase below is complete merely because an earlier migration document says it is.

## Objective and completion rule

Adding or changing a component through the editor, scripts, plugins, or runtime must change its actual value in SceneDB. SceneDB reflects GPU fields as part of its normal write lifecycle. Rendering consumes those GPU columns directly. Opening a property panel, subscribing, draining notifications, running a component dispatcher, or invoking a renderer refresh must have no bearing on whether the component renders.

This is a correction of the entire component lifecycle, not a mesh-only repair. Completion requires every registered component and every production mutation entry point to have a documented disposition and acceptance evidence. A successfully loaded mesh asset, an allocated GPU buffer, or a passing isolated mirror test does not prove that a draw consumes the new component.

## Required architecture

1. **One authoritative live value.** Registered component instances, including disabled instances, retain typed or reflection-owned type-erased values in SceneDB. A serialized attachment record must not be a second editable copy.
2. **Typed internal operations.** Factories, property edits, duplication, class instantiation, overrides, undo/redo, and in-process scripting use reflected values. JSON belongs at file, import/export, external tool, or wire boundaries. Decode once on entry and encode on exit.
3. **Automatic GPU reflection.** Typed and erased insert, replacement, mutation, removal, and despawn follow the same SceneDB mutation contract. Callers never arm render subscriptions, mark a component changed to make it visible, or refresh a GPU companion manually.
4. **Direct GPU consumption.** Render passes use SceneDB-owned GPU columns, presence/liveness, generations, and asset references. They do not scan the CPU world to build or update a second scene. Cross-component rendering joins belong on the GPU.
5. **Independent subscriptions.** Each interested UI or script subscriber can observe committed state changes without consuming another subscriber's notifications. Notifications prompt a read of current state; they are not a second state store or a prerequisite for rendering.
6. **Consistent identity.** Editor, reflection, scripting, class slots, history, and GPU consumers resolve the same component instance. Reordering a list must not silently change the target of a saved reference.
7. **Honest failures.** A failed insertion or unsupported render component must not appear to have attached successfully because a JSON record was retained. Surface a typed error or an explicit unresolved/unsupported state.

### Mechanisms that remain valid

- SceneDB dirty tracking, staging, upload coalescing, and GPU submission are necessary parts of GPU reflection. They must not depend on consumer event queues.
- GPU culling, batching, compaction, lighting, shadow caches, and simulation outputs are derived render work. They may own transient resources without owning a second editable scene.
- Asset decoding, asynchronous loading, geometry generation, physics, and other simulations still perform real work. Their results enter the authoritative data/resource lifecycle through ordinary writes.
- UI expansion, widget drafts, camera controls, and selection presentation remain UI concerns; committed scene values remain in SceneDB.
- Files and external protocols can continue using JSON. Unknown serialized types can retain opaque payloads for lossless round trips, explicitly separated from resolved live components.
- Genuine gameplay/input/lifecycle events can remain ordered events. State-change subscriptions must not silently promise event-history semantics. Audit callers that depend on every intermediate change before selecting coalescing rules.

## Baseline and confirmed evidence

Audit baseline: Pulsar-Native `7fe4a7441a1720ffccc6bf686b0561137758cbe1`; Helio submodule `05c2f7d7`; SceneDB dependency revision `e74df984b95ffe5235a67653a5b95a07e579df47`. Working-tree changes already present in the command executor and viewport asset handler add render subscription/refresh workarounds. They are not the target design.

Paths below are relative to this repository. Symbols are the reference points because line numbers will move during implementation.

| Finding | Evidence | Required correction |
|---|---|---|
| Three consumers compete for one destructive notification drain. | `engine_backend/src/subsystems/render/helio_renderer/renderer.rs` calls `take_component_change_events`; `ui_level_editor/src/core/scene_edit/components.rs::take_world_component_events` feeds the properties panel; `pulsar_script_object_model/src/subscribe.rs::take_change_events_for` drains everything before filtering one subscription. The script helper's claim that other subscriptions are preserved contradicts its implementation. | Remove renderer dependence; give every remaining subscriber independent delivery. |
| Mesh geometry reflection does not establish the rows the draw path consumes. | `helio-component/src/components/static_mesh_component.rs` mirrors geometry pools. `engine_backend/src/scene/helio_bridge.rs::sync_static_mesh_rows` manufactures separate material/static-object rows. `helio-pass-object-batch` consumes `static_objects`; `helio-pass-gbuffer` consumes `builtin_mesh_vertex` and `builtin_mesh_index` plus batch/material resources. | Define and implement the complete authored-column-to-shader contract, including presence and draw eligibility. |
| Light authoring and GPU light data have separate projection paths. | `scene/editor_rows.rs::sync_editor_light_rows`, authored `LightComponent`, its generated/custom GPU companion, forward-lit light rows, and billboard rows. | Establish one authored value and a direct reflected GPU contract for lighting and editor visualization. |
| Component hydration already fails in the running editor. | Log `C:\Users\redst\AppData\Roaming\Pulsar\Pulsar_Engine\data\logs\2026-10-04_19-39-57\engine.log` reports `World hydration failed for LightComponent` and a floating-point intensity where `IntensityLightProps` is expected. | Remove live JSON construction; migrate legacy flat file schemas at the import boundary. Preserve diagnostics. |
| Mesh assets load despite missing visible output. | The same log reports loading `SM_Cube.fbx` with 24 vertices and 36 indices. | Trace through draw eligibility and pixel output; do not equate loader success with rendering success. |
| GPU companion refresh is caller-dependent. | `engine_class_derive` emits `GpuMirrored`/refresh callbacks; `pulsar_world_registry`, script natives, scene edits, asset updates, and command workarounds explicitly refresh or mark values. | Connect generated layouts to SceneDB's intrinsic mutation lifecycle for all write forms. |
| Late GPU attachment uses a finite hand-maintained replay list. | `scene/helio_bridge.rs::ensure_gpu_mirror` registers and reinserts selected component types; the pinned SceneDB `attach_gpu_mirror` sets the handle without replaying existing rows. | Add generic schema-driven initialization of all existing GPU-bearing state. |
| Several render components still enqueue writes for a removed execution path. | `helio-component/src/subsystems.rs::PendingWorldWrites`; producers in water, reflection capture, post-process volume, camera post-process, portal, foliage, and fog behaviors. No production drain caller was found outside that crate. | Replace each producer with a direct data contract; retire the queue and obsolete behavior hooks after coverage exists. |
| JSON remains inside live storage, history, classes, and scripts. | `pulsar_scene_model::{RenderProps, ComponentInstance, ComponentAttachments}`; editor `scene_edit`; `pulsar_class::{world, overrides, component}`; script object-model access/routing. | Migrate values, identity, cloning, and overrides together rather than just changing command argument types. |
| Editor and runtime render preparation differ. | Render bridge calls in editor renderer, `pulsar_game/src/windowed_app.rs`, and `pulsar_game/src/embed.rs`. | Use the same reflected GPU source in all modes. |

These are confirmed architectural defects and observed symptoms. The exact cause of every invisible object has not been proven by an end-to-end capture. Movability affects shadow classification and cache invalidation; the inspected flag handling does not by itself establish that movability caused all missing draws.

## Scope ledger

All rows are in scope. This is the minimum confirmed inventory plus explicit audit obligations, not a claim that a text search has enumerated every dependency. Phase 0 produces a closure ledger with one row per registered class, mutation entry point, subscription consumer, and render pass.

### A. Storage, reflection, and ownership

| Surface | Work required |
|---|---|
| `crates/core/pulsar_scene_model/src/{components,instance,attachments,world_ext}.rs` | Remove `RenderProps` and attachment JSON as live component authority. Keep attachment metadata typed: stable instance identity, owner, order, enabled state, parent, and class-slot provenance. Audit free-form object properties and payload catalogs individually. |
| `crates/core/engine_class_derive/src/lib.rs` | Supply generic typed/erased factory, insertion, clone/drop, and GPU metadata integration. Nested `sub_props`, collections, enums, custom properties, and mutation through reflected methods must obey the same rules as direct writes. Generated GPU companions must be internal reflection data with automatic lifetime, not caller-managed editable components. |
| `crates/core/pulsar_world_registry/src/{lib,engine_class_mut,dispatch,marshal,type_shims,script_natives}.rs` | Make typed reflected operations the internal API. Retain named codecs only for boundaries. Remove caller-required mirror refresh. Validate downcasts and exact types before mutation; propagate failures rather than serializing/recreating values. |
| SceneDB upstream `World`, mutation guards, schema registration, GPU mirror/store and buffer registry | Support erased operations without bypassing hooks, generic late attachment, dynamic registration, correct deletion and row reuse, and accurate buffer generations. Version dependency changes upstream and update the root lock/pin; never ship a modified Cargo cache as the fix. |
| Plugin component factories and runtime registration | Audit `crates/core/plugin_editor_api/src/components.rs`, plugin loader, reflection registries, and registration timing. Factories returning `Box<dyn EngineClass>` must have a valid insertion/GPU registration route, including types registered after world creation. Preserve the permanent-DLL lifetime contract and validate ABI/allocator/type identity assumptions. |

**Identity decision required before migration:** the current ECS stores one value per component type per entity while the editor permits multiple instances and stores dormant copies separately. Adopt a coherent instance model, preferably component entities with an owner reference where multiplicity is required. Record the choice and all query/API consequences before Phase 1 exits. Do not preserve a hidden rule where methods operate on instance zero regardless of the referenced instance. Stable component IDs and class-slot IDs must survive reorder, duplicate, disable/enable, save/load, and reference remapping during history restoration. Unknown types remain explicit unresolved attachments; known disabled types remain typed.

### B. Every producer and editor operation

| Surface | Work required |
|---|---|
| `ui_level_editor/src/core/commands/{types,executor}.rs` | Convert `AddComponent`, `SetComponentData`, class variables, and whole-object payloads to owned typed values/patches. Preserve existing typed property commands. Batch compound operations into one validated transaction and undo step. Remove render arming/marking side effects. |
| `ui_level_editor/src/core/scene_edit/{components,objects,mod}.rs` | Eliminate live serialize/hydrate loops, metadata-first attachment success, JSON fallback for known types, and duplicate record synchronization. Cover add/remove, whole replacement, nested property edits, reordering, nesting, enabled state, object duplication, deletion, and clear. |
| `ui_level_editor/src/ui/properties/object_type_fields/*` and component hierarchy | Build widgets from live reflected values and typed descriptors. Remove default-instance JSON maps, serialized panel snapshots used as component state, and the single-drainer contract. Resolve the selected instance consistently; isolate temporary text-entry drafts from committed values. |
| `ui_level_editor/src/core/scene_edit/history.rs` and command history in state | Use owned typed snapshots/deltas and reflection clone support. Cover component removal, disabled instances, class overrides, object subtree restore, selection/reference behavior, and PIE restoration. Share immutable asset data instead of cloning geometry on every property edit. No JSON round trip for in-memory undo. |
| `ui_level_editor/src/core/scene_edit/classes.rs`; `pulsar_class/src/{component,world,overrides,plan,registry,prefab,migrate}.rs` | Decode definitions at asset boundaries; retain typed defaults and typed override values internally. Replace live JSON diff/merge, `resync_typed`, and serialized record fallback. Include placement, generated children, reload, revert property/slot, reset overrides, removed slots, missing classes, and variable overrides. |
| Viewport `helio_viewport/assets.rs`, `core/asset_updates.rs`, `core/{splines,native_scripts}.rs`, level creation helpers | Route asset drop, reload completion, spline authoring, native script attachment, primitives and generated scenes through typed writes. Preserve atomic undo for a drop creating an object plus component. Audit gizmos, paint/sculpt tools, context menus, keyboard actions, clipboard and drag/drop across editors. |
| `ui_level_editor/src/ai/tools/*`, remote editor/plugin APIs | Keep protocol JSON at the external handler. Validate/decode into typed commands there; return errors if no supported typed insertion exists. The internal command executor must not remain a JSON transport because an AI tool uses JSON. |
| `pulsar_script_object_model/src/{access,routing,instances,world_host,reflect,subscribe}.rs`; world-registry dispatch; game scripting | Use typed VM/reflection adapters for in-process access and methods. Remove serialized dormant-instance mutation and live JSON property marshalling. Fix instance-zero routing and observer delivery. Preserve genuine script lifecycle events and explicit external codecs. |
| `engine_backend/src/scene/runtime_level.rs`, editor `level_io.rs`, `pulsar_scene` loaders and packaging | Retain versioned persistence adapters; populate the same typed database model used by editor creation. Cover class expansion, disabled/unknown instances, reference resolution, import errors, save stability and legacy schemas. Audit packaged/runtime loading as well as editor loading. |
| Physics, audio, gameplay and plugin-defined component consumers | Enumerate runtime behavior registrations and all uses of component metadata. Migrate any scene-state JSON path affected by the shared APIs. Genuine subsystem simulation remains valid; no generic renderer dispatcher should be required to make non-render components exist. |

### C. GPU source and consumer contracts

For each class/pass pair, document the canonical CPU fields, generated GPU columns/keys, WGSL layout and stride/alignment, units, entity/instance join keys, presence/enabled semantics, asset ranges, dirty policy, removal behavior, and consuming pass. Record unimplemented capabilities explicitly. An exposed component with a no-op behavior is not a completed integration.

| Domain | Required audit and correction |
|---|---|
| Meshes and materials | Trace `StaticMeshComponent`, `MaterialOverrideComponent`, LOD, section/submesh/material assignments, textures, shared geometry and bounds into object batching and gbuffer. Remove the CPU `StaticObjectComponent`/material projection from `helio_bridge`. GPU draw preparation joins reflected component, transform, visibility and resource columns. Handle missing assets/materials explicitly. |
| Transform, visibility, selection and instance lifecycle | Reflect every render-relevant field, including enabled/presence. Audit world-space transform conventions, normals, negative/nonuniform scale, hierarchy semantics, bounds, picking IDs, outlines and gizmos. Prevent zero-filled removed rows or recycled indices from becoming live draws. |
| Movability and shadows | Unify authored movability with shader-consumed classification; remove `project_movability` as a second CPU state copy. Cover mesh and light static/movable transitions, changed static geometry/transforms, deletion and shadow-cache invalidation. Preserve intended shadow behavior without gating basic component existence. |
| Lights and billboards | Unify authored sub-properties, GPU layout, world position/direction, intensity/color units, types, visibility, shadow flags, lens flare and editor icons. Remove `EditorLightRows` and CPU light/billboard reconstruction. File compatibility handles the observed flat-versus-nested intensity mismatch. |
| Water | Replace `water_volume_component.rs` queued scene writes. Identify volume/mask input and water simulation/render consumers; retain simulation outputs as derived GPU work. |
| Fog | Replace both fog component paths in `fog_component.rs`; map their rows to volumetric-fog consumers and blending/volume semantics. |
| Foliage | Replace `foliage_component/runtime.rs` queued writes; map placement, mesh/material references, wind/interaction and generated instance outputs. |
| Portals | Replace `portal_component.rs` queued writes; define linkage, transform, liveness and portal-culling inputs. |
| Reflection captures | Replace `reflection_capture_component.rs` queued writes; separate authored capture settings from generated capture textures/probe work. |
| Post-processing | Replace `post_process_volume_component.rs` and `camera_post_process_component.rs` queued writes. Audit `scene/editor_postprocess.rs`, volume selection/blending, camera settings and render-graph inputs. |
| Voxels, terrain and splines | Audit `scene/{voxel_frame,voxel_source}.rs`, voxel component runtimes/world adapters, generator schemas, edit journals, spline data and their GPU consumers. Retain real generation/sculpting work; remove any redundant scene snapshots/JSON handoffs. Include incremental completion and stale async results. |
| Sky, decals and remaining pass schemas | Inventory every pass-owned SceneStore schema, registry entry and old scene API, including sky/sun, decals and optional graphs. Establish an authored source and consumer or explicitly mark unsupported functionality. This row must be expanded into named entries during Phase 0. |

Also audit `engine_backend/src/scene/{helio_bridge,editor_rows,mesh_frame,light_frame,render_resources}.rs`, Helio's scene/resource APIs, `helio-core` scene inputs/buffer registry, `helio-default-graphs`, and all pass bindings. Old no-op compatibility types and misleading ownership comments are removal work, not proof that migration is finished.

### D. GPU lifecycle and resource correctness

- Writes through direct `World` APIs, erased reflection, nested setters, collection edits and reflected methods must produce equivalent GPU state. Mutable access must not escape without committing dirty tracking. Reads must not trigger uploads or synthetic mutation notifications.
- Use generated schemas for packed fields, nested sub-properties and variable-length pools. Audit padding, scalar widths, enum/bool representation, optional data and layout hashes. CPU and WGSL declarations must have a checked contract.
- Distinguish component-instance identity, entity generations, pool offsets and asset content identity. No consumer may assume an object index always equals every component's GPU row.
- Initialize all existing GPU-bearing rows when a mirror attaches after world population; also support registration after attachment, empty worlds, bulk load, mirror recreation/device reset, capacity growth, reallocation, compaction and world replacement.
- Buffer epoch/content generations must invalidate bindings and derived work correctly. Static/once upload policies must support asset replacement and edits without producing stale geometry or rewriting immutable assets every frame.
- Asset loading publishes typed handles/data through the normal write path. Completion must verify the target world, instance generation and requested asset version. Failed or cancelled loads cannot resurrect removed/replaced components.
- Transform, visibility, material and movability edits must not re-read mesh files or clone complete geometry merely to refresh a mirror.
- Keep graphics-free database use valid. A GPU mirror is optional; attaching one later must not require editor repair scans.

### E. Subscribers, scheduling and world transitions

- Replace global destructive drains with independent cursor/subscription delivery. SceneDB already exposes `open_change_cursor`/`read_changes`; evaluate its erased-type support, filtering and overflow behavior before extending it. `pulsar_game` class scripting already uses a change cursor and is a useful existing pattern.
- Specify initial read plus subscription without a missed-update race, commit ordering, bounded retention, coalescing rules, overflow/resnapshot, unsubscribe, removal notification, and world replacement. Each subscriber must independently recover to current state.
- Migrate properties panels and script subscriptions. Audit all other drain callers, tests and exported helpers. A queue drained once and filtered for one subscriber is invalid; a single dispatcher would only be acceptable if it actually fans out independently, with explicit lifecycle ownership.
- Remove renderer subscriptions entirely. Do not replace them with renderer change cursors, a renamed synchronization service, a polling scan, or a generic per-frame component dispatcher.
- Standardize world commit, mirror flush and render submission ordering in editor, standalone, embedded runtime and PIE. Asset completion, GPU-generated work and pending draw-count readbacks must not stall because CPU world revision is unchanged.
- Cover first frame, idle wake-up, background viewport, multiple viewports, play/stop, level replacement and device recreation. No force-resync flag or opening an inspector should repair missing scene rows.

## Ordered implementation work

### Phase 0 — Close the inventory and establish the failure baseline

Deliver an executable inventory of registered components/reflection factories, world schemas, mutation entry points, subscriptions, GPU-upload schemas and renderer consumers across the parent repository, Helio and SceneDB. Give each row an owner module, target contract, test and status. Resolve the component-instance model and renderer-side data consumption before downstream API migration.

Capture a minimal real editor reproduction for adding a mesh and a light, with the properties panel both closed and open. Record typed presence, GPU presence/layout/ranges, draw eligibility, batch count and final pixels. Capture the light hydration failure separately from mesh draw failure. Use existing diagnostics or temporary instrumentation that reads state; avoid installing another synchronization path.

Resolve test invocation/workspace ownership: the attempted root `cargo test -p helio_component --test static_mesh_component_gpu_mirror` is currently rejected because that package requires dev-dependencies and is not a workspace member. Identify valid parent, Helio and upstream SceneDB test/build commands and GPU CI capability rather than treating unavailable tests as passed.

**Exit:** every exposed component has a disposition; reproduction demonstrates the broken stage; instance identity and layout ownership are written down; runnable validation targets are known.

### Phase 1 — Establish typed instance storage and intrinsic write semantics

Implement the agreed instance model, erased reflected value ownership, factory/insertion/clone/drop operations, and transactional mutation. Validate complete compound operations before publishing them. Keep schemas, stable persistence IDs and runtime `TypeId` responsibilities distinct. Handle non-cloneable components explicitly rather than falling back to serialization.

Connect generated GPU metadata to SceneDB insert/mutation/removal for authored values. Complete generic mirror attachment/replay and registration/growth semantics upstream where necessary. Reflection must describe data without making SceneDB depend on Helio-specific component cases or unsafe reentrant callbacks into `World`.

**Exit:** a component created by a generic factory and inserted through erased APIs behaves identically to a direct typed insert, including mutation and removal, with no render-specific helper call. Disabled/multiple instances and late mirror attachment are represented correctly.

### Phase 2 — Prove the complete mesh and light path

Implement GPU consumers of the canonical reflected columns for meshes/materials, transforms, visibility and lights. Establish complete joins, layouts, eligibility and shader units. Convert the minimum add/edit producers needed for a real editor reproduction to the typed API.

Delete the mesh/light CPU projection and render-subscription dependencies for the converted paths in the same slice. Supersede the current executor/asset-drop arming and marking workarounds. Retain the required `SceneWorldExt` import wherever remaining calls need it; removing a workaround is not a reason to reintroduce the earlier compilation failure.

**Exit:** direct insertion and editor insertion both produce visible output without an inspector or notifications; editing, removal, enabled state, asset completion and static/movable transitions work. A rendered-frame probe validates the actual pass path.

### Phase 3 — Convert every editor, class and script producer

Migrate all Section B callers, history and in-memory class data onto the typed APIs. Adapt file/tool/protocol ingress once at the boundary. Add explicit migrations for old flat/nested component schemas; preserve unknown data and report invalid known data. Remove obsolete JSON-backed internal APIs after their final callers move.

**Exit:** internal add/edit/clone/history/class/script paths have no serialize/hydrate round trip, no split authority and no instance-zero aliasing. Every remaining JSON use in the scope ledger has a documented boundary or unresolved-payload purpose.

### Phase 4 — Complete all remaining component-to-pass contracts

Implement every Section C contract, including components stranded behind `PendingWorldWrites` and all additional registrations discovered in Phase 0. Remove each old producer when its replacement is covered. Delete `PendingWorldWrites` and inert renderer behavior dispatch machinery after the final producer is converted; do not restore the removed dispatcher as a fix.

**Exit:** each supported render component has an actual GPU consumer and observable effect, and each unsupported capability is explicitly reported. No exposed component silently succeeds through an undrained queue or no-op runtime behavior.

### Phase 5 — Finish observer delivery and runtime parity

Move all state subscribers to independent delivery, fix misleading helper contracts, and retire destructive global drain APIs from production paths. Share the database/flush/render lifecycle across editor, standalone and embedded runtime. Remove full-sync/resync flags, hand-maintained replay lists and stale bridge exports. This phase may begin earlier, but renderer correctness must already be independent of its completion.

**Exit:** multiple panels/scripts observe the same commits regardless of polling order; all execution modes render the same authored scene through the same data contract; scene replacement and late attachment need no repair path.

### Phase 6 — Remove compatibility residue and close acceptance

Remove obsolete adapters, CPU render row caches, JSON scene mirrors and tests that require them. Update `.agents/REFLECTION.md`, `.agents/ECS.md`, `.agents/SCENEDB_MIGRATION.md`, `.agents/HELIO_SCENE_API_MIGRATION.md`, affected API docs and examples. Add narrowly scoped architecture checks against reintroducing live JSON hydration or renderer scene scans; boundary codecs and real gameplay events remain allowed.

**Exit:** every ledger row is closed with evidence, all acceptance groups below pass, dependency/submodule revisions are reproducible, and documentation describes the implemented architecture.

## Acceptance matrix

Tests below are required implementation deliverables. Source inspection during planning is not evidence that they pass.

| Group | Required cases and evidence |
|---|---|
| Mutation equivalence | Direct insert/get_mut, erased insert, reflected setter/method, nested/collection edit, editor command, asset drop, script, plugin factory and class placement produce equivalent authoritative and GPU values. No manual refresh or subscriber needed. |
| Instance lifecycle | Multiple same-class instances; duplicate/reorder/nest; disable/enable; remove/despawn; index reuse; stale references; undo/redo and subtree restore. Methods/properties/GPU joins address the same instance. |
| Observer fanout | Two panels plus independent scripts subscribe to the same/different components; vary read order and frequency. Exercise overflow, unsubscribe, removal, initial subscription and world replacement. No consumer steals updates. Rendering runs correctly with zero subscribers. |
| Mirror lifecycle | Populate before attach; attach before populate; register a new type late; empty world; bulk load; buffers exceed initial capacity; pool compaction; remove/reinsert; device recreation; multiple worlds/viewports. Check buffer generations and live-row semantics. |
| Real render output | Add one supported component of every rendering class through its real production producer. Validate the relevant visible effect/pass output using GPU readback or deterministic image probes. Inspect GPU rows and draw counts as supporting evidence, not the sole success criterion. |
| Mesh/light regression | Cube insertion with loaded geometry, material override, transform/scale, light types/intensity/direction, visibility/enabled state, shadow flags, static/movable transitions and removal. Exercise inspector open/closed, idle wake-up and pending GPU work. |
| Asset behavior | Shared asset reuse, missing/corrupt asset, reload, rapid replacement, cancellation, removal while loading and stale completions. Property-only edits cause no geometry disk reload or whole-mesh clone. |
| Persistence compatibility | Old flat light schemas and nested schemas; disabled/multiple/unknown components; class defaults and overrides; save/load and package/runtime load. Errors identify object/component/property; failed loads do not leave misleading successful attachments. |
| Runtime parity | Same fixture through editor, standalone, embedded/PIE, play/stop and level replacement; headless typed state remains usable and later GPU attachment is correct. |
| Cost and architecture | Idle scenes perform no CPU component discovery/projection or JSON round trips. Unrelated edits do not rebuild other components. Dirty uploads are bounded to actual changes; asset and GPU caches invalidate correctly. |
| Build and ownership | Run relevant parent, Helio and SceneDB checks/tests through their valid workspaces. Record revisions, hardware/backend for GPU checks, results and any unsupported capability. Build success alone does not close rendering acceptance. |

## Final closure checklist

- [ ] Every registered class and every render pass has a completed source/consumer ledger entry.
- [ ] Every production component producer uses typed writes after any external decode.
- [ ] Disabled and duplicate components have one typed authority and stable identity.
- [ ] No internal history/class/script path uses JSON to clone or update known live values.
- [ ] No renderer locks/scans the CPU scene to discover or project render components.
- [ ] No renderer state-change subscriber, manual refresh/arming hook, or force-resync repair remains.
- [ ] No undrained component write queue or orphaned behavior producer remains.
- [ ] GPU reflection covers all mutation, removal, attachment and buffer lifecycle paths generically.
- [ ] Every subscriber independently observes state and can recover after lag/world replacement.
- [ ] Every supported component's effect is demonstrated through the actual editor and runtime path.
- [ ] Remaining serialization and genuine event streams are explicitly classified and documented.
- [ ] Historical migration claims, examples and tests agree with the final architecture.

The corrective work is complete only when these checks and the acceptance matrix are satisfied. Renaming a bridge, moving its scan to another subsystem, adding more notifications, or repairing meshes while leaving other component families stranded does not satisfy this plan.
