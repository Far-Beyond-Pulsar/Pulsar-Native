# ECS (pulsar_ecs)

Archetype-based ECS with dense `u32` component indexing, `u64` bitmask
filtering for fast archetype matching, and `swap_remove` slot reuse.

## World

```rust
pub struct World {
    pub entity_slots: Vec<EntitySlot>,       // index → generation
    pub free_slots: Vec<u32>,                // recently despawned slots
    pub archetypes: Vec<Archetype>,           // group by component set
    pub archetype_index: AHashMap<ArchetypeKey, ArchetypeId>,
}
```

## Entity

`#[repr(transparent)]` packed `u64` — lower 32 bits are the slot index, upper
32 bits are the generation (for dangling detection):

```rust
pub struct Entity(u64);
pub const DANGLING: Entity = Entity(u64::MAX);
```

## Archetype

Groups entities with the same set of component types. Components are stored in
dense `Vec<T>` columns:

```rust
pub struct Archetype {
    pub id: ArchetypeId,
    pub key: ArchetypeKey,
    pub active_cids: Vec<ComponentId>,
    pub columns: Vec<Option<Box<dyn ErasedColumn>>>,
    pub entities: Vec<Entity>,
    pub mask: u64,     // bitmask for first 64 component IDs
}
```

## Component IDs

Dense `u32` identifiers, assigned sequentially from 1. Obtained via:

```rust
component_id::<T>()  // thread-locally cached; fast path is linear scan of local cache
```

## Queries

`WorldQuery` trait implemented for `&T`, `&mut T`, `()`, and tuples up to 8
elements:

```rust
pub trait WorldQuery<'w>: Sized {
    type Item;
    fn matches(archetype: &Archetype) -> bool;
    unsafe fn fetch(archetype: &'w Archetype, row: usize) -> Self::Item;
}
```

Usage:
```rust
let mut results: Vec<(Entity, &Transform, &mut Velocity)> = Vec::new();
world.query::<(&Transform, &mut Velocity)>().for_each(|entity, (transform, velocity)| {
    results.push((entity, transform, velocity));
});
```

## Schedule

Ordered list of systems run each tick:

```rust
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}
pub type SystemFn = Box<dyn FnMut(&mut World, GameTime) + Send + 'static>;
```

## Actor trait

Autonomous objects with lifecycle callbacks:

```rust
pub trait Actor: Send + Sync + 'static {
    fn begin_play(&mut self, _world: &mut World) {}
    fn tick(&mut self, _dt: f32, _world: &mut World) {}
    fn end_play(&mut self, _world: &mut World) {}
}
```

`ActorRegistry` owns boxed actors and drives their lifecycle each tick.

## ComponentStore

Bridge between the reflection-based `EngineClass` system and the ECS. Stores
`(String, Box<dyn EngineClass>)` pairs and provides JSON-based property access
(`set_property_json`, `get_property_json`, `call_method_json`) for the
blueprint graph runtime.
