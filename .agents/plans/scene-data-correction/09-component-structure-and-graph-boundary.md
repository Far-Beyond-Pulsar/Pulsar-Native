# Contract proposal: generic components and pass boundary

Status: **proposed — review required**

## Questions this contract answers

- Does every component get a common lifecycle? **Yes:** registration, typed storage, reflection, persistence policy, mutation, removal, history policy and notification semantics are generic.
- Does a component render or produce pixels? **No:** components own data. A component schema may opt fields into SceneDB GPU reflection/upload; the renderer decides whether and how to consume those buffers.
- Does a component name the pass that consumes it? **No:** only the render-graph crate names/concretely depends on pass crates.
- Does engine composition name passes? **No:** it registers component bundles and starts/configures the render graph through a pass-agnostic API.

## Component module template

Every component type should follow a consistent feature-local shape, adjusted only when the domain needs a real subsystem:

```text
feature/component_name/
  mod.rs                 public type and feature-local exports
  component.rs           authored data, defaults, schema/reflection declaration
  properties.rs          optional nested reflected property groups
  validation.rs          optional typed constraints and normalization
  asset_refs.rs          optional stable typed resource references
  gpu_schema.rs          optional generated GPU-upload fields (no pass imports)
  system.rs              optional domain behavior, scheduled by its owning subsystem
  archive.rs             optional version migrations/custom boundary codec
  tests.rs               registration, typed lifecycle, and feature behavior
```

Small components need not have a file for every concern. The rules are:

1. The component struct contains authored state and stable references. It does not contain a renderer object, pass instance, GPU buffer pointer, transient upload state, or duplicated transform/visibility owned by its scene object.
2. Reflection metadata and factory are generated from the component type where possible. Nested groups are typed reflected values, not JSON-shaped sub-properties in the live world.
3. Optional GPU-upload fields are explicit, packed/schema checked, and derived automatically from the same live value. This is a SceneDB data-layout/upload declaration only; the component has no render capability, pass ID/name/import, or render-graph dependency. The renderer independently decides whether and how to consume the resulting buffers.
4. A system file exists only for real domain logic. The component type itself does not enqueue a delayed generic world write to a removed dispatcher. Systems query typed SceneDB state and commit typed output through the current transaction/scheduling API.
5. Resources use stable typed handles. Async work returns a typed completion that validates identity/generation before writing.
6. Archive migrations are boundary code, separate from live property setters and mutation hooks.
7. Component lifecycle is independent of whether an editor panel is open, a renderer exists, or a notification subscriber is polling.

## One generic lifecycle for every component

All registered components use the same framework path:

```text
schema registration
  -> typed factory/value
  -> SceneDB component-instance entity
  -> typed/reflected mutation transaction
  -> automatic change journal + generated GPU dirty tracking (if GPU schema exists)
  -> independent subscribers read current values
  -> persistence adapter encodes only at save/export boundary
```

No component-specific call site may be required to:

- Add a subscription so the renderer notices it.
- Call `mark_*_changed`, `refresh_gpu_mirror`, or reinsert a value for the GPU.
- Serialize/reparse to clone, set a property, undo, or notify.
- Enqueue a `PendingWorldWrites` closure for an absent consumer.
- Add a class-name branch to renderer, editor, SceneDB or engine core.

Components can have optional generic capabilities: reflected properties, methods, persistence, GPU-upload fields, asset references, editor inspection, or domain systems. These describe data and lifecycle support, not renderer behavior. Registration composes descriptors but does not fork the basic add/edit/remove/history path.

## Strict crate dependency contract

```text
component feature crates ──> reflection + SceneDB APIs
engine composition ────────> component registration bundles + render-graph API
render-graph crate ─────────> SceneDB GPU resource API + concrete pass crates
pass crates ────────────────> pass-local shader/render APIs + declared GPU input schema
editor ────────────────────> component reflection descriptors + SceneDB transactions
```

Concrete pass types, IDs, constructors, config structures, shader module names, and pass scheduling APIs may be referenced only in the render-graph crate. Pass crates may not be imported from component modules, engine composition, editor UI, SceneDB, plugin SDK, asset compatibility layers, renderer facades, demos, or unrelated subsystem crates. Pass implementation code and tests that live inside the pass's own crate are naturally allowed to refer to their own pass type; integration harnesses outside it must go through the graph API.

This proposal also forbids pass-to-pass crate dependencies: one pass is not a client of another pass. If two passes need common layouts, shader helpers, algorithms, or resource types, move those shared contracts to a neutral renderer-core/schema crate or let the graph coordinate them through declared GPU inputs/outputs. A pass may use its own code and neutral shared APIs. This is the interpretation that keeps concrete pass coupling inside the graph crate.

The render-graph crate owns a graph registry/manifest that maps stable component schema IDs and their GPU layouts to GPU input bindings and pass graph nodes. This is where all pass-specific joins, binding groups, pass ordering, optional pass selection, and backend variations are wired. It may aggregate feature-provided schemas, but component registration does not call back into pass registration. The graph can choose to consume a schema, ignore it, or report an integration gap; that choice is renderer-owned. In the current tree, `helio-default-graphs` is the candidate; current direct pass dependencies in engine, Helio facade/core, component, compatibility, wasm, demos/examples, and pass-to-pass edges are migration scope, not accepted exceptions.

Individual pass crates own the algorithm and shader contract for their work. Their public inputs are explicit GPU data/resource schemas. They do not scan the CPU World or maintain their own authoritative component copies. A pass may own transient GPU output/caches keyed by source generations.

The engine may know that rendering is an optional engine service and construct the render-graph service. It may know concrete component types for registration. It must not know which pass implements a component or import a pass crate. The render graph is replaceable without changing component definitions or editor property code.

Plugins follow the same rule. A component plugin declares reflected data and may mark fields for GPU upload. The graph crate independently decides whether and how to consume those buffers. A plugin cannot make the editor or engine core import a pass. A plugin-supplied pass extension, if supported, enters through the graph crate's API and is scheduled there.

## Keeping components current with engine systems

The audit must identify all existing lifecycle systems and remove obsolete parallel mechanisms. Use current SceneDB `World` write guards, change cursors, `SceneStore` derive/layout registration, GPU mirror flushing, engine `InitGraph`/subsystem lifecycle, asset registry, and typed transaction APIs as verified at implementation time. Do not assume old docs or comments match current APIs.

Specifically audit and retire as appropriate:

- `ComponentRuntimeBehavior::sync_component` used as generic renderer discovery or serialized world replay.
- `PendingWorldWrites` and its producer closures after their real domain work is migrated.
- `sync_static_mesh_rows`, `sync_editor_light_rows`, `mark_render_components_changed`, subscription arming, and finite GPU mirror replay lists.
- JSON property maps and live component instance records that duplicate SceneDB values.
- Old ECS/scheduling examples or reflection documentation that describes removed systems as active.

Keep real physics, audio, gameplay, voxel generation, water simulation, asset loading and GPU transient algorithms. Move domain behavior to its owning current subsystem/schedule and make its reads/writes typed. Do not create a replacement universal component loop: regular query schedules should be feature/system-owned and only run for components with that behavior.

## Generic means

Generic support means every registered component gets the same safe construction, ownership, edit, validation, persistence-boundary, duplication policy, enable/disable, removal, undo and notification lifecycle without a new engine-core branch. Components may opt fields into the same generic SceneDB GPU reflection/upload lifecycle. It does **not** mean every property must be uploaded, every component creates pixels, or all domain behavior is implemented by one universal system.

Adding a new component should require implementation in its feature module, registration bundle, optional system and optional GPU field schema. If the renderer should consume it, the graph owner separately adds the schema-to-pass mapping. It should not require editor special cases, engine-core class-name switches, CPU render synchronization, or changes to generic SceneDB internals unless it exposes a new reusable capability.

## Enforcement and acceptance

- Dependency check: search/CI architecture rule confirms the chosen graph-owner crate is the only client of concrete pass crates. Exempt only a pass crate referring to its own implementation modules; no other crate, including another pass crate, may depend on a pass crate. Check manifests and source references across the parent workspace, Helio workspace, examples and plugins.
- Registration test: a test component added through the generic factory gets normal typed edits, clone/snapshot policy, removal, notifications, archive codec and GPU row when its descriptor requests one.
- Non-render test: a gameplay-only component is fully addable/editable/persistable without any renderer dependency.
- Render test: a component with GPU-upload fields changes SceneDB's GPU buffers through normal writes; the renderer shows an effect only when the graph maps and consumes those fields. Add/edit/remove work without panel polling or CPU projection.
- Module review: component crates contain no pass imports, pass IDs or pass-owned state; engine composition contains component registration and the graph service API, not pass types.
- Up-to-date API review: no production use of removed dispatcher/queue/sync APIs remains unless an explicit migration waiver identifies the genuine domain behavior it serves and its removal plan.

## REVIEW: decisions to confirm or edit

- Does this match the intended meaning: components own data and may request GPU upload through schema metadata; the renderer alone chooses to consume it and produce pixels?
- Should all default/backend graph composition live in one `helio-default-graphs` crate, or should the graph owner be a dedicated replacement crate?
- Which current subsystem scheduling API should the contract name after a targeted source audit?
- Should the component module template be a convention, a proc-macro-generated skeleton, or a compile-time enforced layout?
- Are there any feature domains where pass-local data types must be shared, and should those types move to a neutral schema crate?
