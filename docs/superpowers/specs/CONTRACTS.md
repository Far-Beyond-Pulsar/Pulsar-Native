# SceneDB 2.0 — Frozen Cross-Layer Contracts

**Source of truth:** SceneDB2.0.md Rev 2.3. Changes require editing the spec
first, then this file, then code. Code-first contract drift is a review reject.

## C0. Ownership Law (foundational — binds every other contract)

**SceneDB owns all scene data, CPU and GPU.** SceneDB allocates and owns the
persistent device buffers holding scene object state (instance transforms, mesh
and material registries, vertex/index/geometry, the live-generation buffer,
cluster/meshlet buffers), relates each object's CPU and GPU representation via
its stable slot id (C1), owns the CPU→GPU delta-sync, and owns the queries and
indices serving the whole system including the renderer hot loop.

**Helio owns no scene state.** It owns only renderer-internal derived data
(pipelines, shaders, Hi-Z, framebuffers, draw-command and payload scratch) —
everything except the scene object data. It binds SceneDB-owned buffers and
reads them.

**Dependency direction (enforced):** Helio depends on SceneDB; **SceneDB never
depends on Helio** and stays renderer-agnostic. Crate shape: Layer 1
`pulsar_scenedb` is graphics-free; a Layer 2 GPU crate (`pulsar_scenedb_gpu`)
depends on wgpu + `pulsar_scenedb` and owns the device-side scene buffers; Helio
depends on `pulsar_scenedb_gpu` for those buffers. No edge from any `pulsar_scenedb*`
crate to Helio.

**Device ownership:** the wgpu `Device`/`Queue` is an engine-level context that
outlives any renderer instance. SceneDB's GPU crate allocates scene buffers on
it; Helio is handed the context + SceneDB's buffer/bind-group references.
Dropping Helio must not drop the device or any scene buffer.

**Acceptance criterion (C0 is unsatisfied until this passes):** Test 13 —
Stateless Renderer Teardown. With a scene resident and rendering, drop the
entire Helio instance and rebuild it against the same SceneDB; the scene renders
identically with **zero scene-data reload** (no disk read, no CPU re-marshal, no
buffer re-upload) and every scene SSBO + the device survive the teardown.
Companion: Test 14 — device-loss re-materialization (SceneDB rebuilds the GPU
side from its CPU-authoritative columns).

## C1. Handle

64-bit packed: bits 0–31 stable slot index, bits 32–63 generation.
Generation 0 = INVALID_HANDLE (the all-zero handle is invalid). First live
generation is 1. A slot whose generation reaches u32::MAX is permanently
retired. Slot IDs are stable for the allocation lifetime; row positions are
frame-scoped (slot→row indirection table, one u32 per slot, updated only
during frame-boundary compaction).

## C2. Page layout

One contiguous 64-byte-aligned allocation per page. Header: length u32,
capacity u32, column byte offsets u32 × N. Every column starts on a 64-byte
boundary. Capacity per cell type: default 256, hard ceiling 1024. Combined
registered stride per element ≤ 128 bytes (compile-time assertion, holistic
per cell composition). Liveness bitmask: u64 array, 1 bit per element, atomic.

## C3. Frame phases

Strict order per frame: Simulate (sub-phase A gameplay writes, sub-phase B
physics writeback) → Harvest (read-only, leases) → Cull (GPU compute) →
Draw → Retire/Compact (frame boundary: retirement queue drain, generation
increments, swap-and-pop, slot→row updates, lease/scratchpad maintenance,
domain transitions). No structural page changes outside Retire/Compact.

## C4. Query & harvest

Query input: TypeToken + AABB or frustum (6 planes). Output: caller-provided
scratch buffers; unified token arrays positionally aligned across columns;
null sentinel 0xFFFF_FFFF. Output row indices valid for the issuing frame
only. Lease: per-cell atomic u64 bitmask, lease slots (pool of 64, not
thread-bound), 2.0 ms revocation timeout at frame boundary. Scratchpads:
thread-local, persistent, halved when peak usage < 50% capacity over 8 frames.
DEI = valid/total; DEI < 25% → host-side dense compaction before upload.

## C5. GPU buffer layouts (WGSL, scalar fields only — no vec3)

Mesh metadata: 72 bytes — vertex_offset u32@0, index_offset u32@4,
index_count u32@8, base_vertex i32@12, material_index u32@16, lod_count
u32@20, lod_distances f32×4@24, local_aabb_center f32×3@40,
cluster_table_offset u32@52, local_aabb_extents f32×3@56, meshlet_count
u32@68. Exactly one of {lod_count, cluster_table_offset} is non-zero.

ClusterNode: 48 bytes — meshlet_offset u32@0, meshlet_count u32@4,
parent_error f32@8, self_error f32@12 (invariant self_error < parent_error),
group_id u32@16, child_offset u32@20, child_count u32@24, padding u32@28 (=0),
bounding_sphere f32×4@32 (xyz center, w radius).

Instance: 64 bytes — row-major mat4 transform. Material: 32 bytes (PBR
params, defined in M3 plan). Generation buffer: u32 per slot. Draw command:
index_count u32, instance_count u32 (always 1 or 0), first_index u32,
vertex_offset i32, first_instance u32 (= command slot, bindless lookup key).
Per-view command buffers; bounded atomicAdd allocation; CPU-side count clamp.

Enforcement: Test 3 — host struct offsets vs naga reflection of compiled
WGSL, byte-exact, in CI on every PR touching shared structs.

## C6. Retirement

Deletion enqueues (slot, generation, submission_serial). A slot is recycled
only after Queue::on_submitted_work_done has confirmed its serial. New
generation is written to the VRAM generation buffer before the slot returns
to the free pool. GPU validates handles against the VRAM generation buffer
exclusively.

## C7. Type registration

TypeToken: dense u32 per registered column type, assigned at registration.
Registration macros declare: column element type (Pod), per-cell-type
membership, and stride contribution. Bridged to pulsar_reflection so
EngineClass metadata, serialization, and SceneDB columns share one
registration point. Stride guardrails per C2.
