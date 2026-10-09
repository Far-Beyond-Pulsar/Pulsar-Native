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

```rust
pub trait Actor: Send + Sync + 'static {
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {}
    fn end_play(&mut self, _entity: Entity, _world: &mut World) {}
    fn tick(&mut self, _entity: Entity, _world: &mut World) {}
}
```

`ActorRegistry::register` spawns the actor's entity and calls `begin_play`;
`tick_all` ticks every actor. `pulsar_game`'s tick loop owns one registry
next to its `Schedule`.

## Components

Engine component classes are plain typed components registered with
`#[register_world_component]` (`pulsar_world_registry`), mirrored to the GPU
with `#[derive(SceneStore)]`/`#[gpu]`, and attached to scene objects as
component-instance entities. There is no JSON component store; component
lifecycles run on the live typed value.
