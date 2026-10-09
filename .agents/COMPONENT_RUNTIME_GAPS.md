# Component runtime: current gaps and target

This document compares the component runtime in the repository today with the
Rust-facing runtime needed for native component behavior, Blueprint events,
and engine callbacks. The target is a proposal; the “current” sections describe
the checked-in implementation.

## Summary

> **Implementation status (2026-10-05):** the native component callback path
> and named component event path described below are now implemented for the
> migrated terrain component. The original comparison that follows records
> the state before this implementation. See [Implemented behavior](#implemented-behavior)
> for the current API, tick ordering, and remaining edges.
>
> **Phase 6 of Pulsar-Native#1035 (2026-10-08):** the `ComponentRuntimeBehavior`
> sync dispatch the table below describes is gone. Pulsar registers no
> runtime behavior, `#[register_runtime_behavior]` no longer exists, and
> `#[register_world_component]` takes an inherent `impl Type {}` and
> registers the World value only (factory, boundary decoder, clone,
> insert/remove). Component instances are their own entities, so the
> instance-identity question below is settled: a `ComponentInstanceId` and
> the instance entity. Lifecycle removals are read from per-reader change
> cursors.

Pulsar already has typed component values in SceneDB, reflected properties,
macro-generated component method metadata, a world-level method dispatcher,
and a DLL-safe Gamma event transport. It does not yet join those pieces into a
component lifecycle. In particular, a component has no normal Rust `tick`,
named event emitter/handler API, or generated per-instance callback
registration.

The component value in SceneDB should remain the sole authoritative instance
of the component's editable/runtime state. Runtime callbacks should resolve
and borrow that live value when they run. There should be no per-frame
deserialize/copy/sync loop to keep a second component representation current.
This does not prohibit derived resources such as GPU upload layouts or a
generated terrain cache: those are disposable projections/results, not a
second source of component fields, and must be rebuilt or invalidated from the
live SceneDB value when their inputs change.

## What exists today

| Area | Current implementation | Gap |
| --- | --- | --- |
| Component storage | `pulsar_scenedb::World` holds typed component rows. *(Before Phase 3, `pulsar_world_registry` hydrated them from scene JSON on edits; edits are now typed writes and JSON is decoded only at load.)* | The storage and reflection paths are not yet a component behavior lifecycle. |
| Rust callable methods | `#[component_methods]` describes methods and `#[world_method]` exposes them to the script/Blueprint dispatch path. For example, terrain methods take `&World`/`&mut World` and an `Entity`. | The ergonomic Rust call still looks like a static world operation, not a method on the live component instance. There is no generated Rust event emitter or event handler binding surface. |
| Runtime behavior trait *(removed, Phase 4/6)* | `ComponentRuntimeBehavior::sync_component` receives `&Self` and is dispatched by the world registry. Its use is for projection/synchronization work; it is not a tick callback. Several voxel implementations are deliberately empty. | No component `begin_play`, `tick`, event-handler dispatch, enable/disable, or end lifecycle is provided by this trait. |
| SceneDB mutation | World methods can use `&mut World`; terrain edits are appended to the typed terrain component row. That row is the authoritative data. Reflection setters and callable methods also dispatch against the live typed World value. | Runtime callbacks need a defined way to borrow the same value mutably, with World borrow rules respected. A second copy of component fields would introduce the sync architecture SceneDB is intended to avoid. |
| Events | `pulsar_events` wraps Gamma's event bus. Typed events are convenient within a Rust build; the host/plugin path supports DLL-safe descriptors and encoded payloads. Script event delivery queues handler calls for a later script phase. | Components do not currently declare named events that are shared by Rust, Blueprint reflection, and DLL registration, nor do they have per-component-instance subscription dispatch. |
| Registration | `#[register_runtime_behavior]` registers the current sync behavior; `#[register_world_component]` generates World hydration/removal/dispatch metadata. The world registry is populated by inventory. | These registrations do not declare or register component lifecycle callbacks, named events, event handlers, or their Blueprint pins. |

### Terrain as a concrete example

`VoxelTerrainComponent` has reflected scripting methods such as `get_block`,
`set_block`, `fill_sphere`, and `fill_cube`. The methods use world/entity
arguments, and mutating operations update the terrain's typed World row. Its
`ComponentRuntimeBehavior::sync_component` implementation was empty (the stub
was removed in Phase 6). That was a
useful and intentional state at the time: the terrain has callable operations and
authoritative data, but it does not yet have a component-owned runtime or an
`on_block_broken` event surface.

There is a derived CPU terrain cache in `voxel_world.rs` (`WORLDS`, keyed by
entity bits). It stores a recipe, a snapshot of the edit journal, and an
`Arc<Planet>` so terrain queries can reuse generated data and apply appended
edits incrementally. This is not a copied `VoxelTerrainComponent`: component
configuration and edits still come from the live World row, and the cached
planet is computed output. The cache therefore does not make component
callbacks synchronize a shadow component. Keep this distinction explicit:
callbacks must read/write the World row, while disposable derived caches may
track source revisions to know when their outputs need updating.

The desired event should be emitted at the domain operation that commits a
block removal, after the terrain data/edit journal has been updated. It should
not be inferred by diffing a synchronized copy later.

## Target Rust experience

The component should declare Blueprint-visible event names and payload
signatures once, close to its Rust definition. The declaration is metadata,
not the implementation of the behavior:

```rust
pub trait TerrainEvents {
    #[bp_event]
    fn block_broken() -> BlockData {
        // Event declarations stay empty.
    }
}
```

The macro should derive a stable event identity from the declaring component
and event name and generate the registrations/adapters needed by reflection,
Blueprint, and the runtime. Rust payload and argument types remain the types
written in the signature; the event macro must not rewrite those APIs. Across
the DLL boundary, the generated adapter uses Gamma's stable descriptor and
DLL-safe encoded representation. The Rust type itself is not the cross-DLL
event key.

At an emission site, Rust should call the generated event by its name, for
example `ctx.events.block_broken(block_data)`. It should not select the event
through a Rust event type. Blueprint binds to the same named event and sees
the declared payload pins. The generated glue owns event identity, payload
marshalling, registration, and dispatch adaptation.

A component runtime trait should separately express callbacks that belong to
the component instance, with a shape along these lines:

```rust
impl VoxelTerrainComponent {
    fn begin_play(&self, ctx: &mut ComponentContext<'_>) {}

    fn tick(&mut self, ctx: &mut ComponentContext<'_>, dt: f32) {}

    fn on_block_broken(&mut self, ctx: &mut ComponentContext<'_>, block: BlockData) {}
}
```

This is illustrative syntax, not an existing API. The generated dispatcher
must resolve the component instance and call it while borrowing the live
SceneDB value. Read-only callbacks can take `&Self`; callbacks that change
component state need `&mut Self`. The runtime must define how this borrow is
obtained from the World and avoid holding it while re-entering the World.

## SceneDB and callback execution

SceneDB is the instantiated level and owns the component's live data. A
callback should be routed by stable instance identity (at minimum owner/entity
plus component identity, with a generation or equivalent protection against
stale registrations), then look up that component in SceneDB at dispatch time.
The registry may hold generated function pointers and subscription metadata;
it must not hold a cloned component value as a runtime shadow.

This gives direct mutation without a sync pass:

1. Runtime resolves the live component row in SceneDB.
2. Generated adapter borrows it as `&Self` or `&mut Self` and invokes the Rust
   callback.
3. Changes are immediately changes to the authoritative component data.
4. If the callback emits events, the event writer queues them for the
   appropriate runtime phase.

Events should generally be queued and delivered after the emitting callback
releases its World borrow. Inline delivery could re-enter World while it is
already mutably borrowed. The existing script runtime's queued event-handler
phase is a useful precedent. The component contract still needs to specify
which engine phase drains component events and what ordering they have relative
to input, physics, scripts, and ECS schedules.

The runtime should also define callback lifecycle and cleanup: when
`begin_play`/`end_play` run, whether disabled components receive ticks/events,
and how subscriptions are removed on component removal, entity despawn, world
teardown, or plugin unload. Callback state crossing a DLL boundary must use
host-owned registration/handles and DLL-safe descriptors; a Rust trait object,
monomorphized type identity, or function pointer must not be treated as a
portable event identity.

## Macro-led migration

The macro layer is the right place to keep migration incremental and the Rust
surface pleasant. Most of the complexity is registration and dispatch glue,
not component business logic. A component author should declare intent in
Rust; generated code should connect that declaration to existing registries.

The migration can proceed component by component:

1. Keep the current typed SceneDB component as the sole authoritative
   component-state instance; retain only derived caches/projections whose
   inputs are read from that live value.
2. Extend or add a derive/attribute macro for component lifecycle callbacks,
   named event declarations, and handler adapters.
3. Generate stable named event descriptors and Blueprint metadata from event
   declarations, while preserving the declared Rust payload signatures.
4. Generate inventory registration for the callbacks and event metadata.
   Runtime dispatch resolves the target component in the live World and
   invokes the generated adapter.
5. Add a runtime phase for component lifecycle callbacks and queued event
   handlers, with explicit ordering and teardown behavior.
6. Migrate a concrete component such as `VoxelTerrainComponent`; move domain
   logic into ordinary instance methods and emit `block_broken` at the
   successful edit site.
7. Retire old sync/hydration paths only when all consumers have moved. JSON
   scene loading may still need an ingress step to construct typed rows; that
   is a load boundary, not a per-frame component mirror.

The existing `#[component_methods]` surface can remain the script/Blueprint
callable-operation mechanism. Named events are a separate declaration and
registration concern: they describe outputs from runtime behavior, rather
than pretending each event is an ordinary callable method.

## Open decisions before implementation

- Which runtime phase owns component `begin_play`, `tick`, and queued event
  callbacks, and what is their ordering relative to the current game tick
  phases?
- Do callbacks receive a direct `&mut World`/component context, or only
  constrained services plus the borrowed component? The design must prevent
  aliasing/reentrant World access while the component is borrowed.
- How are handlers attached: generated component methods, Blueprint instance
  graph bindings, or both through one registration record?
- What exact stable event name format and payload schema/version policy should
  Gamma descriptors use?
- What is the component instance identity when the same class can occur more
  than once on an owner, and how are stale subscriptions rejected?
- ~~Which pieces of `ComponentRuntimeBehavior::sync_component` remain useful for
  renderer projection?~~ None: Pulsar-Native#1035 moved every projection to
  SceneDB GPU rows and graph-owned derivations and removed the dispatch.

## Source anchors

- `crates/renderer/helio/crates/helio-component/src/components/voxel_world.rs`:
  terrain methods and edit journal mutation.
- `crates/renderer/helio/crates/helio-component/src/components/voxel_component_runtime.rs`:
  voxel World registrations and the terrain's component runtime.
- `crates/core/pulsar_world_registry/src/lib.rs`, `src/values.rs` and `src/dispatch.rs`:
  typed World values (factory, boundary decode, clone, erased insert/remove)
  and reflected property/method dispatch.
- `crates/core/pulsar_world_registry/src/component_lifecycle.rs`: live
  component lifecycles and their removal cursors.
- `crates/core/engine_class_derive/src/lib.rs`:
  `register_world_component` and `register_component_runtime` macro output.
- `crates/core/pulsar_events/src/host.rs` and `src/hub.rs`:
  Gamma-backed host/plugin event transport and in-process event API.
- `crates/core/pulsar_game/src/scripting/events.rs` and `src/tick.rs`:
  queued script event delivery and current game tick phases.

## Implemented behavior

### Live component callbacks and lifecycle

`#[register_component_runtime(enabled = enabled)]` on an inherent component
impl registers typed lifecycle shims in `pulsar_world_registry`. A component
can define `begin_play(&mut self, ctx: &mut ComponentContext<'_>)`,
`tick(&mut self, ctx: &mut ComponentContext<'_>, dt: f32)`, and optionally
`end_play(entity, events)`. The generated shim uses a typed `World` query and
mutates the row already owned by SceneDB. It holds no component copy and does
not call the renderer sync path.

`ComponentRuntimeState` stores only each active entity's generation-bearing
`Entity` and component identity. It runs `begin_play` once per activation,
then `tick`; disabling/removing the component or despawning the entity ends
the activation. Scene replacement and TickLoop shutdown end remaining
activations. Removal-journal processing runs after actor mutation and again
after script mutation, outside the component borrow. Since SceneDB's removal
journal has no removed value, `end_play` is owner-only and cannot inspect the
removed component's old fields.

The exact TickLoop sequence is: input and `AfterInput` event flush; ECS
schedule; registered component callbacks in type-name order, with `begin_play`
on first activation, queued native event handlers in arrival order, then
`tick`; actor and rebinding callbacks; component outbox transfer;
`AfterPhysics` event flush; script begin/tick/command phase; component-removal
cleanup for script mutations; `AfterScripts` and `EndOfFrame` event flushes;
SceneDB change-window closure. Gamma delivery is queued, so subscribers do
not re-enter a World borrow. Blueprint handlers for component events therefore
run in the script phase after component emission in the earlier
`AfterPhysics` flush. Native Rust handlers queued by a flush are invoked in
the next component callback phase.

`ComponentContext` exposes the component owner and a named event writer but
not `&mut World`. Component code can mutate its own live `&mut self` state and
emit through `ctx.events`; world-level component methods continue to mutate
the authoritative World row via their existing generated dispatch. Rust-side
subscriptions use generated `#[bp_handler("event_name")]` methods, not
arbitrary closures stored on the component.

### Named events, Blueprint pins, and DLL payloads

`#[component_events(class = "...")]` applied to an event trait accepts
`#[bp_event] fn name(arguments...) -> Payload {}` declarations. It generates
`<Trait>EventWriterExt` methods, Gamma descriptors named
`<ComponentClass>.<event>`, a typed VM `EventDecl`, and inventory metadata.
Each declared argument becomes a same-named Blueprint pin, and the return
payload becomes a `payload` pin. The generated writer call takes the original
arguments in declaration order, followed by the return payload; for example,
`fn changed(x: i32, material: String) -> BlockData {}` is emitted as
`ctx.events.changed(x, material, block_data)`. No declared Rust parameter or
payload type is rewritten. Fields are currently transported as individual
Gamma `Bytes` values containing versioned `PSEV` envelopes; the local
`EventDecl` keeps each field's registered object type so Blueprint and native
handlers recover the original typed pins. Arguments must be named concrete
value types (or shared references to concrete value types); generic and
variadic event methods are rejected. The return payload is an owned concrete
named value type, and generated native handlers take owned values in the same
field order.

`ScriptDriver::attach_events` installs generated typed declarations before
linking script subscriptions. `BlockData` is a reflected VM value with `x`,
`y`, `z`, and `material` fields, so handler code can read those values using
the generated script accessors. Rust components may add
`#[bp_handler("block_broken")] fn on_block_broken(&mut self, ctx, data:
BlockData) { ... }` in their `#[register_component_runtime]` impl. For events
with declared arguments, handlers take those argument types in declaration
order before the return payload, after `ctx`. The
generated adapter subscribes on the instance's entity channel; the Gamma
callback only copies dynamic bytes into a host-owned inbox keyed by component
type and generation-bearing entity. The next native component phase
re-resolves and mutably borrows the live row, decodes the VM codec envelope,
and invokes the handler before `tick`. Inbox queues and event subscriptions
are cleared on disable, removal, despawn, scene replacement, and runtime
shutdown. Codec and type names are stable identifiers; the envelope currently
uses JSON payload bytes under version 1. Changing the payload schema requires
a compatible codec or an explicit version change.

### Terrain migration

`VoxelTerrainComponent` now declares `TerrainEvents::block_broken() ->
BlockData`. Its live runtime tick drains the transient pending event list and
calls the generated name-based writer. `set_block`, `fill_sphere`, and
`fill_cube` share `append_edits`; removal brushes inspect the pre-edit planet
and queue one `BlockData` for each solid base cell selected by the resolved
brush. Events are queued only after the edit journal append succeeds. The
pending list is `serde(skip)` and reset on component clone, so it is transport
state on the authoritative component row, not persisted authoring data.

### Remaining boundaries

- Native Rust handlers are limited to generated component event declarations
  and the `#[bp_handler("event_name")]` method convention. A general closure
  subscription API for arbitrary Gamma events is not part of
  `ComponentContext`.
- Dynamic plugin unload cannot safely retain proc-macro-generated function
  pointers or Rust payload codecs after unloading their defining DLL. The
  current registrations are link-time inventory entries for the engine's
  statically linked component crates; unloadable plugin-owned components
  need host-owned registration handles and unregistration hooks.
- Component event declarations support multiple named arguments and one
  structured return payload. Payload schema migration tooling is not yet
  implemented.
