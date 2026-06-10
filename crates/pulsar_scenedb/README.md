# pulsar_scenedb

Archetype-based Entity Component System for the Pulsar engine.

## Design

- **Dense `u32` ComponentId** â€” assigned on first access via `component_id::<T>()`. Thread-local cache for hot paths, global `Mutex` registry for cold registration. Avoids `TypeId` HashMap overhead and macOS linker ICF issues.
- **`Vec<Option<Box<dyn ErasedColumn>>>` storage** â€” columns indexed directly by `ComponentId.0`, no hashing on lookups.
- **`u64` archetype bitmask** â€” fast pre-filtering: a component type's bit is set at archetype creation. Queries check the mask before touching any column.
- **`swap_remove` slot reuse** â€” entity removal swaps in the last entity, updating its location. No tombstones, no compaction passes.
- **`thread_local!` + `Mutex` for CID registry** â€” avoids `OnceLock` linker ICF collision on macOS.

## Performance

| Benchmark | Rate | Notes |
|---|---|---|
| Spawn throughput (2 components) | ~1.3M entities/sec | Vec push, no hashing |
| Query traversal (3-component dense) | ~6.3M items/sec | Bitmask pre-filter + Vec index |
| Archetype migration | ~584K migrations/sec | Pre-populated target columns |
| Sparse query (200 archetypes) | ~6.6M items/sec | Bitmask skips non-matching archetypes |

All measurements on Apple Silicon (M-series). Run `cargo bench -p pulsar_scenedb` for current numbers.

## Usage

```rust
use pulsar_scenedb::{World, Component, QueryIter, WorldQuery};

struct Pos(f32, f32);
struct Vel(f32, f32);

let mut world = World::new();

// Spawn
let e = world.spawn((Pos(0.0, 0.0), Vel(1.0, 0.0)));

// Insert
world.insert(e, (Pos(10.0, 20.0),));

// Query
for (pos, vel) in world.query::<(&Pos, &Vel)>() {
    // ...
}

// Despawn
world.despawn(e).unwrap();
```

## Modules

| Module | What |
|---|---|
| `component` | `ComponentId`, `Column<T>`, `ErasedColumn` trait, `component_id!` |
| `entity` | `Entity` (u32 handle + generation) |
| `archetype` | `Archetype`, `ArchetypeId`, `ArchetypeKey`, bitmask |
| `world` | `World` â€” entity spawn/despawn, insert/remove, query, migrate |
| `query` | `WorldQuery` trait (tuples of refs), `QueryIter` |
| `schedule` | `Schedule` â€” system ordering and execution |
| `actor` | `Actor`, `ActorRegistry` â€” autonomous entity wrappers |
| `component_store` | `ComponentStore` â€” blueprint runtime storage |

## Safety

All unsafe blocks have `// SAFETY:` comments explaining the invariants. The test suite includes adversarial tests (interleaved mutations, dangling entity rejection, component churn) in addition to correctness tests and self-timing performance checks.

## Test

```sh
cargo test -p pulsar_scenedb
cargo bench -p pulsar_scenedb
```
