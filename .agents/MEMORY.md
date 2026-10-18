# Memory: how it is held, bounded and inspected

Why this exists: a project with a few hundred 4K textures drove the editor to
12 GB, then 21 GB committed. The cause was not a leak: every texture a material
referenced was decoded to full-size RGBA8 on an asset worker, kept as the asset
payload for as long as a component referenced it, and uploaded to the GPU with
no mips and no size cap. Memory grew with the number of assets, not with what
was in use.

## The tiers an asset now has

| Tier | Where | Owner | Budget |
|---|---|---|---|
| Cold | the source file (a JPEG) | the project | none |
| Disk | the derived file (`.pulsar/cache/derived/<kind>/<key>.pdd`) | `pulsar_scenedb::derive_cached` | `streaming.derived_cache_mb` |
| RAM | the cooked bytes in the residency cache | `pulsar_scenedb::Residency<T>` | `streaming.cpu_cache_mb` |
| VRAM | a `wgpu::Texture` in a scene's `TextureStore`, shared per device | `TextureStore::register_mips` / `register_shared` | `streaming.texture_stream_pool_mb` (tier engine) |

A value dropped from RAM is rebuilt by reading the Disk tier, which costs a file
read. Rebuilding from Cold costs a decode, a resize, a mip chain and an encode.

## The rules

1. **Rebuildable data is a cache, not owned data.** Anything derived from a
   source file lives in `Residency<T>`: a process-wide, byte-budgeted LRU.
   `get()` returns the value or rebuilds it; the cache drops only *its*
   reference, so a value in use is never pulled out from under a caller. One
   value larger than the whole budget is kept while it is the most recent (no
   thrash). Code: `SceneDB/src/residency.rs`.
   - Recency is a counter indexed by an ordered map: victim choice, touch and
     insert are `O(log n)` (tested with 100,000 residents against a reference LRU).
   - `get()` is **single flight**: concurrent callers share one rebuild.
   - `prefetch()` starts a rebuild on the asset loader threads and returns a
     handle. A `get()` or `Prefetch::wait()` that reaches a still-queued
     prefetch claims it and runs it itself, so a loader thread can never wait
     on the queue it is blocking.
2. **Derived data is cached on disk.** `derive_cached(request, build)` returns
   the file for `(kind, source path, source length+mtime, settings, version)`, or
   runs `build` and stores it. Writes are temp-file + rename; a header (magic,
   version, length, key) is checked on read; a bad file is a miss.
   `prune_derived_cache(max)` deletes least recently used files (hits refresh
   mtime). Code: `SceneDB/src/derived.rs`. The engine points it at
   `<project>/.pulsar/cache/derived` and prunes on a background thread.
3. **Textures are cooked.** `helio-component/src/cooked_texture.rs`: size cap
   (`streaming.max_texture_size`, default 2048, `0` = off) applied before the
   RGBA conversion, a full mip chain averaged in linear light, then BC7
   (Intel ISPC compressor) when the device has `TEXTURE_COMPRESSION_BC` and the
   size is a multiple of 4, otherwise RGBA8. One blob per format (`PTX1`
   container), validated on read. A 2048² image: 16 MiB RGBA8 one level,
   21.3 MiB RGBA8 with mips, 5.3 MiB BC7 with mips.
4. **The upload understands mips and blocks.** `TextureStore::register_mips`
   checks every level's byte length against the format's block layout *before*
   taking a slot, then writes each level (small levels round up to a block).
5. **A texture is a device resource.** `register_shared` lets a second scene's
   store adopt a texture another scene on the same device uploaded. Helio keeps
   one upload per (device, texture, version) (`SharedTexture` in
   `texture_assets.rs`), pruned lazily when no store holds it.
6. **Lock discipline.** The residency cache lock is never taken while holding a
   value's slot lock, values are dropped after every lock is released, a rebuild
   never runs under a lock, and `resident_slot` fetches cooked data before it
   takes the upload locks.
7. **State goes through the runtime.** The residency and derived caches live in
   `AssetRegistries`, so every statically linked copy of SceneDB shares one (see
   `PLUGIN_SYSTEM.md`, "Plugin components and the World"). Adding a `static`
   there fails `tests/world_state.rs`; adding a field bumps the ABI version (now 5).

## Settings (project, namespace `streaming`)

| Key | Default | What |
|---|---|---|
| `max_texture_size` | 2048 | Largest decoded side in texels; `0` disables the cap |
| `cpu_cache_mb` | 1024 | Residency cache budget for cooked data in RAM |
| `derived_cache_mb` | 4096 | Disk budget for derived files; pruned LRU at startup |
| `texture_stream_pool_mb` | 512 | SceneDB GPU tier budget (VRAM) |

## Finding where memory is (Mission Control, tab "Memory Explorer")

Three sources, because no single tool covers a process whose "RAM" is half VRAM:

- **Heap**: live allocations at or above a threshold (4/16/64/256 MiB), grouped
  by call stack with up to 48 frames, attributed to the first non-plumbing frame.
  *Off by default* (it captures a backtrace per large allocation): turn on
  "Heap tracking", then load the assets. Allocations made before it was enabled
  are not listed. Code: `ui_log_viewer/src/utils/large_allocs.rs`. The same view
  shows the residency cache (resident / budget / evicted / rebuilt / prefetched /
  joined) and the derived-data cache (hits, cooks, bytes).
- **GPU**: wgpu's allocator report for every engine device (`device_loss::watched_devices`),
  by allocation name and by memory block fill. VRAM is invisible to the heap tracker.
- **Process**: `VirtualQuery` over the address space. `PAGE_WRITECOMBINE` regions
  are GPU-visible memory (VRAM windows, upload heaps), never ordinary heap. A
  256 MB write-combined block is a `gpu-allocator` device block; 64 MB is a host block.

"Export report" writes all of it to
`<project>/.pulsar/profiling/memory/memory_report_<unix>.md`.

Timings for your own images: `real_texture_cook.rs` (ignored test, see its header).

## Not done yet

- **Streaming by detail level.** Mip chains now exist and the RAM and Disk tiers
  can serve them, but a texture is still uploaded whole (all levels). Dropping
  the top levels under a VRAM budget, with a global bias, is the next step.
- **Every cooked texture is sRGB BC7.** Normal and roughness maps would want
  linear formats (BC5 for normals); the pipeline treated them all as sRGB before
  and still does.
- **The editor's shared material sampler is wgpu's default** (nearest filtering,
  clamp). Mips are sampled, but trilinear filtering is an engine decision.
- **Mesh vertex pools are per scene.** The GPU report shows
  `scenedb-var-len-pool-"builtin_mesh_vertex"` once per scene; only textures are
  shared across scenes.
- **A miss decodes on the caller's thread** unless something prefetched it.
  `compiled_graph` prefetches a material's textures; other callers do not yet.
