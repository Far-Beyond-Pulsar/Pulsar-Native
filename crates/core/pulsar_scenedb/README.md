# pulsar_scenedb

A **cross-device ECS**: one index space, two devices.

Every engine copies data to the GPU every frame. Most make you write the bridge
code by hand — command buffers, staging buffers, dirty flags, sync fences.
SceneDB flips it: the entity index is the same on CPU and GPU. A component
declares where it lives; accessing it natively is zero-cost, accessing it
remotely triggers an implicit transfer (with a tracing warning so you know).

```rust
// This is a GPU-native component. SceneDB mirrors it automatically.
struct InstanceData {
    transform: [f32; 16],
}

// CPU code reads/writes it through the same handle — no staging, no enqueue.
cell.write_transform(handle, &matrix, &sim);
// On the GPU, the same handle indexes directly into the SSBO.
```

The point isn't to hide the GPU. It's to make the bridge so natural that you
stop thinking about which device owns what — and when you *do* need to care,
the tools (tracing warnings, per-field locality, delta-sync stats) are right
there.

## Why this exists

Every frame, engines upload scene data to the GPU. The naive approach—upload
everything—wastes bandwidth on unchanged data. The efficient approach—track
what changed, upload only deltas—requires a bespoke dirty-tracking and sync
layer per buffer. The safest approach—handle stale/dead entity reads gracefully
instead of crashing—requires runtime validation most engines skip because it's
too expensive.

SceneDB solves all three with one mechanism: **unified handles**.

A `Handle` (packed u64: slot index + generation) is the same value on CPU and
GPU. Both sides resolve it through the same slot table. On the CPU the slot
registry maps to a row in a `CellStorage` page. On the GPU the slot-mirror
SSBO maps to a row in the transform buffer. Compaction moves the data; the
handle stays valid. Retirement bumps the generation; stale handles are
rejected instantly.

**Delta-minimal uploads come free**—each mirrored column carries a per-row
dirty mask. At the frame boundary, only marked rows are uploaded, coalesced
into the fewest write ranges. Zero-dirty frames issue zero GPU writes. No
manual shadowing, no separate sync layer.

**GPU-native fields are declarative.** Tag a column in the cell type
description and SceneDB mirrors it automatically: dirty tracking, coalesced
delta upload, generation shadowing, slot-mirror self-healing. The distinction
between CPU-local and GPU-native is a column attribute, not an architectural
decision.

The handle-based resolution also provides a **safety net**: access a GPU-native
field from the CPU and SceneDB handles the transfer transparently—with a
tracing warning so you know it happened. Development stays fluid; you fix the
cross-device access when you're ready, not when the engine crashes.

## Architecture

Two storage tiers, one index space:

### Tier 1 — Archetype ECS (CPU)

Classic archetype-based ECS with dense `Vec<T>` columns, `u64` bitmask
filtering, swap-remove slot reuse, and `u32` component IDs. Spawn entities,
insert/remove components, query with tuples up to 8 components:

```rust
let mut world = World::new();
let e = world.spawn();
world.insert(e, Pos(1.0, 2.0, 3.0));
world.insert(e, Vel(0.0, 0.0, 0.0));

for (pos, vel) in world.query::<(&Pos, &Vel)>() {
    // zero-cost slice iteration — no boxing, no hashing
}
```

### Tier 2 — SceneDB Storage (CPU + GPU, `gpu` feature)

Persistent SoA pages (`CellStorage`) with deferred swap-and-pop compaction,
atomic liveness masks, and slot→row indirection that survives compaction.
The `gpu` feature adds:

- **`SceneGpuStore`** — region-partitioned SSBOs with coalescing delta-sync.
  Only dirty rows are uploaded each frame — bytes transferred = `O(mutations)`,
  not `O(total entities)`.
- **Compile-time phase machine** — `SimulateWitness` / `HarvestPhase` /
  `BoundaryPhase` witness types make frame-ordering violations a compile error.
- **Self-healing slot mirror** — boundary scan catches every slot-staleness
  path (writes, compaction moves, allocs into vacated rows) in one invariant.
- **Asset store** — `MeshRegistry`, `ClusterBuffer`, `TextureStore` with
  corrupted-VRAM rebuild gates.
- **Streaming grid** — concentric domains (Outer/Margin/Inner) with
  hysteresis-banded promotion/demotion and VRAM budget enforcement.

## Quick start

```rust
use pulsar_scenedb::*;

// Tier 1 — CPU ECS
let mut world = World::new();
world.reserve_entities(1000);
for _ in 0..1000 {
    let e = world.spawn();
    world.insert(e, Transform::default());
}
let count = world.query::<(&Transform,)>().count();
```

With the `gpu` feature:

```rust
use pulsar_scenedb::gpu::*;

let mut store = SceneGpuStore::new(&ctx, config);
let mut driver = FrameDriver::new();
let sim = driver.begin();
store.write_transform(cell_id, &mut cell, handle, &matrix, &sim);
let boundary = sim.end().end();  // SimulateA → HarvestPhase → BoundaryPhase
let stats = boundary.run(&mut store, &mut [CellSlot { id: cell_id, cell: &mut cell }]);
// stats.ranges, stats.bytes — delta-minimal per frame
```

## Feature flags

| Flag | Description |
|------|-------------|
| *(none)* | CPU-only ECS + SceneDB storage. No GPU dependencies. |
| `gpu` | GPU-resident store, delta-sync, phase machine, asset store. Adds `wgpu` dep. |

The core crate is **graphics-free** (`--no-default-features`) by design —
CONTRACTS.md C0.

## Benchmarks

```
# CPU-only ECS benchmarks
cargo bench -p pulsar_scenedb --bench ecs_bench

# SceneDB delta-sync vs legacy full-upload
cargo bench -p pulsar_scenedb --features gpu --bench legacy_model_bench

# Full GPU benchmark suite (spatial query, harvest, DEI compaction)
cargo bench -p pulsar_scenedb --features gpu --bench scenedb_bench
```

## Design docs

- `docs/superpowers/specs/SceneDB2.0.md` — full specification (Rev 2.3)
- `docs/superpowers/specs/CONTRACTS.md` — design contracts C0–C6
- `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`
- `docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md`
