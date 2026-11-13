# ECS (pulsar_scenedb)

The engine's ECS is SceneDB's `World` (`pulsar_scenedb`, the SceneDB
repository): an archetype store with dense `u32` component ids, which also
owns the scene's change journals, object subscriptions and GPU mirror. The
former `pulsar_ecs` crate no longer exists. How Pulsar stores scene objects
and components on it is in [SCENEDB_MIGRATION.md](SCENEDB_MIGRATION.md); the
full API is in SceneDB's README.

## Entity

`#[repr(transparent)]` packed `u64`: the low 32 bits are the slot index, the
high 32 bits the generation, so a stale handle to a recycled slot is
rejected (`World::is_alive`). `Entity::DANGLING` is a sentinel.

## Writes

Every write goes through the `World`, which runs the same hooks for each
path (change journal, object subscriptions, GPU mirror):

```rust
let e = world.spawn();
world.insert(e, Transform::default());          // typed insert / overwrite
world.get_mut::<Transform>(e).unwrap().scale = [2.0; 3]; // write on guard drop
world.insert_dyn(e, boxed_value)?;              // type-erased insert
world.get_dyn_mut(e, component_id);             // type-erased write guard
world.remove::<Transform>(e);
world.despawn(e);
```

A `get_mut` guard that is only read does not count as a write.

## Archetypes and component ids

Entities with the same component set share an `Archetype`, whose components
live in dense columns indexed by `ComponentId`. `component_id::<T>()` assigns
ids on first use and caches them per thread.

## Queries

`WorldQuery` is implemented for `&T`, `&mut T`, `()` and tuples:

```rust
for (entity, (transform, velocity)) in world.query::<(&Transform, &Velocity)>() {
    // ...
}
```

A query visits every matching archetype. Incremental consumers do one full
scan and then follow a change cursor instead of querying each frame.

## Change journals and subscriptions

- `world.open_change_cursor::<T>()` / `world.read_changes(&mut cursor, &mut out)`:
  every insert, write and removal of `T` since the cursor's last read, per
  reader. `ChangeRead::Overflowed` means rescan.
- `world.subscribe_object(object, callback)`: called inside every write to
  the object's components with the new value. For views only.

See [SCENEDB_MIGRATION.md#observers](SCENEDB_MIGRATION.md#observers).

## Schedule

An ordered list of systems:

```rust
pub type SystemFn = Box<dyn FnMut(&mut World, GameTime) + Send + 'static>;

let mut schedule = Schedule::new();
schedule.add_system("physics", |world, time| { /* ... */ });
schedule.run(&mut world, time);
```

## Actors

SceneDB has no actor trait or registry of its own (removed, P5b). `TickLoop`
has no registry: a boxed gameplay actor runs through the component lifecycle.

```rust
// pulsar_game::lifecycle
pub trait NativeActor: Send + 'static {
    fn begin_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut ActorCx<'_>) {}
    fn tick(&mut self, _entity: Entity, _world: &mut World, _cx: &mut ActorCx<'_>, _dt: f32) {}
    fn end_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut ActorCx<'_>) {}
}
```

`TickLoop::register_native_actor(actor)` spawns an owner entity (stamped with a
`ScriptTag`, the identity PIE hot reload matches on) and an instance entity
carrying an `ActorCarrier` component linked to it by `ComponentOwner`; the
carrier's table (`EngineActor`, class `NativeActorCarrier`) calls into the box,
which the session scope holds in an `ActorBank` resource. So the actor gets
`begin_play` on the first frame it is ticked, `tick` per frame, and `end_play`
when its instance is removed or disabled **and at the session end** (before a
PIE snapshot restore; SceneDB's registry never ended actors at shutdown).
Hot reload re-attaches: the same class registered again on a surviving tagged
owner gets a fresh instance entity, the old behavior is dropped at the session
end while its code is still loaded. Generated Rust Blueprint actors implement
`NativeActor` by its full path.

## Component lifecycle (`Actor` on a component)

`NativeActor` above is the boxed, entity-owning object. A component class
gets lifecycle events through `pulsar_component_lifecycle::Actor`, reachable as
`pulsar_game::Actor`, `pulsar_game::prelude::Actor` and
`pulsar_game::lifecycle::Actor`: `impl Actor for T` and
`ClassSpec { actor: Some(ActorTable::of::<T>()), .. }` from the plugin root.
`WorldActor` is the same with `&mut World` and the instance entity in each callback (no
component borrow; physics). `tick_live_components` drives these tables in one
`Step::Components`: `begin_play` when an enabled instance is first seen, `tick` each frame,
`end_play` on disable, removal, despawn or session end (owner-only for `Actor`; the world
form also gets the world, and the component may be gone). `construct` runs when the
component is added or changed, in the editor too, coalesced to once per frame
(`pump_construct`; `engine_backend/src/scene/construct_pump.rs` is the editor's pump).
Every callback gets an `ActorCx` with the session's `SessionScope`: its `SessionKind`
(`Editor`, `PlayInEditor`, `Standalone`, `Headless`; only `construct` runs in `Editor`),
the level's `world_settings`, and typed resources whose `Drop` is the shutdown hook.
A reflected method named like a lifecycle event fails the leak audit
(`pulsar_world_registry::audit::find_lifecycle_problems`).
`#[register_component_runtime]` is the same registration: its expansion is an
`ActorTable` (with native `#[bp_handler]` events, subscribed at `begin_play`
and delivered before `tick`) submitted as an `EngineActor`; the old raw
`inventory` tick registration is retired (P6). Plan:
`COMPONENT_LIFECYCLE_PLAN.md`.

## Components

Engine component classes are plain typed components registered with
`#[register_world_component]` (`pulsar_world_registry`), mirrored to the GPU
with `#[derive(SceneStore)]`/`#[gpu]`, and attached to scene objects as
component-instance entities. There is no JSON component store; component
lifecycles run on the live typed value.
