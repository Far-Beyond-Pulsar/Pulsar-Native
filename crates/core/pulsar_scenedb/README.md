# pulsar_scenedb

A cross-device ECS. One index space for CPU and GPU.

The handle is a `u64`. Slot index in the lower 32 bits, generation in the upper 32. On the CPU it resolves through the slot registry to a row offset in a `CellStorage` page. On the GPU it resolves through the slot-mirror SSBO to a row offset in the transform buffer. Same value, same meaning, both sides.

GPU-native columns track their own dirty rows. At the frame boundary the storage layer coalesces those marks into the smallest possible write ranges and issues the uploads. Zero dirty rows means zero GPU writes. No manual shadow buffers, no per-frame full-region uploads, no separate sync layer to maintain.

Remote access works. Read a GPU-native field from the CPU and SceneDB transfers the data transparently. A tracing warning fires so you can find and fix the cross-device access. Development keeps moving. The crash comes later, when you want it, not when the borrow checker or the GPU hangs.

## Handles

`Handle` packs a 32-bit slot index and a 32-bit generation counter. Generation 0 is permanently invalid. Live handles start at generation 1.

```rust
let h = Handle::new(14, 2);
h.index();      // 14
h.generation(); // 2
h.is_valid();   // true
```

The slot index is stable for the lifetime of the allocation. The registry absorbs compaction moves and slot recycling behind the scenes. The handle never changes. Only the generation bumps on free, which makes stale handles immediately detectable.

`HandleRegistry` owns the slot table. Allocate returns a fresh handle pointing at a row. Free bumps the generation and pools the slot. A slot whose generation reaches `u32::MAX` is permanently retired. No recycling, no aliasing.

## Delta-minimal GPU uploads

This is the reason the crate exists. The storage layer tracks what changed so the upload layer sends exactly those bytes.

Each `SceneGpuStore` holds a `DirtyMask` per mirrored column per cell. `write_transform` writes the CPU column and marks the row. At the frame boundary `sync_all` iterates over marked rows, coalesces adjacent marks into ranges, and issues one `write_buffer` per range. Ranges are contiguous. A frame where nothing changed issues zero GPU writes.

The `legacy_model_bench` benchmark compares this against a full-upload baseline. At 0.1% mutation on 100k objects the SceneDB path transfers 6,400 bytes and the legacy path transfers 6.4 megabytes. The CPU time difference is 8x. At 100% mutation the overhead of tracking catches up and the two paths converge. The crossover point is the same across every scene size tested. Below that crossover, SceneDB wins by a wide margin. Above it, the legacy full-upload path is slightly faster because there is nothing to track.

## GPU-native fields

A component field becomes GPU-native by appearing in the cell type description. The cell type declares which columns exist and which token types they hold. SceneDB handles the rest.

`SpatialCell::with_transform` creates a cell with six positional f32 columns, a `[f32; 16]` transform column, and an `InstanceInfo` column. The transform and instance-info columns are token-registered. They show up in the page layout and the GPU store picks them up at registration time.

```rust
let mut cell = SpatialCell::with_transform(1024).unwrap();
let h = cell.alloc(aabb).unwrap();

let sim = driver.begin();
store.write_transform(id, &mut cell, h, &matrix, &sim);
store.write_instance_info(id, &mut cell, h, InstanceInfo { mesh_index: 42, flags: 0 }, &sim);
```

On the GPU side each cell gets a region in the shared SSBO. `register_cell` allocates the region, seeds the generation buffer, marks every occupied row dirty, and sets up the shadow state for delta-minimal writes. The slot mirror is warmed up by the boundary scan on its first pass, not at registration time.

Columns without a GPU mirror stay CPU-only. No tracking overhead, no upload cost. The distinction is in the column list, not in the API.

## Phase machine

Frame phases are types. `SimulateA` and `SimulateB` permit mutation. `HarvestPhase` permits reads only. `BoundaryPhase` runs the retire-compact-sync pipeline. Crossing from one to the next consumes the witness. Calling `write_transform` with a `HarvestPhase` reference is a compile error. The crate enforces this with a sealed trait and a `compile_fail` doc test.

A full frame looks like this:

```rust
let mut driver = FrameDriver::new();
let sim = driver.begin();              // SimulateA

// mutation happens here
store.write_transform(id, cell, h, &m, &sim);

let harvest = sim.end().end();         // SimulateA -> SimulateB -> HarvestPhase
// reads happen here
let boundary = harvest.end();          // HarvestPhase -> BoundaryPhase
let stats = boundary.run(&mut store, cells);  // retire -> compact -> sync
```

`SimulateA` and `SimulateB` exist as separate types because gameplay and physics writeback are expected to land in different scheduling buckets once the physics subsystem is integrated. For now both accept any `SimulateWitness` and the distinction is purely documentary.

`BoundaryPhase::run` is the all-in-one path. The individual stages are also exposed as consuming transitions for tests that need to observe state between them.

```rust
let (retired, drained) = boundary.retire(&mut store, cells);
// inspect store state after retire, before compact
let compacted = retired.compact(&mut store, cells);
let stats = compacted.sync(&mut store, cells);
```

`retire_all` drains every cell's deferred-retire queue against the completed-submission watermark. `compact_all` runs swap-and-pop compaction and marks moved rows dirty. `sync_all` uploads dirty rows and runs the self-healing slot-mirror boundary scan. Each stage advances the internal phase enum. Calling `retire_all` twice in the same boundary is a debug assertion failure.

## Self-healing slot mirror

The slot mirror is a `SceneBuffer<u32>` indexed by GPU row. Each entry holds `slot_base + local_slot`, which is the global slot ID the shader uses for handle validation.

`sync_all` runs a boundary scan over every occupied row. It compares a per-row shadow against the authoritative slot column. On mismatch it marks the row dirty, calculates `slot_base + slot_column[row]`, writes it into a scratch buffer, and uploads. This catches every staleness path: writes after alloc, compaction moves, allocs that re-occupy a compaction-vacated row. No per-event trigger can miss a case because the scan checks every row every frame.

The shadow starts at `u32::MAX` for every row, which will never match a real slot. On the first frame every occupied row gets uploaded. After that only rows whose slot actually changed trigger a write.

## Compile-time checks

`ErasedColumn` is a `pub(crate)` trait. External code cannot implement it. `Archetype::columns` is public but its contents are opaque outside the crate. The type system prevents column type confusion.

`SimulateWitness` is sealed. No external type can implement it. Mutation APIs are gated on `&impl SimulateWitness`. A `HarvestPhase` reference fails to compile where a `SimulateA` reference passes.

Generation zero is permanently invalid. `Handle::INVALID` is all zeros. The registry starts allocations at generation 1. Handles reaching `u32::MAX` on free are permanently retired. These invariants are enforced in the allocator, not in the caller.

## Performance contracts

Delta uploads scale with mutation count, not total entity count. A frame where nothing changes issues zero GPU writes and the boundary scan does one compare per occupied row per cell. That compare is a `u32` load, a `u32` load from the shadow, and a branch. On x86 the branch is predictable (same values every frame in the steady state) and the overhead is a few nanoseconds per row.

Compaction is swap-and-pop. Removal marks a row dead. Compaction at the frame boundary swaps the last live row into the hole and pops the tail. Handles that pointed at moved rows are redirected by the compaction callback. The slot registry is updated before the next frame starts. Compaction cost is proportional to the number of holes, not the total entity count.

Spatial queries use the scalar reference implementation by default. The SIMD dispatcher checks for AVX2 at runtime and falls back to scalar if unavailable. The AVX2 paths are verified bit-identical against the scalar reference at every call site through the `query_aabb_scalar_for_bench` and `query_frustum_scalar_for_bench` seams. The `simd` module contains both implementations plus the unused `compress_tokens` and `compress_tokens_avx2` routines that will land in a future harvest-compaction stage.

## Structure

The crate splits into two tiers.

Tier 1 is the archetype ECS. `World` owns entity slots, archetypes, and a hashmap from archetype key to archetype ID. Components are dense `Vec<T>` columns indexed by `ComponentId`. Queries match archetypes by bitmask and fetch rows through pointer access on the erased column trait. Spawning into an archetype pre-reserved with `reserve_entities` avoids reallocation.

```rust
let mut world = World::new();
world.reserve_entities(1000);
for _ in 0..1000 {
    world.spawn();
}
for (pos,) in world.query::<(&Pos,)>() {
    // direct pointer access, no downcast per row
}
```

Tier 2 is the SceneDB storage layer. `CellStorage` owns a `Page`, a `LivenessMask`, and a `HandleRegistry`. The page is a single allocation with 64-byte-aligned SoA columns. Column 0 is always the slot ID column. User columns are addressed by index starting at 1. `alloc` pushes a row and issues a handle. `free` marks the row dead. `compact` reclaims dead rows with swap-and-pop. Token-keyed column access resolves a `TypeToken` to a user column index.

```rust
let mut cell = CellStorage::new(&[ColumnDesc::of::<f32>()], 256).unwrap();
let h = cell.alloc().unwrap();
let row = cell.row_of(h).unwrap();
cell.user_column_mut::<f32>(0)[row as usize] = 1.0;
```

The `gpu` feature adds the GPU tier on top. The storage layer itself stays graphics-free. All GPU code lives behind `#[cfg(feature = "gpu")]` and is guarded by a CI check that runs `cargo check --no-default-features`. The wgpu dependency is pinned to a specific version on crates.io, separate from the workspace-wide wgpu fork that the rest of the engine uses.

## Feature flags

Two configurations exist. The base crate has no GPU dependencies and compiles on any target that supports `std`. The `gpu` feature adds wgpu and the full SceneDB GPU layer.

| Flag | What it enables |
|------|----------------|
| none | CPU ECS, `CellStorage`, spatial queries, `HandleRegistry`, `LivenessMask`, `Page` |
| `gpu` | `SceneGpuStore`, `SceneBuffer`, `GenerationBuffer`, `FrameDriver`, phase machine, asset store |

## Quick start

```rust
use pulsar_scenedb::*;

let mut world = World::new();
world.reserve_entities(1000);
let entities: Vec<_> = (0..1000).map(|_| world.spawn()).collect();
for &e in &entities {
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Vel(0.0, 0.0, 0.0));
}

let mut sum = 0.0;
for (pos,) in world.query::<(&Pos,)>() {
    sum += pos.0;
}
```

With the `gpu` feature:

```rust
use pulsar_scenedb::gpu::*;

let mut cell = CellStorage::new(&[ColumnDesc::of::<[f32; 16]>()], 1024).unwrap();
let h = cell.alloc().unwrap();

let mut store = SceneGpuStore::new(&ctx, config);
let id = store.register_cell(&cell, 0).unwrap();

let mut driver = FrameDriver::new();
let sim = driver.begin();
store.write_transform(id, &mut cell, h, &[0.0; 16], &sim);
let boundary = sim.end().end().end();
let stats = boundary.run(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
```

## Benchmarks

Measured on an optimized build (cargo bench profile). All improvements are against the original archetype ECS baseline before the `get_raw`, `merge-insert`, and `reserve_entities` changes landed.

### Spawn throughput

Entities are spawned into a fresh world. The `empty` case spawns into the null archetype. `with_component` spawns and inserts `Pos(f32, f32, f32)`. `with_4_components` spawns and inserts `Pos`, `Vel`, `Health`, and `Tag`.

```
spawn/empty/100         279 ns   359 Melem/s   51% faster
spawn/empty/1000        1.84 µs  545 Melem/s   28% faster
spawn/empty/10000       17.9 µs  560 Melem/s   20% faster
spawn/empty/50000       92.8 µs  539 Melem/s   22% faster

spawn/with_component/100     4.66 µs   21.5 Melem/s   11% faster
spawn/with_component/1000    41.4 µs   24.2 Melem/s    9% faster
spawn/with_component/10000   411 µs    24.3 Melem/s   25% faster
spawn/with_component/50000   2.10 ms   23.8 Melem/s    9% faster

spawn/with_4_components/100     31.4 µs   3.19 Melem/s   25% faster
spawn/with_4_components/1000    298 µs    3.36 Melem/s   25% faster
spawn/with_4_components/10000   3.00 ms   3.33 Melem/s   25% faster
spawn/with_4_components/50000   14.9 ms   3.35 Melem/s   23% faster
```

The empty spawn numbers show the clearest improvement. `reserve_entities` eliminates Vec reallocation churn during the batch loop. At 50,000 entities the empty path runs at 539 Melem/s compared to 422 Melem/s at the baseline.

The 4-component case improves consistently across every size. This is the merge-insert for `ArchetypeKey::with`. Each spawn runs four inserts, each insert builds a new archetype key. The old code cloned the Vec and sorted it. The new code preallocates the exact capacity and merges in a single pass.

### Query iteration

Two query shapes. The single benchmark queries `(&Pos, &Health)` across a world where 10% of entities have `Vel` instead of `Pos` and are skipped by the archetype filter. The 8-tuple benchmark queries all eight component types on 10,000 entities.

```
query_single/iter/100      1.21 µs   82.8 Melem/s   8% faster
query_single/iter/1000     11.9 µs   84.0 Melem/s   9% faster
query_single/iter/10000    119 µs    84.4 Melem/s   8% faster
query_single/iter/50000    595 µs    84.1 Melem/s   7% faster

query_8_tuple/iter         525 µs    19.1 Melem/s   7.5% faster
```

The improvement comes from replacing `as_any().downcast_ref::<Column<T>>()` with a direct `get_raw(row)` call on the erased column trait. Each fetch skips the TypeId comparison and the second `unwrap_unchecked`. For the 8-tuple query that removes 8 dynamic type checks and 8 unwraps per entity, or 80,000 type checks per frame at 10,000 entities.

### Archetype migration

Entities move between archetypes as components are added and removed. `insert_component` spawns 10,000 entities then inserts `Pos` on each one. `add_then_remove` spawns 10,000 entities, inserts `Pos` and `Health`, then removes `Health` from each.

```
archetype_migration/insert_component   416 µs   12% faster
archetype_migration/add_then_remove    1.81 ms  18% faster
```

The merge-insert in `ArchetypeKey::with` avoids the sort-and-dedup pass on every migration. The old code cloned the Vec, pushed the new ID, sorted, and deduped. The new code checks for the existing ID first and returns early if it is already present, or allocates the exact capacity and merges in one pass otherwise.

### Delta-sync vs full upload

The `legacy_model_bench` benchmark (gpu feature) compares SceneDB delta-sync against a full-upload baseline. It sweeps scene sizes from 1,000 to 100,000 entities and mutation percentages from 0% to 100%.

At 100,000 objects with 0.1% mutation:

```
scenedb:    168 µs CPU,   6,400 bytes transferred
legacy:   1,354 µs CPU, 6.4 MB  bytes transferred
```

Speedup is 8x on CPU time and 1003x on bytes transferred. The upload is a single coalesced range for the 0.1% of rows that changed. The legacy path re-uploads every row every frame regardless.

At 100% mutation the two paths converge. SceneDB issues slightly more CPU work because it runs the dirty mask scan before giving up and uploading everything. The crossover point sits at the 100% mutation level across all scene sizes tested.

### Running the benches

```
cargo bench -p pulsar_scenedb --bench ecs_bench
cargo bench -p pulsar_scenedb --features gpu --bench legacy_model_bench
cargo bench -p pulsar_scenedb --features gpu --bench scenedb_bench
```

## Design documents

The crate follows a written specification. The spec defines the handle format, the storage layout, the phase machine transitions, the memory ordering contracts, and the validation invariants. The implementation is audited against the spec at each milestone.

`docs/superpowers/specs/SceneDB2.0.md` covers the full specification at Rev 2.3. `docs/superpowers/specs/CONTRACTS.md` lists the design contracts C0 through C6. `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md` covers the implementation strategy. `docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md` is the cross-milestone conformance audit.
