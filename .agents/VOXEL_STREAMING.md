# Voxel edit streaming on SceneDB residency (spec, not implemented)

Status: proposal. Nothing here is built. Written after `voxel_craft` filled
memory at ~20-30 MB/s (measured below) and exposed that a `Planet` has no
tier below RAM for its edits.

## 1. What we have today (verified in the code)

### 1.1 Voxel planets hold every baked edit in RAM, forever

- `Planet.edits.baked: EditStore` (`helio-pass-voxel-planet/src/edit_store.rs`)
  is a sparse map of columns -> `Arc<Column>` = `Vec<(bk, Arc<Brick>)>`; a
  `Brick` is 8x8x8 `CellEdit` (u16) = 1 KB. Every level (0 = base, coarser by
  majority) is stored, so one edited region costs its bricks at all levels.
- Nothing evicts: `Planet::apply_baked` / `seal` only add or replace. README:
  "memory follows what the edits left, not how many there were". That bounds
  *brush count*, not *footprint*. `undo_seals` (128) and `changes`
  (`CHANGES_KEPT`) are the only bounded parts (the latter now also capped by
  columns, `CHANGES_COLUMNS_KEPT`).
- The Pulsar editor is fine in practice because its edits are human-scale
  (a `VoxelEditJournal` of strokes, replayed or loaded from an `HVPS`
  snapshot, `snapshot.rs`). The journal is chunked + prefix-hashed in RAM and
  saved with the level. Nothing pages either of them.
- So the existing planets do *not* avoid this; they just never get edit
  volumes large enough to notice. A procedurally decorated planet is exactly
  that volume.

### 1.2 Measured cost (voxel_craft, 1 m blocks, jungle/forest)

| decorated regions (32x32 blocks) | extra working set | per region |
|---|---|---|
| 0 | baseline (~0.9 GB ws, mostly GPU-side commit) | - |
| ~5,000 (radius 40) | +~650 MB | ~100-130 KB |
| radius 2048 | grows ~22 MB/s until the whole face (~640k regions) is covered | ~65-80 GB total |

Interim mitigation in the example: `MAX_REGIONS = 5000` in `config.rs`
(decoration stops; far world is bare). This spec replaces that cap.

### 1.3 What SceneDB (rev 8b5b114, "asset residency") provides

| Piece | Where | Shape | Fit for voxel edits |
|---|---|---|---|
| `Residency<T>` | `SceneDB/.../residency.rs` | process-wide byte-budgeted LRU of **rebuildable** values; `get` (single-flight rebuild), `prefetch` on loader threads, `Arc` keeps in-use values alive, one oversize value allowed while MRU; stats | Right shape for *derived* columns. Not a store: a value must come with a rebuild closure; dropping loses it. |
| `derive_cached` / `prune_derived_cache` | `derived.rs` | disk tier, `<project>/.pulsar/cache/derived/<kind>/<key>.pdd`, temp+rename, header check, LRU prune by size | Key today = (kind, **source file path + len + mtime**, settings, version). Procedural data has no source file: needs a keyed variant (see 4.3). Not for authoritative data (it is a cache; files can be pruned). |
| `StreamingGrid` (`gpu/grid.rs`) | `Outer -> Warm -> Margin -> Inner` per 2D cell, hysteresis, `RamHooks { stage, evict }`, `execute_transitions_with_ram`, budgets (`ram_budget`, `max_ram_cached_cells`) | XZ-planar cells, Y unbounded. The `Warm` tier is exactly "source data in system RAM, no VRAM". Caveats: LRU eviction of warm cells under pressure is documented **future work**; it is **not used by Pulsar-Native at all today** (no references in `crates/`). |
| Settings | `streaming.cpu_cache_mb` (1024), `derived_cache_mb` (4096) | project settings, `.agents/MEMORY.md` | Reuse; add voxel-specific keys (6). |
| Memory Explorer | `ui_log_viewer` | residency + derived stats, heap, GPU, process | Voxel tiers should report here. |

Not provided: any authoritative (must-not-lose) paging store, any voxel/column
awareness, any cube-sphere topology (the grid is a flat XZ plane), any link to
`Planet`.

### 1.4 Who reads the baked store (all must keep working)

- CPU: `Planet::material/solid/kind` (`planet.rs` ~475, 513), `solid_top`
  (811), raycast/column walks (875) -> `baked.cell/column/solid_top`.
- Renderer: `ChunkWorld` keeps its own `EditStore` clone and reads columns
  for tile/chunk jobs (`chunks/mod.rs` ~2294, 2448); `sync_edits` uses
  `Edits::baked_changes_since` (the bounded change log); `BlockMaterials`
  (`chunks/blocks.rs`) holds a store clone and loads level-0 columns near the
  eye into the GPU exact-material table.
- Persistence: `snapshot.rs` iterates `baked.bricks()`.
- History hash (`sealed_hash`) identifies every state; a different history
  makes `sync_edits` return false = full world rebuild.

## 2. Goals / non-goals

Goals
1. Memory is bounded by a budget, independent of how much of the planet has
   ever been decorated or edited.
2. Player edits are never lost (authoritative tier on disk).
3. Decoration (derived from seed + recipe) is regenerated on demand and
   cached on disk; evicting it is free of data loss.
4. Same visual result: distant detail still drawn via coarse levels; no
   extra hitching or "flash" on page-in; CPU and GPU still read identical
   cells.
5. Observable in Memory Explorer; budgets in project settings.

Non-goals (this spec): multiplayer replication of edits, changing the GPU
generation shaders' brick format, GPU-side decoration (see option C).

## 3. Core idea: split the store by who owns the truth

Today one `EditStore` mixes two kinds of data:

| Layer | Truth is... | Size | Tier policy |
|---|---|---|---|
| **Player edits** (`Authoritative`) | the data itself | small (strokes) | RAM working set + write-back to disk store; never dropped without being on disk |
| **Generated** (`Derived`, trees/structures/anything a pure function of `(seed, recipe fingerprint, generator version, region)`) | the generator | huge | RAM = `Residency`, disk = derived cache, rebuild on miss |

Read order is unchanged (`recent(baked(large(terrain)))`); `baked` becomes
`authoritative ∘ derived`: a cell reads the authoritative brick if present,
else the derived one, else terrain. A player edit into a generated region
writes the *authoritative* brick (copy of the derived brick with the change)
so regenerating the derived layer never clobbers it.

### 3.1 Interface (sketch)

```rust
/// A column source below the store's RAM map.
pub trait ColumnSource: Send + Sync {
    /// All levels' bricks of one column (face, ci, cj), or None if empty.
    fn column(&self, key: &ColumnKey) -> Option<Arc<Column>>;
    /// Cheap "might this have bricks?" so queries skip empty space without a load.
    fn may_have(&self, key: &ColumnKey) -> bool;
}

pub struct EditStore {
    authoritative: ShardedMap<ColumnKey, Arc<Column>>,   // today's shards
    derived: Option<Arc<dyn ColumnSource>>,              // new: generated layer
    ...
}
```

`EditStore::column()` becomes `authoritative.get().or_else(derived.column())`,
so every existing reader (`cell`, `solid_top`, renderer jobs) works unchanged.
`derived` for voxel_craft = region generator wrapped in
`Residency<Column>` keyed by `(face, ci, cj, level)`.

### 3.2 Granularity: the unit is the generation region, not the brick

Decoration is generated per 32x32-block region and touches columns
(ci, cj) of 8x8 blocks => 16 columns/region at level 0, plus the coarser-level
bricks over them. The residency/disk unit should be a **region blob**
(`{face, rx, ry}` -> all levels' columns it owns), because:
- it is what the generator naturally produces (and `decorate_all` already
  builds per region, in parallel);
- one disk read/`Residency::get` per region, not per brick;
- coarse bricks at level L>=1 span several regions: store them as a separate
  `CoarseTile` blob (per level, per 2^L-aligned tile) derived by majority
  from base columns (`rebuild_coarse` already does this) so far view does not
  require every base region resident. This is the part that needs care
  (section 5.2).

## 4. Tiers and where each lives

```
Terrain (analytic)                       (no memory)
Generated layer:
  VRAM    ChunkWorld leaf/materials      (already budgeted by residency planner)
  RAM     Residency<RegionBlob>          budget: streaming.voxel_cpu_cache_mb
  Disk    derived cache                  budget: streaming.derived_cache_mb
  Cold    the generator (seed + recipe)  rebuild = decorate(region), ~2 ms
Player edits:
  RAM     authoritative working set      bounded by LRU of *clean* columns
  Disk    edit page store (new)          authoritative, write-back, never pruned
```

### 4.1 Residency of the generated layer

- `Residency<RegionBlob>`; `size()` = brick bytes. `get()` on miss runs
  `derive_cached` else `decorate`.
- Prefetch driven by the observer, **per-face 2D cells**: `StreamingGrid` is
  XZ-planar with unbounded Y, which matches "columns (ci, cj) with unbounded k"
  *per cube face*. Use one grid per face with face-local coordinates and an
  observer set projected onto each face; handle faces' edges by also
  registering the observer on the neighbour face when within the margin
  (open question 8.1). Map: `Inner/Margin` = needed for the GPU/physics
  (RAM-resident, pinned while drawn), `Warm` = in `Residency`, `Outer` =
  disk/rebuild.
- Interaction with the existing planner: `ChunkWorld`'s residency planner
  decides which tiles are *drawn*; the generated layer's grid must cover
  strictly more than the draw cut (margin) so a tile job never waits on a
  decorate. A tile job that does hit a miss gets `Residency::get` (sync
  rebuild, ~ms) rather than stalling the frame: acceptable fallback, counted
  in stats.

### 4.2 Authoritative tier (new, small)

- Same column format as `snapshot.rs` (HVPS runs-of-equal-cells + checksum),
  but paged: one file per `(face, 64x64-column page)` under
  `<project>/.pulsar/voxel/<world-id>/pages/`, plus an index.
- Write-back: dirty columns flushed on a background thread (temp+rename), a
  journal tail for crash safety (reuse `record.rs` brush records: append-only,
  replay on top of pages on load).
- RAM: LRU over *clean* columns only (a dirty column is pinned until
  flushed); evicted columns are re-read on demand. Budget
  `streaming.voxel_edit_cache_mb`.
- The editor's `VoxelEditJournal` already persists with the level; this tier
  is for scale. For levels that are small it can stay entirely in RAM (page
  store optional below a threshold, so existing editor behaviour does not
  change).

### 4.3 SceneDB changes needed

1. `derive_cached` keyed variant without a source file:
   `DerivedKey::Procedural { kind, world_id, recipe_fingerprint,
   generator_version, coord }` (hash into the same 128-bit key space, same
   header/temp+rename/prune machinery). Today `DerivedRequest.source: &Path`
   is mandatory and stat'ed (`source_stamp`).
2. `Residency` is fine as is; add (optional) a **pin** (`get_pinned` ->
   guard) so a value in use by a GPU upload is not counted evictable, or rely
   on the existing `Arc` rule (it already never pulls a value from a holder).
3. `StreamingGrid`: finish "LRU-style eviction of warm cells under pressure"
   (documented future work) or replace the warm cap with Residency's byte
   budget as the single source of truth (preferred: one budget, not two).
   Add a non-planar cell topology hook (face-local cells) or keep planar and
   instantiate six.
4. Report voxel tiers through `residency_stats()` / `derived_stats()` so the
   Memory Explorer shows them with no new UI.

## 5. Hard parts (call these out before implementing)

### 5.1 Consistency of CPU and GPU views
CPU queries and the renderer read the same `EditStore`; with a lazy derived
layer both go through `column()`, so they stay bit-identical *as long as the
derived layer is deterministic* (it is: `Rng` keyed by seed+region; add a
test that decorate(region) == decorate(region) across threads and that a
disk round-trip equals a rebuild).

### 5.2 Coarse levels
Level L bricks are majority-built from level L-1. If level-0 regions are
evicted, the coarse bricks over them must not be rebuilt from missing data
(they would read empty -> trees vanish at distance). Options:
- (a) store coarse levels as their own derived blobs (per 2^L tile),
  generated once by loading the covered children (parallel, in the
  generator, off-thread), kept resident much longer than level 0 (coarse is
  a small fraction of bytes). **Recommended.**
- (b) never evict above level N (simple; memory floor grows with world size).

### 5.3 Player edits inside derived regions
Authoritative brick shadows the derived one (3.). Changing the generator
version (`generator_version`/recipe fingerprint) invalidates the *derived*
cache but keeps edits (matches today's HVPS rule: "baked edits do not depend
on the terrain under them"). Edits that remove a tree trunk leave leaves
floating, same as today.

### 5.4 The change log and history hash
`baked_changes_since` and `sealed_hash` track authoritative changes only.
Generated pages entering/leaving residency are *not* history events; instead
they emit a `ColumnsLoaded/ColumnsEvicted` notification the renderer uses to
regenerate the tiles over those columns (today decoration arrives as one
`apply_baked` change, which is how the renderer learns of it). Without this
the renderer keeps drawing stale tiles after a page-in.

### 5.5 Page-in must not flash
Tiles regenerate when columns arrive. Rule: the generated layer is loaded
**before** a tile is first generated (planner margin, 4.1), so first
generation already includes trees; eviction only happens outside the draw
cut + margin, so nothing visible changes. Add a test: walk along a face and
assert no visible tile regenerates after its first draw (use the existing
`HELIO_VOXEL_CHUNKS_*` timing counters).

### 5.6 Cube-face seams
Observer near a face edge needs neighbour-face regions warm; decoration near
edges already exists today (regions are per face), so only the *prefetch
shape* is new.

## 6. Settings (project, namespace `streaming`, in `.agents/MEMORY.md` table)

| Key | Default | What |
|---|---|---|
| `voxel_cpu_cache_mb` | 768 | RAM budget of the generated layer (`Residency<RegionBlob>`) |
| `voxel_edit_cache_mb` | 256 | clean authoritative columns kept in RAM |
| `voxel_warm_radius_m` | 3000 | Warm band beyond the GPU margin |
| `voxel_decorate_radius_m` | 2*view | how far prefetch may rebuild ahead |
| (reuse) `derived_cache_mb` | 4096 | disk cache of regions/coarse tiles |

voxel_craft replaces `MAX_REGIONS` with these (the example reads them from
its own config until it hosts a project).

## 7. Phased plan

1. **Measure + guard (done in the example):** the 100-130 KB/region
   figure, `MAX_REGIONS`, change-log cap, `BlockMaterials` using a shared store
   clone instead of its own index.
2. **Split the layers in Helio** (no SceneDB dependency): `ColumnSource`,
   `EditStore::{authoritative, derived}`, `column()` fall-through; voxel_craft
   moves decoration out of `apply_baked` into a `ColumnSource` whose
   backing is an in-memory map (behaviour unchanged, memory unchanged). Exit:
   all existing planet tests + a CPU/GPU parity test with a lazy source.
3. **Region blobs + Residency:** generator becomes `region -> RegionBlob`
   (all levels), wrapped in `Residency`; budgeted eviction works; renderer
   gets `ColumnsLoaded/Evicted`. Exit: memory plateaus at the budget while
   walking 20 km; no visible regeneration (5.5 test).
4. **Disk tier:** procedural `derive_cached` key (SceneDB change 4.3.1),
   prune hookup, warm-start of a second run is read-bound not CPU-bound.
5. **Coarse tiles as derived blobs** (5.2); far view survives eviction.
6. **Authoritative page store** + journal tail + background flush, behind a
   size threshold so small levels are unchanged.
7. **StreamingGrid integration** for prefetch shape (per-face cells) once
   its warm eviction (4.3.3) lands; until then a planner-driven prefetch in
   Helio is enough.
8. Memory Explorer rows; document in `.agents/MEMORY.md`,
   `docs/voxel-system.md`.

Phases 2-3 alone remove the unbounded growth for decoration; 6 is only needed
for worlds whose *player* edits are large.

## 8. Open questions (need a decision)

1. **One StreamingGrid per face vs. a cube-sphere-aware grid in SceneDB?**
   Per-face reuse is cheapest; edge handling is the cost.
2. **Is decoration allowed to become analytic (GPU) instead?** Option C: make
   trees/structures a GPU generator layer (like terrain's `rules`), zero
   memory, infinite radius, bit-exact CPU mirror required (the README's "one
   world, two consumers"). Biggest payoff, biggest cost (tree shapes in WGSL +
   Rust). Worth a spike only if the region blob approach shows too much
   rebuild cost.
3. Where does the generated-layer *generator* live? It is game code
   (`voxel_craft/world/decorate.rs`); the trait lets the game supply it, which
   fits `.agents/` rule that engine code has no game knowledge.
4. Level-0 vs coarse residency ratio and the coarse tile size (5.2); needs a
   measurement of coarse bytes/region (not separated yet).
5. Authoritative page size (64x64 columns proposed) vs journal-only for the
   first cut.
6. Does Pulsar's editor want the generated-layer path for authored
   (non-procedural) content (e.g. imported voxel models)? Pulsar's old voxel
   objects (`VoxelComponent`, their own chunk store) were removed: voxel
   content is terrain worlds only.

## 9. Acceptance criteria

- Walk 20 km along a face with `voxel_cpu_cache_mb=768`: RSS flat after
  warm-up (+/- 5%), no tile regenerates after first draw, p99 frame time
  unchanged within 10%.
- Kill the process mid-edit: restart shows every acknowledged edit.
- Disable the disk cache: identical visuals, higher CPU during travel.
- Change `generator_version`: derived cache invalidated, player edits intact.
- Memory Explorer shows the three voxel tiers with hit/rebuild counts.
