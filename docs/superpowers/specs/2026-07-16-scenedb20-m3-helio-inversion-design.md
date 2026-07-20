# SceneDB 2.0 — Milestone 3 Design: The Helio Inversion

**Date:** 2026-07-16 (rev 2 — post adversarial review)
**Status:** M3-β implemented (M3-α Tasks 1-12 complete; M3-β T1-T10 complete). Shipped
this milestone: per-view token + expected-generation upload (`ViewTokenBuffers`, C5),
§9.2.1-adjacent bypass primitives (lease/compaction hazard tests), the cull compute pass
(gen validation, bounds-check, frustum, indirect-command emission, design §4), the
indirect draw executor (`record` and `record_multi_indirect`, §14.1), Test 2 (stale-token
drop), Test 4 (GPU-path transform sweep), Test 5 (overflow clamp), Test 13 (stateless
renderer teardown, C0's binding gate — MET, mutation-proven), GPU-vs-CPU cull equality,
and GPU pass-timing instrumentation (§14.2 below). §9(a) is now CLOSED — see §13.
M3-γ (VG cluster traversal, meshlets, HLOD) is next.
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

(a) **CLOSED (M3-β T9/T10, measured — see §13.2).** §14.2 clamp strategy:
counter readback vs conservative max-count — measure on wgpu 30. The
recommendation flipped during review: strategy (b′)
(`multi_draw_indexed_indirect` over a repacked, tightly-packed 20 B args
buffer) beats readback-then-clamp (a), which beats a naive CPU-loop
conservative max-count (b). Full numbers, methodology, and the honestly-flagged
projected-not-measured crossover caveat are in §13.2. (b) Radiant-graph ↔
material-row interplay beyond the R8 row shape (graph index field reserved).
(c) The Helio-side reflection harness's exact mechanism (naga on WGSL source
vs compiled-module introspection).

## 10. Deferred to M4

Everything in §6's M4 gate; `sync_scene` dismantling (batch DFS push AND
the gizmo/selection dual-writes); `EngineGpuContext` single device root
(two owners today: editor wgpui surface + pulsar_game GpuContext);
lights/voxel/water/post ownership (R10) + `ComponentRegistry` dismantle;
`scenedb2` feature flag + legacy SceneDb replacement; authored HLOD proxy
pipeline; editor visual verification.

- **Required migration step — re-point the seam's path dependency (M3-β
  T10 addition).** SceneDB has been extracted to its own repository
  (`Far-Beyond-Pulsar/SceneDB`); Pulsar-Native's in-tree
  `crates/core/pulsar_scenedb` is being superseded and is frozen to source
  edits as of M3-β T10. `helio-scenedb`'s `Cargo.toml` currently depends on
  it via `pulsar_scenedb = { path = "../../../../core/pulsar_scenedb",
  features = ["gpu"] }` (§3/§6's nested-workspace path-dep mechanism). When
  Pulsar-Native switches to consuming the standalone SceneDB repo instead
  of its in-tree copy, this relative path breaks (the target directory
  either moves or disappears from Pulsar-Native's tree entirely) and the
  seam needs re-pointing — either to a git dependency on the new repo or to
  wherever Pulsar-Native's own consumption strategy lands (vendored
  submodule, published crate, etc.). Named as a required M4 input; the
  exact target depends on decisions the extraction itself hasn't finalized
  yet.

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

## 12. Post-α carry-forwards (M3-α final review, 2026-07-16)

- **§16.1 meshlet limits unenforced host-side** (T6 review advisory):
  `MeshletBuffer::append` validates counts nonzero but not the spec's
  ≤64-vertex/≤124-triangle ceilings (counts are packed u8, so ≤255 is
  vacuous). Tighten at spec level (Rev 2.4 candidate) or enforce in the
  γ meshlet-build pipeline — decide in the γ plan.
- **Nebula vendor de-inheritance re-sync hazard** (T9): the 9
  `vendor/nebula` subcrates on Helio `scenedb20-m3` had `workspace = true`
  entries replaced with equal literals (dual-foreign-workspace resolution
  fix, zero semantic drift — T9 review verified). Any future vendor sync
  from upstream nebula reintroduces the bug unless the fix is upstreamed.
  M4-plan input, alongside the helio-snapshot disposition.
- **Seam bind-group storage budget** (T9/T10): `SceneDbBinding`'s 8
  read-only storage buffers equal the entire WebGPU default per-stage
  budget (8); visibility is VERTEX_FRAGMENT | COMPUTE. β's cull/draw
  passes must raise device limits (adapter-derived) or split the group —
  budget the β plan for it (doc'd on `SceneDbBinding` itself).
- **M3-β cull shader MUST bounds-check `mesh_index`** against the mesh
  table (T4: recycled-tail bytes untrusted — doc'd on
  `instance_info_buffer()`); and Helio's submodule lockfile carries a
  second wgpu (23.0.1, via examples→rapier→bevy, pre-existing) — the seam
  graph unifies on 30.0.0; note for the M4 gate.
- **Test 13's streaming-frozen scope is narrower than "streaming coexists
  with zero-reupload"** (M3-β T8/T10): the gate's window holds streaming
  transitions frozen for its N-frame duration and asserts zero scene-object
  reupload *within that frozen window* — it does not exercise a
  promote/demote cycle happening *during* the zero-reupload window. The
  gate proves teardown-then-rebuild is clean; it does not yet prove
  streaming and the zero-reupload property compose. A dedicated
  streaming-during-teardown-window test is M3-γ/M4 scope.
- **`revalidate_run` is generation-blind** (M3-β T2 `stress_gpu`,
  `hazard_revalidate_run_cannot_detect_a_row_reused_by_compaction_swap`,
  committed and green): a row reused by a compaction swap between a
  snapshot and its revalidation is not detectable by generation comparison
  alone in that specific hazard shape. The repro is committed as a live
  test, not just a note — closing it (or scoping it as an accepted, proven
  hazard) is M3-γ/M4 work.
- **Cull is dispatch-overhead-bound at 1k–10k tokens, on this host** (M3-β
  T9): cull dispatch cost is ~9.5–15 µs essentially flat from N=1,000 to
  N=10,000, with real N-dependence only becoming visible by N=100,000
  (~12.5–15 µs). This is directly relevant to the still-DEFERRED per-view
  multi-dispatch item (perf-validation report contract #44, design carried
  forward as R-PERF-4/#44): fixed per-dispatch overhead at this scale means
  batching multiple views' cull dispatches will matter more than N-scaling
  alone would suggest.
- **The draw-side transpose pin needs an asymmetric quad.** C5's
  column-major instance-transform convention (M3-β T5 review amendment) is
  today pinned only on the cull side (world-AABB extents, via a
  non-symmetric rotation probe). The draw-side executor (`record` /
  `record_multi_indirect`) has no equivalent asymmetric-geometry pin of its
  own — a symmetric quad fixture would not distinguish a correctly- vs.
  transposed-flattened transform in a rendered-pixel assertion. M3-γ/M4
  carry-forward.
- **`generation_write_count`'s shadow-gate scope** needs a doc note on the
  gate itself (`generation_uploads_are_shadow_gated_to_changes_only`,
  `gpu_store.rs`) clarifying exactly which write paths it covers and which
  it doesn't (streaming eviction's CPU-side-only retirement path, C6, is a
  documented exception already; the note is about making that exception
  discoverable from the gate's own doc, not a behavior change). This is
  now **owed to SceneDB's own repo** (Far-Beyond-Pulsar/SceneDB) rather
  than actionable here — `crates/core/pulsar_scenedb/src/**` is frozen to
  edits in Pulsar-Native as of M3-β T10 (extraction in progress, see §10).
  Deferred by the extraction, not dropped.
- **The equivalence spot-pin has a `first_instance` blind spot** (M3-β T9
  follow-up, `tests/draw_multi_indirect_equivalence.rs`): the repack
  spot-pin checks three slots (first visible, first zero-instance, last at
  capacity) field-for-field against the source `CullRecord`, but the
  follow-up's own mutation-kill note records that a `first_instance = 0`
  mutation (dropping the §14.1 command-slot bindless key entirely) does
  NOT fail at the spot-pin for those three slots — it happens to only
  surface downstream, at the "both mesh colors present" guard, because
  those three particular slots' real `first_instance` values are
  legitimately 0 or the mutation's effect is masked by construction. A
  spot-pin slot whose real `first_instance` is guaranteed nonzero would
  close this gap directly. M3-γ/M4 carry-forward.

## 13. M3-β decisions: device requirements (§14.1) and clamp strategy (§14.2)

### 13.1 Device requirement: `INDIRECT_FIRST_INSTANCE` (spec §14.1)

**Promoted out of a test doc comment (M3-β T10).** Spec §14.1's indirect
draw-command contract keys every command on `first_instance == command
slot` — the bindless lookup key downstream passes use to fetch
`visible_instance_ids[first_instance]` and, transitively, the row it
addresses. This is not optional decoration: without
`wgpu::Features::INDIRECT_FIRST_INSTANCE` enabled on the device, wgpu 30's
own documentation states a nonzero `first_instance` in an indirect draw's
`DrawIndexedIndirectArgs` "has to be 0, unless
`Features::INDIRECT_FIRST_INSTANCE` is enabled" — and this stack's actual
observed behavior (both the Vulkan and the platform-default backend,
empirically confirmed, M3-β T7) is to execute the draw **as if
`first_instance` were zero, silently, with no validation error**. Every
indirect draw's `@builtin(instance_index)` would then read record 0 no
matter which slot the draw call's args actually named — breaking §14.1's
bindless key outright, with no diagnostic to point at the cause. The
capability is widely supported on desktop GPUs (Vulkan/DX12/Metal all
expose it natively; the WebGPU spec merely gates it behind an opt-in
feature).

**Device-requirements callout (binding for any device the M3-β/γ cull or
draw passes run on):** `wgpu::Features::INDIRECT_FIRST_INSTANCE` MUST be
requested at `request_device` alongside `wgpu::Limits::default()` (T4
established the seam otherwise fits under the WebGPU-portable default
limits — this is the one additional feature the draw path genuinely
needs, and no more).

**Cross-reference — the two consumers cannot drift.** Helio's own renderer
already hard-requires this feature independently, with its own pinning
test:
- `helio/src/renderer/config.rs:14-18` — `required_wgpu_features` ORs
  `INDIRECT_FIRST_INSTANCE` into the required set unconditionally (both
  the native and the wasm32 arms), plus a unit test
  (`indirect_first_instance_is_required_even_when_adapter_does_not_report_it`)
  pinning that it survives even against an empty adapter-features input.
- `helio/src/renderer/setup.rs:73-74` — a runtime `assert!` at renderer
  construction that the device's features contain
  `INDIRECT_FIRST_INSTANCE`, with the message: *"Helio requires
  INDIRECT_FIRST_INSTANCE because GPU-driven object and meshlet draws use
  non-zero indirect first_instance values; create the device with
  helio::required_wgpu_features(adapter.features())"*.

`helio-scenedb`'s own GPU test contexts
(`crates/helio-scenedb/tests/support/mod.rs`,
`test_context_indirect_first_instance`) request the same feature
independently for the draw-executor test suites, with the requirement's
full rationale documented on the constructor. This design-doc callout and
the parallel CONTRACTS.md entry (§C5) exist so a future device-construction
path (e.g. the M4 real-Helio-pass integration, or a from-scratch consumer)
cannot silently reintroduce Helio's own renderer requirement without
requesting it for the SceneDB seam too, or vice versa — the two call sites
are independent code paths today and must be kept honest against each
other by documentation until M4 unifies device construction
(`EngineGpuContext` single device root, §10).

### 13.2 Clamp strategy — decided, closing §9(a) (spec §14.2)

**Measured at N=10,000 tokens / 10% visible** (the plan brief's named
operating point), 8 independent `cargo bench` process invocations (M3-β
T9, reworked after review):

| Strategy | mean-of-run-means (ns) |
|---|---:|
| (a) readback-then-clamp | 221,567.1 (range 205,743.3–247,113.3) |
| (b) conservative max-count, CPU draw-call loop | 1,572,062.1 (range 1,502,923.3–1,638,666.7) |
| (b′) conservative max-count, GPU repack + `multi_draw_indexed_indirect` | 136,049.2 (range 126,140.0–176,910.0) |

Per-run ordering: **(b′) < (a) < (b) in 8 of 8 runs, no exceptions.** The
reviewer independently reproduced this on a separate 8-run session:
(b′) < (a) in 8/8 runs, ratio range 1.485×–2.253× (report's own range:
1.40×–1.79×) — same order, no inversions, consistent magnitude band.

**The decision flips.** (b′) — `RenderPass::multi_draw_indexed_indirect`
issuing all indirect draws from ONE call, fed by a GPU-side compute
`RepackPass` that turns the 32 B `CullRecord` array into a tightly-packed
20 B `DrawIndexedIndirectArgs` buffer (no CPU stall, no extra
`wgpu::Features` beyond the universal `DownlevelFlags::INDIRECT_EXECUTION`)
— beats readback-then-clamp (a) consistently, because it pays neither
(a)'s CPU stall (~45–85 µs) nor (b)'s CPU draw-recording loop. The repack
pass itself costs **~24–60 µs GPU-side** at capacity=10,000 — cheap enough
not to erase (b′)'s advantage over (a) at this operating point.

**Correctness, not just speed: (a) and (b′) render byte-identical output.**
`tests/draw_multi_indirect_equivalence.rs` (promoted to a permanent pin,
M3-β T9 follow-up) renders one fixture (24 tokens, 5 visible at distinct
screen columns, 19 frustum-culled) via both strategies and asserts
byte-identical 64×64 RGBA8 offscreen targets: **0/16384 differing bytes.**
The reviewer independently built and ran a second, throwaway
output-equivalence probe before trusting the report's claim and got the
same result. This closes the concern that (b′)'s always-issued
`instance_count = 0` no-op draws (9,000 of 10,000 at this operating point)
might leak stray rasterized content — they provably do not.

**Recommendation for M3-γ/M4 integration:** adopt **(b′)** as the default
at visible fractions the design expects to be typical (≥~5–10%, the regime
this task actually measured) — faster, avoids the CPU stall entirely, and
`record_multi_indirect` is now a shipped `DrawExecutor` capability, not a
bench-only prototype.

**What is NOT settled, stated as speculation, not a finding:** the
crossover below which (a) may win again (because (a)'s real-draw-count
cost shrinks with the visible fraction while (b′)'s repack/multi-draw cost
stays roughly flat at ≈`capacity`) was **projected from already-measured
component costs, not independently measured by a dedicated low-visibility
sweep**. The task-9 report's own arithmetic (`V* ≈ 400–500`, ≈4–5%
visible) used stall-cost inputs carried over from an earlier session. The
**reviewer's own 8-run session measured a higher stall mean directly**
(`a_stall_mean` mean ≈ 85.8 µs across its 8 runs, vs. the ~50–70 µs the
report's crossover arithmetic assumed) and re-derived, using its own
session's data throughout: `V* ≈ 363` — about 15–30% lower than the
report's figure, shifting the projected crossover to an even lower visible
fraction (≈3.6% rather than ≈4–5%). This is the figure this design doc
records as the more conservative, better-grounded projection — but it
remains a **projection, not a measurement**: no dedicated sweep across low
visible fractions (1%, 2%, 5%) was run. A future integration targeting
extreme-culling scenes (well under ~5% visible) should run that sweep
before relying on either crossover estimate.
