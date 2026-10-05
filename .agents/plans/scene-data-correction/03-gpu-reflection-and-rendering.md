# Contract proposal: GPU reflection and rendering

Status: **proposed — review required**

## Proposed decision

SceneDB owns authored render-component values and their GPU-reflected columns. Helio consumes these columns as inputs. The renderer does not enumerate CPU scene entities, reconstruct component structs, subscribe to editor changes, or keep a second editable scene.

SceneDB remains graphics-independent. GPU layouts and buffers are declared from reflected/`SceneStore` metadata; engine feature modules register component schemas; Helio binds the resulting typed resources and executes passes. This defines a dependency direction of feature/schema -> SceneDB registration and render graph -> SceneDB GPU read interface, without a SceneDB -> Helio dependency.

**Pass isolation rule:** only the render-graph crate may name, import, construct, configure, or schedule pass types. Component crates, SceneDB, editor, engine composition, plugin SDK, and other subsystems must not depend on pass crates or pass IDs. Components publish reflected data schemas; a component schema may opt selected fields into SceneDB GPU reflection/upload. This says where data goes, not what renders it. The renderer decides whether and how to consume uploaded buffers. The render-graph crate owns the mapping from component schemas to pass inputs and owns the graph that schedules those passes. Individual pass crates own their shaders, bindings, algorithms, and pass-local transient state.

## Identity, rows and joins

Component rows are keyed by component-instance entity identity and generation. Owner object identity is an explicit GPU field/key. Transform, visibility and object flags are reflected from the owner object. GPU queries join component owner IDs against object columns using a SceneDB-provided lookup/table or generated join primitive. Sparse ECS row order must never be mistaken for entity identity or for another component's row order.

Every GPU table exposes presence/liveness and generation semantics. Removal clears or invalidates the row before its slot may be reused. A render pass must reject stale owner generations and disabled/missing components. Scene IDs and component-instance IDs are stable persistence keys, not shader indices.

## Authored data versus execution output

Authored fields (mesh/asset reference, material parameters, light settings, movability, portal settings, etc.) are reflected directly. A pass may produce transient culling lists, visible-object compaction, indirect draw commands, shadow assignments, probe outputs, or simulation buffers. These are derived execution outputs with explicit lifetime and source generation; they are not another CPU-authored component store.

For meshes, the target path is GPU-side selection/join of enabled static mesh component rows, owner transform/visibility rows, mesh resource ranges, and material data, followed by culling/batching and draw generation. Eliminate CPU-authored `StaticObjectComponent` and default-material row construction in `helio_bridge`. For lights, join authored light rows with owner transform/visibility and feed lighting/shadow passes directly; eliminate `EditorLightRows` and billboard reconstruction. Editor-only billboards/gizmos can be a GPU pass over the same authored light/object data.

Arbitrary reflected properties and GPU uploads do not automatically create pixels or draw calls. Components do not declare render capabilities or pass consumers. The render-graph crate alone decides whether and how a GPU schema is consumed, maps it to pass inputs, and reports unconsumed render-relevant data as an integration gap. A component can be valid and useful without GPU fields or a renderer consumer.

## Layout contract

Every GPU schema records:

- Stable schema ID/version and layout hash.
- Field order, scalar representation, alignment, padding and array/variable-length representation.
- Units and coordinate conventions (world/local, handedness, radians/degrees, color/intensity units).
- Component/owner ID and generation fields, enabled/presence semantics.
- Resource handle representation, pool offset/capacity and content generation where applicable.
- Mutation invalidation policy and GPU layout metadata. The renderer graph separately owns the mapping from these schemas to shader/pass consumers.

Generated Rust and WGSL layouts must be checked together. Avoid raw Rust object bytes: padding, pointers, enums, bools, and variable-length allocations are not a GPU ABI. GPU store owns explicit packed bytes/columns generated from schema metadata.

## Resources and mesh data

Components hold typed stable resource references/content IDs. Asset storage owns immutable vertex/index/material/texture data and GPU allocations, shared among component instances. SceneDB GPU rows carry checked resource handles/ranges and generations. Geometry replacement updates asset content generation and invalidates dependent draw state. Transform, movability or unrelated property edits do not reread mesh files or duplicate immutable geometry.

Missing or failed resources have explicit status and diagnostics. A stale async result cannot publish a handle into a different world/component generation. Material defaulting is defined once by component schema/material resolution; renderer bridge code must not invent a second default material record.

## Mirror lifecycle and frame ordering

The SceneDB GPU mirror can attach before or after world population. Schema registration, existing-row replay, bulk hydration, buffer growth/reallocation, compaction, device recreation and world replacement use generic schema-driven logic. No finite hand-maintained type list or per-component reinsert loop is required.

The frame contract is: commit scene transactions -> make their reflected dirty ranges visible to the GPU resource provider -> encode render work that reads those resources -> submit. The exact asynchronous upload mechanism may differ, but a draw cannot pass a required pending write and display stale state as current. GPU-produced work/readbacks can complete independently of CPU `World::revision`; idle gates account for pending uploads, asset completions, simulation and readback state.

## Required graph-owned component/pass ledger

Before implementation, the render-graph crate receives a ledger row mapping component schema -> authored fields -> optional SceneDB GPU-upload fields -> graph-owned pass mapping -> observable output. At minimum include static mesh, material override, LOD, light and light visualization, movable/shadow classification, water, fog, foliage, portal, reflection capture, post-process volume, camera post-process, voxel/terrain, spline, sky/sun, decals, and every optional pass or graph found in Helio. Components declare only their authored/GPU data; only the graph ledger names passes and decides how that data is used. Each row documents its actual data consumer and observable behavior or marks the feature as unconsumed. See [09-component-structure-and-graph-boundary.md](09-component-structure-and-graph-boundary.md).

## REVIEW: decisions to confirm or edit

- Are component-entity GPU rows with owner-key joins acceptable as the baseline?
- Should GPU joins use a SceneDB-generated entity-to-row table, compute pass, or another mechanism?
- Is GPU-side draw-record generation the target for all geometry, or may a CPU-built transient command list remain for specific backends? If retained, how is it prevented from becoming another scene copy?
- Should editor visualization components (billboards/gizmos) be authored components, pass outputs, or editor-only GPU consumers of authored rows?
- How should the renderer graph report a GPU schema that has no consuming pass, while allowing valid components that simply are not rendered?
- Which graphics APIs/backends must support the same reflected layout and lifecycle?
- Does the graph-owned mapping keep all pass references inside the render-graph crate as required?
