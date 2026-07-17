# SceneDB 2.0 — Milestone 3 Design: The Helio Inversion

**Date:** 2026-07-16 (rev 2 — post adversarial review)
**Status:** Approved (design); implementation plans to follow (three, per §1.1)
**Governs:** spec §0/C0 (Test 13 — THE Ownership Law gate), Part IV §10–14, Part IVb §15–19, C5 (remaining layouts), C6/§20 + Test 2 (GPU validation), C4
**Spec of record:** `docs/superpowers/specs/SceneDB2.0.md` (Rev 2.3) + the Rev 2.4 punch list (holistic audit; extended by this design — R8–R11 in §11)
**Predecessors:** M2a design (Rev 3), M2b design (Rev 2 §11)
**Recon of record:** `.superpowers/sdd/m3-recon-helio.md`, `m3-recon-engine.md`

> **Lineage decision (USER, 2026-07-16): upstream wgpu 30 is the direction
> for Helio main.** M3 designs against the **v4 lineage** (active line,
> crates.io wgpu 30.0.0). SceneDB's `gpu` feature migrates off the fork pin
> to wgpu 30 as M3-α task 1. The rest of the Pulsar workspace (wgpui,
> editor) stays on the fork until the **M4 engine-wide migration gate**.

> **Rev 2 note.** The adversarial review found four design-breaking holes in
> rev 1, all corrected here: (1) §6's vendor-and-`[patch]` plan would have
> hard-broken the entire editor in M3-α (a `[patch]` is workspace-global —
> every legacy Helio consumer would compile against v4/wgpu-30 while wgpui
> hands them fork-wgpu devices) and `helio-snapshot` (consumed by
> `engine_fs` thumbnails) does not exist on v4 — the patch flip and
> consumer cutover move to M4; M3 vendors WITHOUT patching (§6). The
> nested-workspace mechanics themselves were verified viable (the "naga
> lesson" does not generalize; two in-tree precedents path-dep into nested
> workspaces). (2) rev 1's `visible_instance_ids[slot]` conflated three
> index spaces — §14.1/14.2's command-slot-keyed, **row-valued** shape is
> restored verbatim (§3). (3) Test 2/§3.3/C6's GPU generation check was
> silently unimplementable — resolved with an **aligned expected-generation
> column** in harvest output (§3.1). (4) texture ownership was inverted —
> a SceneDB-owned texture store now holds the `wgpu::Texture` objects (§2).
> Also: the missing per-instance **instance-info column** (without which
> the cull shader cannot find a token's mesh) joins M3-α; voxel/water/
> post-volume/ComponentRegistry scene state is explicitly classified (§2.1);
> HLOD proxy draws are re-sourced per-cell from the grid, never from
> harvest tokens (§4.1); the 32 B material row goes to contract
> renegotiation BEFORE code (§11 R8).

---

## 1. Goal & position in the roadmap

Invert the renderer: Helio stops owning scene state and becomes a
**stateless consumer** of SceneDB's persistent GPU buffers, gated by
**Test 13 — Stateless Renderer Teardown**.

Recon ground truth: Helio's pass layer is already stateless in shape (all
passes consume borrowed `SceneResources<'_>`). The inversion is (a)
replacing `GpuScene`'s owned scene buffers with bindings to SceneDB's, (b)
un-fusing `Scene::flush()`, (c) building the cull front-end over M2b's
harvest output.

### 1.1 Milestone split (binding)

| Milestone | Scope | Depends on |
|---|---|---|
| **M3-α — alignment & seam** | wgpu-30 migration of `pulsar_scenedb::gpu` (+naga, Test 3 pins, fork-API drift); cut/feature-gate the `pulsar_reflection → gpui-ce` edge (removes fork-wgpu from scenedb's tree; audit follow-up); Helio vendored WITHOUT `[patch]` (§6); `SceneDbBinding` seam; **new SceneDB deliverables:** instance-info column (mesh_index+flags, 8 B/row, mirrored+writer+Test 3), texture store (owns `wgpu::Texture`s + slot table), meshlet buffer (§19, 32 B), material buffer per the **renegotiated C5 row** (R8 — contract before code), expected-generation harvest column (§3.1), asset-store write counters (Test 13 instrumentation), `VERTEX` usage on GeometryArena's vertex buffer; Test 3 rows both sides (Helio-side reflection harness is NEW infrastructure) | M2b |
| **M3-β — the consuming passes** | Cull compute (frustum → near-plane → Hi-Z, §11–13; generation check per §3.1); indirect generation (§14 verbatim); traditional pipeline binding; Cull/Draw phase witnesses; **Test 13**; **Test 2** (stale-handle injection into the cull stream); **Test 4** (transform sweeps) + **Test 5** (overflow clamping) headless | M3-α |
| **M3-γ — VG + HLOD** | VG cull/cluster/meshlet re-pointed at SceneDB buffers (compute adaptation); **per-cell** HLOD proxy draws + α-stipple (§4.1) with a generated proxy-stub asset; Tests 7/8 | M3-β |

γ does NOT fold into β (rev 1's hedge withdrawn — HLOD needs new pass
infrastructure Helio lacks entirely, plus the proxy-sourcing design of §4.1).

## 2. The ownership extraction

**Already built in SceneDB (M2) — Helio binds:** instance transforms,
mesh metadata (MeshRegistry), geometry (GeometryArena), cluster DAG
(ClusterBuffer), generation buffer + slot mirror. Helio's separate
per-object AABB buffer is **retired** (world AABB computed in-shader per
§11 from the instance matrix + local AABB in mesh metadata).

**Moves to SceneDB in M3-α (new):**
- **Instance-info column** (adversarial finding 5): a second mirrored
  per-row column `InstanceInfo { mesh_index: u32, flags: u32 }` (8 B, C5
  row, Test 3), with a `write_instance_info` writer beside
  `write_transform` (same dirty/sync machinery). Without it the cull
  shader cannot resolve token → mesh. Flags reserves the §12 near-clip
  bit's CPU-visible twin and future per-instance bits.
- **Texture store** (finding 4): owns the `wgpu::Texture` objects and the
  bindless slot table (16384-slot ceiling per recon). Helio rebuilds only
  views + the bind array + bind groups at construction — actual derived
  state. Spec §10 G4 satisfied as written.
- **Meshlet buffer** (§19): 32 B/meshlet beside ClusterBuffer.
- **Material buffer**: per the renegotiated C5 row (R8) — layout decided
  at contract level FIRST; the recon shows 32 B cannot hold PBR params +
  bindless texture indices + a Radiant-graph reference (Helio's current
  material carries a u64 graph hash). Proposed: 64 B row; the Rev 2.4
  amendment (R8) must land before the α writer is coded.
- **LOD representation decision:** C5's single index range + 4 LOD
  distances cannot express per-LOD geometry ranges (§14.1 "index_count for
  selected LOD"). Decision: **LODs are consecutive MeshRegistry entries**;
  `mesh_index` addresses LOD 0 and `lod_count` spans the run;
  `lod_distances` stays on the LOD-0 entry. Recorded as R9.

**Stays Helio (derived, C0-clean):** Hi-Z pyramids, per-view indirect
command/counter/visibility/task-payload scratch (§14.3), framebuffers,
pipelines/shaders, light-cull structures, shadow matrices/atlases,
texture VIEWS + bind arrays, post stacks, debug overlays.

### 2.1 Explicitly classified scene-adjacent state (finding 6)

Applying the lights pattern — each is scene data long-term, each is
DEFERRED with a record rather than silently mislabeled "derived":

| State (recon cite) | M3 disposition | Owner decision |
|---|---|---|
| Lights + shadow-caster scene state | Engine re-pushes after teardown (allowed: not scene-*object* data under C0's current wording) | M4 + Rev 2.4 (R10) |
| Voxel pools/volumes/edit ring (`gpu_scene.rs:263-276`; VoxelMeshPass is a default geometry pass) | Same carve-out; **Test 13 in M3 scopes to non-voxel scenes**, stated in the gate | M4 + Rev 2.4 (R10) |
| Water/postprocess volumes | Same carve-out | M4 |
| `GpuScene.components: ComponentRegistry` (type-erased ECS inside the renderer) | Untouched in M3; explicitly on M4's dismantle list (it is the push-model's shadow ECS) | M4 |

Test 13's M3 wording: "zero **scene-object** data reload (instances,
meshes, geometry, materials, textures, clusters/meshlets); lights, voxel
volumes, and water/post volumes are engine-repushed derived-path state
pending the R10 ownership amendment."

**Un-fusing `Scene::flush()`:** mechanical scene-buffer uploads → deleted
(delta-sync owns them); shadow-caster importance budgeting → stays (Helio
pass-prep over bound buffers); PSO/material-range bookkeeping → splits
(data SceneDB, pipeline grouping Helio); VG topology rebuild → moves to
asset registration.

## 3. The binding seam (M3-α core)

New crate `helio-scenedb` inside the vendored Helio workspace — the ONLY
place Helio names `pulsar_scenedb` (path dep, `features = ["gpu"]`).

- `SceneDbBinding`: buffer refs (instance, instance-info, slot mirror,
  generation, mesh configurator, material, cluster, meshlet, cell
  metadata, geometry V/I, texture bind array sources) + the C5 bind group
  layouts. Rebuilt at renderer construction — Test 13's mechanism. Passes
  reach it through the existing `SceneResources<'_>` seam.
- **Per-frame inputs:** per-view dense token buffers uploaded from
  `HarvestStaging` (Helio-owned frame scratch, §14.3). The M2b
  `region_base` freshness contract binds the driver. The remap table is
  NOT consumed on the GPU path (CPU-side positional re-join only).
- **Indirect data shapes are spec-verbatim (finding 2):**
  `visible_instance_ids[command_slot] = global_row` (§14.2), commands per
  §14.1 with `first_instance = command_slot`. Downstream passes fetch
  instance data row-indexed via `visible_instance_ids[first_instance]`.
  No slot-keyed cross-frame array exists; per-view buffers are
  frame-scoped derived scratch.

### 3.1 GPU generation validation (findings 3 + 2; Test 2, §3.3, C6, §20.2)

Harvest gains an **aligned expected-generation column**: for every dense
token emitted, the CPU also emits `expected_gen[i] =
registry.generations()[slot_column[local_row]]` (two array reads per valid
token; C4's "unified token arrays positionally aligned across columns"
covers it; DEI compaction compacts both arrays with the same remap; the
frozen token layout is untouched). The cull shader validates
`generations[slot_mirror[row]] == expected_gen[i]`; mismatch ⇒ token
dropped, telemetry counter incremented (fails closed).

This (a) preserves Test 2, §3.3-GPU, C6, and §20.2 **verbatim** —
stale-handle injection into the cull input stream is detectable exactly as
specified; (b) is genuine defense-in-depth: a violated `region_base`
freshness contract (M2b's documented silent hazard) now surfaces as a
generation mismatch instead of silently wrong draws; (c) costs one u32 per
valid token per view. The frame-scoping argument (tokens die at the
boundary; pending-retire rows never harvest; regions serial-pin) remains
the reason organic staleness cannot occur — the shader check is the
contract-mandated backstop, not the primary mechanism. §8's "generation
mismatch ⇒ command dropped" is now consistent with §3 (rev 1's internal
contradiction resolved).

## 4. The consuming passes (M3-β / γ)

Traditional (β): cull compute per view — token+gen fetch → §3.1 validation
→ instance matrix + InstanceInfo.mesh_index → local AABB from mesh
metadata → §11 |M₃ₓ₃| world AABB → view-space near-plane pre-test → §12
W≤0 bypass (near-clip flag into per-instance indirect data) → frustum →
§13 Hi-Z (previous-frame pyramid; mip selection, 5% boundary blending,
kernel expansion) → §14.2 bounded-atomic slot alloc (CPU clamp after
counter readback, or conservative max-count with `instance_count = 0` —
β plan measures wgpu-30 readback latency and picks) → §14.1 command +
`visible_instance_ids[command_slot] = row`. Then indirect draw over
GeometryArena (vertex buffer gains `VERTEX` usage in α; classic vertex
fetch is the default binding model, vertex pulling recorded as an option),
Hi-Z rebuild (existing `HiZBuild` re-pointed per §18 traditional-first),
existing downstream passes unchanged behind `SceneResources`.

VG (γ): object cull identical over vg tokens → compute cluster-DAG
traversal (§16.3 error metric with the radius-subtracted distance, §17
backface cone + per-meshlet frustum) → per-meshlet indirect records
(task/mesh → compute adaptation, locked) → meshlet raster from SceneDB's
meshlet + geometry buffers.

### 4.1 HLOD proxy draws are per-CELL, not per-token (finding 7)

Outer cells have no region, no `gpu_id`, and are never harvested — proxy
draws CANNOT come from harvest tokens. Instead, the driver builds a
per-frame **proxy draw list from the grid's materialized-cell set**:
for every materialized cell with domain ∈ {Outer, Margin-fading}, emit
(proxy mesh_index from the cell's registry entry, cell placement
transform, dense_id). The stipple shader reads α+domain from the per-cell
metadata buffer at `dense_id * 8` (M2b's shipped contract). Margin-cell
*entity* draws get their fade α via a row→dense_id lookup carried in the
per-view token upload for margin cells (a small aligned column, γ scope).
Proxy assets: γ ships a **generated stub proxy** (colored bounding-box
mesh per cell, registered through the normal MeshRegistry path) so Tests
7/8 gate the *mechanism*; authored/baked proxies are M4+ asset-pipeline
work (spec Appendix B).

## 5. Verification

- **Test 13 (headless, exact assertion set — finding 8):** window = drop
  Helio A → construct Helio B → N frames. Assert: Σ`SyncStats.bytes` == 0
  across the window (transform + slot-mirror + instance-info syncs);
  Δ`generation_write_count` == 0; **new α asset-store write counters**
  (MeshRegistry/ClusterBuffer/meshlet/material/texture/GeometryArena
  upload counts) all zero; streaming transitions frozen for the window
  (or `write_cell_metadata` excluded and counted separately — it is an
  unconditional full rewrite per call by design); G-buffer readback hash
  byte-identical with **jitter pinned / non-TAA graph** (Helio B resets
  frame_count → Halton phase differs otherwise); Hi-Z warm-vs-cold is
  image-invariant only because occlusion is conservative — asserted via
  the final converged frame, stated in the gate. Device + every scene
  SSBO alive throughout (buffer IDs unchanged).
- **Test 2 (β):** inject a stale token+gen pair into the cull input →
  shader drops it, telemetry increments, no command written.
- **Test 14 cross-repo**, **Test 4** (transform sweeps through the GPU
  path vs CPU reference), **Test 5** (§14.2 overflow: over-subscribe the
  command buffer, assert CPU clamp + silent drops, no corruption).
- **Tests 7/8 (γ)** with the stub proxy.
- **GPU-vs-CPU cull equality:** the β driver test culls the same token set
  on CPU (reference impl) and GPU; visible sets must match exactly (M1b
  bit-identity discipline applied at pass level).
- Test 3 (α): instance-info, material (post-R8), meshlet, draw-command,
  task-payload rows — naga in SceneDB CI + the NEW Helio-side reflection
  harness against the same WGSL.

## 6. Repo mechanics (reshaped per finding 1)

- **M3: vendor WITHOUT `[patch]` and WITHOUT touching root git pins.**
  Submodule at `crates/renderer/helio` tracking branch `scenedb20-m3`
  (cut from the v4 lineage). It is a **standalone nested workspace** —
  NOT a Pulsar workspace member, NOT patched in. `helio-scenedb` inside
  it path-deps `../../core/pulsar_scenedb` with `features = ["gpu"]`
  (mechanics verified: cargo resolves path deps into nested
  workspace-inheriting members; two in-tree precedents). All M3 building
  and testing runs from the submodule's workspace root. The editor and
  every legacy Helio consumer keep compiling against the pinned
  `b88e366d` git dep, untouched.
- **M4 gate (moved from M3):** the `[patch."…/Helio"]` flip, legacy
  consumer cutover, dual-Helio-rev retirement (root `b88e366d` vs wgpui's
  `f124aeac`), the engine-wide wgpu-30 migration, and the
  **helio-snapshot disposition** — it exists only on the old lineage
  (`crates_other/helio-snapshot`, absent on v4) and `engine_fs`
  thumbnails consume it: port it to v4, vendor it separately, or replace
  the thumbnail path. Named M4-plan input.
- **wgpu-30 migration (α task 1):** `pulsar_scenedb`'s gpu feature gets
  its own `wgpu = "30"` (crates.io — distinct source from the fork
  `[patch]`; coexistence empirically verified); naga dev-dep → crates.io
  matching release; fork-28→30 API drift migrated with the 6-suite matrix
  as oracle. Prerequisite in the same task: cut or feature-gate
  `pulsar_reflection`'s unconditional `gpui-ce`/`ui` deps (finding 9 —
  they drag fork-wgpu + the whole UI stack into scenedb's tree and would
  bloat every Helio-side build; also the audit's recorded crate-split
  follow-up). CI addition: resolve `pulsar_scenedb` from OUTSIDE the
  workspace (as the submodule build will) to assert no reliance on root
  `[patch]` tables.
- Helio repo `scenedb20` branch rebases onto the v4 lineage; CONTRACTS
  mirror refreshed with all audit amendments.

## 7. M2b §11 carry-forwards addressed

Meshlet buffer → α. Remap consumer → decided (CPU-side; §3). Cull/Draw
witnesses → β (`HarvestPhase::end() -> CullPhase -> DrawPhase ->
BoundaryPhase`; token upload gated on Cull, submit on Draw). Mirror
stale-tail → structurally unhittable on the GPU path (dispatch bounded by
issuing-frame token count) + driver assert. `region_base` freshness →
driver contract + §3.1's generation backstop. Material layout → α after
R8.

## 8. Error handling

Token-buffer overflow at upload = hard error (frame scratch sized to
configured max). Cull generation mismatch = token dropped + telemetry
(fails closed). §14.2 command overflow = silent drop + CPU clamp (Test 5).
Device loss = Test 14 path. Layout mismatch = wgpu bind-group validation +
Test 3. Missing texture slot = bind-array default texture (fails visible,
not UB).

## 9. Open items for the α plan (not blocking design approval)

(a) §14.2 clamp strategy: counter readback vs conservative max-count —
measure on wgpu 30. (b) Radiant-graph ↔ material-row interplay beyond the
R8 row shape (graph index field reserved). (c) The Helio-side reflection
harness's exact mechanism (naga on WGSL source vs compiled-module
introspection).

## 10. Deferred to M4

Everything in §6's M4 gate; `sync_scene` dismantling (batch DFS push AND
the gizmo/selection dual-writes); `EngineGpuContext` single device root
(two owners today: editor wgpui surface + pulsar_game GpuContext);
lights/voxel/water/post ownership (R10) + `ComponentRegistry` dismantle;
`scenedb2` feature flag + legacy SceneDb replacement; authored HLOD proxy
pipeline; editor visual verification.

## 11. Rev 2.4 punch-list additions (contract changes this design requires)

- **R8:** C5 material row: 32 B → 64 B (PBR params + bindless texture
  indices + Radiant-graph reference do not fit in 32 B). MUST land before
  the α material writer is coded (spec → CONTRACTS → code order).
- **R9:** LOD representation: LODs as consecutive MeshRegistry entries
  (mesh_index = LOD 0, lod_count spans the run) — §6.1/§14.1 wording.
- **R10:** ownership enumeration extended: lights, voxel volumes,
  water/post volumes as SceneDB scene data (M4 execution).
- **R11:** §14.2's `visible_instance_ids` wording confirmed as
  command-slot-keyed/row-valued (no change — recorded here because rev 1
  nearly drifted it; plus the instance-info column joins §10's buffer
  inventory).
- **R12:** MeshletEntry layout (32 B, C5 amendment, M3-α T6): spec §19 fixes
  size + contents only ("32 B/meshlet beside ClusterBuffer") — the field
  order/offsets are defined here: sphere_x/y/z/radius f32@0/4/8/12,
  cone_packed u32@16 (i8x3 snorm axis | i8 snorm sin-cutoff, §17.2 backface
  test), data_offset u32@20 (geometry index buffer element offset),
  counts_packed u32@24 (vertex_count u8 | triangle_count u8 << 8 | reserved
  u16 << 16), reserved u32@28. See CONTRACTS.md C5 for the canonical row.
