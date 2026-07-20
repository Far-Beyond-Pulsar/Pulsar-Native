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
depends on Helio** and stays renderer-agnostic. Crate shape: **one crate,
`pulsar_scenedb`, owns both sides.** Its core (Layer 1) is graphics-free; the
device-side store is a feature-gated GPU layer — module `pulsar_scenedb::gpu`
behind the `gpu` cargo feature (optional wgpu dep, off by default). **There is
no separate GPU crate.** Helio depends on `pulsar_scenedb` with
`features = ["gpu"]` for those buffers. No edge from `pulsar_scenedb` to Helio.

**Graphics-free enforcement:** the boundary is the feature, not a crate. CI must
keep `cargo check -p pulsar_scenedb --no-default-features` green (the core
compiles with zero graphics dependency) alongside the no-`pulsar_scenedb`→Helio
edge guard.

**Device ownership:** the wgpu `Device`/`Queue` is an engine-level context that
outlives any renderer instance. SceneDB's GPU layer (`pulsar_scenedb::gpu`)
allocates scene buffers on it; Helio is handed the context + SceneDB's
buffer/bind-group references.
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
registered stride per element ≤ 128 bytes — shipped as a **hard runtime
`Result`** check at cell-type build / page-layout time (holistic per cell
composition); "compile-time assertion" remains a Rev 2.4 aspiration note, not
current behavior. Liveness bitmask: u64 array, 1 bit per element, atomic.

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

**Amendment (M3-β T2, §9.2.1 / contract #32 — PRIMITIVES DELIVERED, wiring
pending; cite Rev 2.4 routing per the perf report R-PERF-3):** the 2.0 ms
is a hold-duration TIMEOUT (the trigger condition for revocation), not a
latency budget on any operation. On expiry
`gpu::HarvestPipeline::revoke_overdue` force-releases the lease's
`LeaseMask` slot immediately (`any_held()` clears without waiting for the
holder's own drop), and the gated compaction seams
(`gpu::RetiredPhase::compact_gated` / `SceneGpuStore::compact_all_gated` /
`HarvestPipeline::compaction_ready`) let a driver proceed against the
primary layout while the straggler keeps reading its pinned
`LivenessSnapshot`. **Scope honesty:** nothing in production binds
`LeaseMask` to cells or calls the gated seams yet — the default boundary
path (`BoundaryPhase::run` → `compact_all`) remains ungated; wiring is M4
World-driver scope, so #32's stall bound is deliverable but not yet
delivered by default. `revalidate_run` is liveness-only, not
generation-aware — see the committed hazard repro test; generation-aware
revalidation is an M3-γ/M4 prerequisite for any production reliance
post-revocation.

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

**Amendment (audit-remediation, see
`docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md`):**
`cluster_table_offset` (mesh metadata, above) is a **node index** into the
global cluster DAG buffer — byte offset = index × 48 — not a byte offset;
spec §6.1's "byte offset into global cluster DAG buffer" wording is
superseded pending Rev 2.4 (§16.2's "indexed by" phrasing is the accurate
one). **Node 0 is a reserved all-zero sentinel** (never a real table): under
the XOR rule above, `cluster_table_offset == 0` means "no table", so real
tables start at node index ≥ 1 and `max_nodes` budgets include the sentinel.

Instance: 64 bytes — mat4 transform. Generation buffer: u32 per
slot. Draw command: index_count u32, instance_count u32 (always 1 or 0),
first_index u32, vertex_offset i32, first_instance u32 (= command slot,
bindless lookup key). Per-view command buffers; bounded atomicAdd
allocation; CPU-side count clamp.

**Device requirement, parallel to #47 (M3-β T10, promoted out of a test doc
comment):** any device that issues indirect draws against this row's
`first_instance = command slot` contract MUST request
`wgpu::Features::INDIRECT_FIRST_INSTANCE` at `request_device`. Per wgpu-types
30's own documentation on `DrawIndexedIndirectArgs::first_instance`, it "has
to be 0, unless `Features::INDIRECT_FIRST_INSTANCE` is enabled." Without the
feature, this stack's observed behavior on wgpu 30 (both the Vulkan and the
platform-default backend, empirically confirmed M3-β T7) is to execute the
indirect draw **as if `first_instance` were zero — silently, with no
validation error** — which breaks this row's bindless lookup key outright:
every indirect draw's `@builtin(instance_index)` reads record 0 regardless of
which slot the draw call's args actually named, with no diagnostic pointing
at the cause. Widely supported on desktop GPUs (Vulkan/DX12/Metal all expose
it; the WebGPU spec merely gates it behind an opt-in feature). Enforcement:
Helio's own renderer hard-requires this feature independently — this is the
SAME requirement, not a new one, and the two call sites must be kept in sync
by design: `helio/src/renderer/config.rs:14-18` (`required_wgpu_features`,
pinned by `indirect_first_instance_is_required_even_when_adapter_does_not_
report_it`) and `helio/src/renderer/setup.rs:73-74` (construction-time
`assert!`). `helio-scenedb`'s own GPU test contexts request it independently
for the draw-executor suites (`crates/helio-scenedb/tests/support/mod.rs`,
`test_context_indirect_first_instance`). See design doc §13.1 for the full
callout.

**Amendment (M3-β T5 review — instance transform flattening, empirically
resolved on a real GPU):** the instance element's 16 floats are the
**column-major flattening**, `array[4 * col + row] = M[row][col]` — what a
column-major library's `to_cols_array()` emits, and what WGSL's
`mat4x4<f32>` consumes with no transpose (`m * vec4(local, 1.0)`). This
row previously said "row-major mat4"; that wording is superseded because
its natural literal reading transposes the rotation block and silently
corrupts the §11 |M₃ₓ₃| world-AABB extents (probe: Rz(30°)·Rx(40°) gives
extent y = 1.9417 correct vs 1.7509 transposed — enough to flip a
frustum decision). Translation-only transforms are identical under both
readings, which is why it went unnoticed until the M3-β cull pass became
the first shader to consume a transform. Rev 2.4 routing per R-PERF-3.

MaterialRow (SceneDB-owned, amendment M3-α, Rev 2.4 R8 approved
2026-07-16): 64 bytes — base_color u32@0 (RGBA8-unorm packed base color
factor, linear), metallic f32@4 (∈[0,1]), roughness f32@8 (∈[0,1]),
normal_scale f32@12 (1.0 = authored), emissive_r/g/b f32@16/20/24 (linear),
emissive_intensity f32@28 (nits-scale HDR multiplier), tex_albedo u32@32,
tex_normal u32@36, tex_metallic_roughness u32@40, tex_emissive u32@44 (all
four texture fields: bindless slot, sentinel 0xFFFF_FFFF = unbound),
radiant_graph_index u32@48 (sentinel 0xFFFF_FFFF = default PBR template),
flags u32@52 (bit 0 double-sided, bit 1 alpha blend, bit 2 alpha test
against alpha_cutoff, bit 3 has normal map, bits 4-31 reserved = 0),
alpha_cutoff f32@56 (∈[0,1], meaningful when flags bit 2 set), reserved
u32@60 (must be 0). Supersedes the 32-byte placeholder ("Material: 32 bytes
(PBR params, defined in M3 plan)") this row previously carried — R8's 64-byte
row is now the binding text (`docs/superpowers/specs/CONTRACTS.md`, restated
from `Research/public/drafts/SceneDB2.0-Rev2.4-PROPOSAL.md` § "R8 — The
64-byte material registry row" until Rev 2.4 is applied to the spec of
record in full). Registration validates metallic/roughness/alpha_cutoff ∈
[0,1] (NaN-rejecting `!(x >= 0.0 && x <= 1.0)` form) and reserved == 0 and
flags bits 4-31 == 0. `MaterialRegistry` (`gpu::MaterialRegistry`) mirrors
`MeshRegistry`'s shape (T7 pattern) and owns its buffer standalone — it
retires `SceneGpuStore`'s prior 32-byte material placeholder buffer/field/
`max_materials` config knob (never written to by anything) in the same
commit.

Slot mirror (SceneDB-owned; amendment, audit-remediation): u32 per **row** —
`global_slot = slot_region_base + local_slot`, i.e. `global_slot(global_row)`.
The GPU resolves `generations[slot_mirror[row]]` for handle validation (C6).
Maintained solely by the frame-boundary self-healing scan. [Pending spec §10
amendment in Rev 2.4.]

Instance info (SceneDB-owned, amendment M3-α): 8 B per row — mesh_index
u32@0 (LOD-0 MeshRegistry entry, R9), flags u32@4 (bit 0 near-clip twin, rest
reserved 0). Row-indexed beside instance transforms.

MeshletEntry (SceneDB-owned, amendment M3-α, design Rev 2 §2 + Rev 2.4
punch-list R12): 32 bytes — sphere_x/y/z/radius f32@0/4/8/12 (bounding
sphere), cone_packed u32@16 (i8x3 snorm axis | i8 snorm sin-cutoff φ, §17.2
backface test), data_offset u32@20 (element offset into the geometry index
buffer), counts_packed u32@24 (vertex_count u8 | triangle_count u8 << 8 |
reserved u16 << 16, must be 0), reserved u32@28 (must be 0). Spec §19 fixes
size + contents only ("32 B/meshlet beside ClusterBuffer"); this layout is
the R12 amendment. Meshlet-offset `i` uploaded at byte offset `i * 32` —
`ClusterNode::meshlet_offset`'s index space.

Per-view token buffer (SceneDB-owned, amendment M3-β T1, design §3.1): one
u32 per valid harvested token (global row, sentinel-free — the T6 finding
holds on both the CPU staging column and this device mirror) plus a
positionally-aligned expected-generation column (u32 per token, same
count). One `(tokens, expected_gens)` buffer pair per `MeshClass` per view
(`gpu::ViewTokenBuffers`), uploaded each harvest from
`HarvestStaging`'s per-class token/gens `Vec`s via `ViewTokenBuffers::upload`
— one `write_buffer` per column (two total per non-empty upload; an empty
column issues zero `write_buffer` calls and does not move the upload
counter). Unlike the region-partitioned scene SSBOs (fixed capacity at
registration, hard error on overflow), this pair is per-view frame scratch
that **grows on demand with slack** (Vec-like ~1.5x, never below a
previously reached high-water mark) — the same discipline `HarvestStaging`
already holds its own `Vec`s to, extended one layer onto the device. The
M3-β cull compute pass is the consumer: validates
`expected_gens[i] == generations[slot_mirror[tokens[i]]]` per §3.1,
dropping (+telemetry) any mismatch.

Enforcement: Test 3 runs in CI on every PR via the `gpu_layout` test target
(`cargo test --features gpu --test gpu_layout` — naga reflection only, no GPU
adapter required): host struct offsets vs naga reflection of compiled WGSL,
byte-exact. Device-dependent suites (`gpu_store`, `gpu_assets`) run locally,
sequentially (`--test-threads=1`), not in CI.

## C6. Retirement

Deletion enqueues (slot, generation, submission_serial). A slot is recycled
only after Queue::on_submitted_work_done has confirmed its serial. New
generation is written to the VRAM generation buffer before the slot returns
to the free pool. GPU validates handles against the VRAM generation buffer
exclusively.

Exception (streaming, M2b §4.1; amendment, audit-remediation): when a cell is
demoted to non-resident (eviction), its queued deferred retires are committed
**CPU-side only** — registry generation bump + slot pool — with **no VRAM
generation write**; the cell owns no generation region at that point, and
writing into a freed (possibly reallocated) region would corrupt a neighbor.
VRAM safety is carried by the freed region's serial pin; on re-promotion the
generation region is bulk-rebuilt and its tail scrubbed from the registry.
[Pending spec §20.2 amendment in Rev 2.4.]

## C7. Type registration

TypeToken: dense u32 per registered column type, assigned at registration
via the runtime builder API (`TypeToken::of::<T>()`, `CellType::with`/
`build`) — declaring column element type (Pod), per-cell-type membership,
and stride contribution. (Amendment, audit-remediation: "registration
macros" is a Rev 2.4 wording fix — no macro form ships today; the builder
API above is what's shipped.) Bridged to pulsar_reflection so EngineClass
metadata, serialization, and SceneDB columns share one registration point.
Stride guardrails per C2.
