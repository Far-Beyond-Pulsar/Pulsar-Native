# Contract proposal: authority and component identity

Status: **proposed — review required**

## Proposed decision

SceneDB is the sole live authority for scene and component state. A scene object and each attached component instance are separate SceneDB entities:

- A scene object entity owns identity, name, hierarchy, transform, visibility, selection, and object-level metadata.
- A component instance entity owns exactly one registered component value and attachment metadata: stable component-instance ID, owner object ID/reference, order, enabled state, parent/nesting metadata, and optional class-slot provenance.
- Component data exists once as a live typed value. The property panel, scripts, renderer, undo layer, and class system refer to it; they do not own serialized editable copies.
- GPU component rows use the component entity as their identity and contain/join through the owner entity identity for transform and visibility.

This proposal chooses component entities because the current ECS naturally represents one value of a Rust component type per entity, while the editor permits multiple instances of the same class on one object. It avoids hiding an array of erased values inside one ECS component and gives each instance a first-class address for reflection, subscriptions, history, and GPU joins.

## Identifier rules

1. `Entity` is an ephemeral handle valid only in one live `World`. Never save it to a scene file, script reference, undo archive, or persistent asset.
2. `SceneObjectId` and `ComponentInstanceId` are stable, unique IDs within a scene. Generate them at creation; preserve them through save/load and class reload where the logical instance survives.
3. The registered component type has a stable schema ID (for example a namespaced UUID/string), distinct from Rust `TypeId`, display name, and plugin-local symbol.
4. A class prefab slot has its own stable slot ID. A placed component instance records the slot ID as provenance; it does not use list position as identity.
5. Ordering is metadata, never identity. Reorder changes display order only. References resolve by stable instance ID and verify type/owner as appropriate.
6. GPU rows and asset handles are runtime indices/handles; pair them with generation/version data and never treat an index alone as a persistent identity.

## Ownership and relationship invariants

- A live component instance has exactly one live owner object. Reparent/transfer is a validated transaction that updates ownership once.
- Despawning an object either despawns all owned components in the same transaction or rejects the operation; orphaned component entities are invalid.
- Removing a component removes its authored GPU rows and releases instance-owned resources. Shared immutable assets are reference-counted/retained by their asset owner.
- Enabled state controls participation in the relevant runtime/render query. A disabled component remains typed and editable, survives serialization and history, and does not get recreated from a JSON shadow on enable.
- Nested component presentation (if retained) is represented by explicit typed parent-instance metadata and cycle checks. It must not alter data ownership or routing.
- A stable script reference identifies `(scene/world identity, component-instance ID, expected schema ID)` and fails clearly if stale. It never silently falls back to the first component of that class.
- Class expansion creates/updates actual component entities and owner links. Overrides are attached to stable slot identity, not array index.

## Scene queries and editor surface

Object hierarchy queries operate on object entities only. Component hierarchy/property queries first resolve the selected object, then enumerate owned component entities ordered by attachment metadata. Render queries operate on component entities and join owner data through a GPU-visible owner key. Scripting resolves an explicit component-instance ID; a convenience `(object, type)` lookup must state whether it requires exactly one match and return ambiguity otherwise.

Object creation, component addition, object duplication, class instantiation, deletion, component reorder, and hierarchy changes are SceneDB transactions. Observers see the committed transaction, not intermediate orphan states. Undo records stable IDs and typed values and can remap raw entity handles during restoration.

## Tradeoffs and required feasibility check

Component entities add an owner relationship and GPU join. They increase entity count but simplify multiplicity, subscriptions, history, plugin-defined types, and authored identity. The GPU path must support joins using stable per-world row keys plus generation/presence data; no assumption may be made that sparse component rows align to owner-object rows.

Before implementation, verify SceneDB can support:

- Typed metadata components on component entities without name/type collisions.
- Erased insertion/removal of the class value while preserving normal `World` mutation hooks.
- Stable ID uniqueness and transaction validation without a parallel object database.
- GPU-visible owner IDs/generations and lookup/join structures.
- World snapshots/restoration that preserve stable IDs while changing ephemeral entities.

If these require upstream changes, propose those changes against SceneDB itself and pin a reviewed revision. Do not solve feasibility by retaining live JSON attachments or rebuilding renderer rows on the CPU.

## REVIEW: decisions to confirm or edit

- Is one component entity per attached instance the desired multiplicity model?
- Should object-owned components be despawned automatically with the object, or should callers have to request cascade explicitly?
- Are nested components real component-to-component ownership, presentation-only grouping, or should nesting be removed?
- What stable ID format and scope should scene and component IDs use?
- Should script references target component instances directly, or should the public script API expose a validated object-plus-type/index facade?
- Are there scene component types that intentionally belong to a world/global entity instead of an object?
