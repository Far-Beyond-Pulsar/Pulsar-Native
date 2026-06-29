# SceneDB 2.0 — Stage 0: Spec Rev 2.2 & Frozen Contracts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce SceneDB 2.0 spec Revision 2.2 (all logged analysis issues resolved, SceneDataCorrections.md merged, wgpu adaptation normative) and a frozen `CONTRACTS.md` that Milestones 1–4 build against.

**Architecture:** Pure documentation stage. The spec of record is edited in the Research repo (`C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md`), then re-vendored into Pulsar-Native. `CONTRACTS.md` is authored in Pulsar-Native at `docs/superpowers/specs/CONTRACTS.md` and copied to Helio. After this stage, contract changes flow spec → CONTRACTS.md → code, never code-first.

**Tech Stack:** Markdown. Verification = grep checks against the edited files (no code).

**Repos touched:** `C:\Users\Sepehr\Desktop\Dev\Research` (spec), `C:\Users\Sepehr\Desktop\Dev\Pulsar-Native` (vendored spec + CONTRACTS.md), `C:\Users\Sepehr\Desktop\Dev\Helio` (mirrored CONTRACTS.md).

**Design doc:** `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`

---

### Task 1: Resolve the handle / swap-and-pop contradiction (spec §3 + §4.4)

The blocker issue: §3.1 says the handle's index field is "a direct unsigned integer offset into the core registry arrays", but §4.4's swap-and-pop compaction moves entities between rows, so a handle index cannot be a row offset. Resolution (from the design doc): handles carry a **stable slot ID**; pages store rows densely; a **slot→row indirection table** bridges them.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md` (§3.1 "Bit layout" Index paragraph, ~line 152; §4.4 critical-constraint block, ~lines 239–241)

- [ ] **Step 1: Rewrite the §3.1 Index paragraph**

Replace the paragraph beginning `**Index (bits 0–31):** A direct unsigned integer offset into the core registry arrays.` with:

```markdown
**Index (bits 0–31):** A **stable slot ID**. Slot IDs are allocated from a free
pool and never change for the lifetime of an allocation. A slot ID is *not* a row
offset into the SoA page columns: pages store live elements densely, and
swap-and-pop compaction (Section 4.4) moves elements between rows. The registry
maintains a **slot→row indirection table** — one `u32` row index per slot,
updated during compaction — so dereferencing a handle is two O(1) array reads
(slot → row, then row → column data). Supports up to $2^{32} - 1$ concurrent
live slots per registry. The generation array and the slot→row table are both
indexed by slot ID; only column data is indexed by row.
```

- [ ] **Step 2: Rewrite the §4.4 critical-constraint block**

Replace the `> **Critical constraint:**` blockquote with:

```markdown
> **Critical constraint:** Swap-and-pop changes which entity occupies a given
> *row*. Handle dereference is unaffected — the slot→row indirection table is
> updated atomically with the compaction (Section 3.1), so a valid handle always
> resolves to its entity's current row. However, **harvested index arrays contain
> raw row indices**, not handles, for GPU lockstep addressing. Clients must treat
> harvested row arrays as valid only for the frame in which they were issued.
> The client lease API (Section 9.2) enforces this by invalidating all
> outstanding scratch buffers at the frame-boundary phase.
```

- [ ] **Step 3: Fix the §3.1 "within a single cell context" wording**

In the same Index paragraph region, the old text claimed 4.29 B allocations "within a single cell context". The replacement text in Step 1 already scopes the limit to "per registry" (the registry is global, cells reference slots). Verify no other occurrence remains:

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "single cell context"`
Expected: no matches

- [ ] **Step 4: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "slot→row|slot ID"`
Expected: matches in §3.1 and §4.4

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: resolve handle/swap-and-pop contradiction via slot->row indirection"
```

---

### Task 2: Model physics writeback as a second writer (spec §2 + §22)

§2 names the game thread "the only routine writer", but the physics solver writes back transforms. Resolution: writers are serialized **per phase** — the simulation phase gains explicit sub-phases.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md` (§2 access-pattern paragraph after the mermaid diagram, ~line 133; §22 `SIM` node description, ~line 832)

- [ ] **Step 1: Replace the §2 access-pattern paragraph**

Replace the paragraph beginning `The access pattern asymmetry is intentional. The game thread is the only routine writer during the simulation phase.` with:

```markdown
The access pattern asymmetry is intentional. Writes are serialized **per
sub-phase**, not globally: the simulation phase consists of an ordered sequence
of write windows — gameplay mutation (game thread), then physics solver
writeback (transforms and velocities only). Exactly one writer class is active
in any window, enforced by write-leases issued by Layer 2. All other clients
are readers during the harvest phase, and the strict phase separation means no
reader ever sees a partial write.
```

- [ ] **Step 2: Update the §22 frame-lifecycle SIM node**

In the §22 mermaid `SIM` node, replace `Game thread & physics mutations` with `Sub-phase A: game thread mutation\nSub-phase B: physics writeback`.

- [ ] **Step 3: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "only routine writer"`
Expected: no matches

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: model physics writeback as serialized simulation sub-phase"
```

---

### Task 3: Specify the same-frame Hi-Z pyramid rebuild (spec §13 + §18)

§13 says Hi-Z is "a mipmap of the previous frame's depth buffer", but §18 claims the traditional pass populates Hi-Z "for the VG pass" in the same frame. Resolution: add an explicit rebuild pass.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md` (§13 intro, ~line 480; §18 paragraph after the mermaid, ~line 682)

- [ ] **Step 1: Amend the §13 intro**

After the sentence ending `…where each texel stores the **maximum** (furthest) depth value in its footprint.`, append:

```markdown
Two Hi-Z states exist per frame: the **previous-frame pyramid** (used by the
first cull pass of the frame) and the **intra-frame pyramid**, produced by an
explicit **Hi-Z rebuild pass** — a compute mip-chain reduction dispatched after
the traditional raster pass completes and before the VG object-level cull
begins. Any pass that claims same-frame occlusion benefit must consume the
intra-frame pyramid; consuming the raw depth buffer mid-frame is a contract
violation.
```

- [ ] **Step 2: Amend §18's ordering paragraph**

Replace `(it tends to cover large surfaces quickly, populating the Hi-Z for the VG pass)` with `(it tends to cover large surfaces quickly; the Hi-Z rebuild pass defined in Section 13 then regenerates the pyramid from the partially populated depth buffer before the VG object-level cull dispatches)`.

- [ ] **Step 3: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "Hi-Z rebuild pass|intra-frame pyramid"`
Expected: ≥ 2 matches

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: specify same-frame Hi-Z rebuild pass between traditional and VG passes"
```

---

### Task 4: Fix the mesh-metadata layout claims for WGSL (spec §6.1 + §10)

Two problems: (a) the "explicit 16-byte alignment of the local_aabb fields is preserved" claim is arithmetically false (offsets 40 and 56 are not 16-aligned); (b) the layout contract must be restated for WGSL, not GLSL scalar layout.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md` (§6.1 paragraph after the table, ~line 323; §10 GLSL extension block, ~lines 419–426)

- [ ] **Step 1: Replace the false alignment sentence in §6.1**

Replace the sentence `The explicit 16-byte alignment of the local_aabb fields is preserved — cluster_table_offset at byte 52 and meshlet_count at byte 68 sit in the same positions the padding occupied and carry the same alignment guarantee.` with:

```markdown
All fields are 4-byte aligned scalars. The struct is authored in WGSL as scalar
`f32`/`u32` fields only — never `vec3<f32>`, which carries 16-byte alignment in
WGSL and would shift every subsequent offset. `cluster_table_offset` at byte 52
and `meshlet_count` at byte 68 occupy the positions of the former padding
fields, so the 72-byte total size and every existing field offset are unchanged.
```

- [ ] **Step 2: Replace the §10 GLSL extension block**

Replace the paragraph beginning `All shaders interacting with these SSBOs must declare:` plus the GLSL code block and the paragraph after it (ending `…validates this contract statically.`) with:

```markdown
All shaders interacting with these SSBOs are authored in **WGSL**. WGSL storage
buffer layout rules are normative for every shared struct, with one project-wide
authoring constraint: **shared structs use scalar `f32`/`u32`/`i32` fields
exclusively** (no `vec3`, which has 16-byte alignment; no implicit padding).
Vector math inside shaders reconstructs vectors from scalars at load time.
The hardware alignment verification test (Test 3, Part VI) compares the byte
offset of every host Rust field against the offsets reported by naga reflection
of the compiled WGSL and fails on any single-byte difference.
```

- [ ] **Step 3: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "GL_EXT_scalar_block_layout"`
Expected: no matches

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: WGSL layout contract; fix false 16-byte alignment claim"
```

---

### Task 5: Remaining analysis-log corrections (spec §8.3 boundary note, §9.2, §12, §13.1, §16.3)

Five smaller fixes from the embedded analysis log.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md`

- [ ] **Step 1: §13.1 Hi-Z floor-mip wording**

Replace `Using the floor ensures the selected mip level is never finer than the projected footprint.` with:

```markdown
Using the floor selects the finest mip whose $2 \times 2$ texel gather still
covers the projected footprint: one texel at level $\lfloor \log_2(\text{MaxDim})
\rfloor$ spans up to $\text{MaxDim}$ pixels, so a $2 \times 2$ gather spans up
to $2\,\text{MaxDim}$ — always at least the footprint.
```

(The Appendix A row "Floor operator in mip selection" rationale must be updated to match: replace its rationale cell with `A 2×2 gather at the floored level always covers the projected footprint; ceil would over-coarsen and inflate false occlusion.`)

- [ ] **Step 2: §12 near-plane bypass narrowing**

After the §12.1 formula paragraph (ending `…the culling pipeline is bypassed for that object.`), append:

```markdown
To keep the bypass population small, a **view-space pre-test** runs first: the
object's view-space AABB is tested against the near-plane slab. Only objects
whose view-space bounds actually straddle $z = -z_{\text{near}}$ enter the
W≤0 corner check; objects fully in front of the near plane proceed through the
normal culling pipeline even when large.
```

- [ ] **Step 3: §16.3 VG error metric radius correction**

Replace the line `$$d = \|C_{\text{world}} - P\|$$` and the sentence introducing it with:

```markdown
$$d = \max\!\left(\|C_{\text{world}} - P\| - r_{\text{world}},\ z_{\text{near}}\right)$$

Where $r_{\text{world}}$ is the node's bounding-sphere radius. Subtracting the
radius uses the *nearest* point of the node rather than its center, which
prevents error underestimation at grazing angles and for large nodes close to
the camera.
```

- [ ] **Step 4: §9.2 lease slots for dynamic thread pools**

Replace `The lease is a lightweight token stored in a per-cell atomic bitmask — one bit per registered worker thread.` with:

```markdown
The lease is a lightweight token stored in a per-cell atomic bitmask — one bit
per **lease slot**. Lease slots are acquired from a fixed-size pool (default 64,
matching the bitmask width) at query start and released at query end; they are
not bound to thread identity, so dynamic thread pools, work-stealing schedulers,
and nested queries are all supported. Pool exhaustion blocks the requesting
query until a slot frees (a saturated pool indicates a leaked lease, surfaced by
Test 1).
```

- [ ] **Step 5: §8.3/§18 boundary clarification**

At the end of §8.3, append:

```markdown
Production of the unified token array is a Layer 1 responsibility; all
*consumption-side* partitioning (LOD/VG/HLOD splits, DEI-driven dense
compaction, staging-buffer packing) belongs to Layer 2, as specified in
Section 18.
```

- [ ] **Step 6: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "lease slot|view-space pre-test|r_\{\\\\text\{world\}\}"`
Expected: matches for each

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: apply remaining analysis-log corrections (Hi-Z wording, near-plane pre-test, VG radius, lease slots, layer boundary)"
```

---

### Task 6: Merge SceneDataCorrections.md into the spec

The Helio-repo addendum (`C:\Users\Sepehr\Desktop\Dev\Helio\SceneDataCorrections.md`) has five normative items not yet in the spec. The stride conflict (its 256 B vs the spec's 128 B) resolves in favor of **128 B** — update the addendum content as it's merged.

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md`
- Reference: `C:\Users\Sepehr\Desktop\Dev\Helio\SceneDataCorrections.md`

- [ ] **Step 1: Add lease revocation to §9.2**

Append to §9.2 (after the lease-slot text from Task 5):

```markdown
#### 9.2.1 Lease timeout and revocation

The liveness bitmask and index-column registries are **double-buffered**: a
read-lease pins a snapshot of the current frame topology, not the live buffers.
If a lease is still held 2.0 ms into the frame-boundary isolation phase, Layer 2
forcibly revokes it: the holder's handle set is pushed to a secondary stale
validation lane (reads continue against the pinned snapshot; all results are
re-validated against live generations on use) and compaction proceeds on the
primary layout immediately. A revocation is logged; persistent revocations from
the same client are a bug in that client (Test 10).
```

- [ ] **Step 2: Add the holistic stride check to §7.1**

Append to §7.1:

```markdown
The guardrail is evaluated **holistically per cell composition**, not per type
registration in isolation: the ingestion macro aggregates the cumulative
per-element byte size of *all* columns registered against a shared cell type.
Splitting one logical layout into many small registrations cannot bypass the
limit — the combined cross-component stride against any single cell type must
stay ≤ 128 bytes or compilation fails.
```

- [ ] **Step 3: Add spatial hysteresis to §5 (new §5.5)**

Append a new subsection after §5.4:

```markdown
#### 5.5 Domain transition hysteresis

Promotion and demotion boundaries are asymmetric to prevent oscillation when an
observer hovers on a cell boundary:

$$\text{PromotionBoundary} = \text{CellBounds} + \Delta_{\text{pad}}$$
$$\text{DemotionBoundary} = \text{CellBounds} + \Delta_{\text{pad}} + \delta_{\text{hysteresis}}$$

A promoted cell stays in its domain until the observer exits the cell bounds
plus a padding of 10% of the cell width. Sub-pixel camera jitter therefore
never triggers domain churn (Test 11).
```

- [ ] **Step 4: Add DEI dense compaction to §8 (new §8.5)**

Append a new subsection after §8.4:

```markdown
#### 8.5 Density Efficiency Index and dense compaction

Before uploading a harvested token array to VRAM, Layer 2 computes the
**Density Efficiency Index**:

$$\text{DEI} = \frac{\text{Count}(\text{valid tokens})}{\text{total token slots}}$$

If DEI < 25%, lockstep streaming of the sparse array is bypassed: a vectorized
host-side reduction (SIMD masked compress-store) strips null sentinels and
produces a dense, packed index payload plus a compact remap table. This bounds
the GPU bandwidth wasted on `0xFFFF_FFFF` tokens in sparse cells (Test 12).
```

- [ ] **Step 5: Add the expanded Hi-Z kernel to §13.2**

Append to §13.2:

```markdown
Additionally, if the projected extent spans more than two texels of the selected
mip along either screen axis (possible for elongated or diagonal footprints),
the shader expands the gather kernel from $2 \times 2$ to $3 \times 3$ or
$4 \times 4$ so the conservative-coverage guarantee holds for non-square
footprints.
```

- [ ] **Step 6: Add Tests 10–12 to Part VI**

Append after Test 8 in §21:

```markdown
#### Test 10 — Editor lease stall compliance
Opens a persistent entity selection lease in Layer 2, then forces immediate
frame-isolation compaction in Layer 1. Pass: execution continues with zero
lockups; the lease is revoked per §9.2.1 and the holder's reads complete
against the pinned snapshot.

#### Test 11 — Grid boundary oscillation compliance
Jitters camera parameters along a cell grid boundary at 60 Hz. Pass: zero
redundant domain transitions, host-to-device allocations, or buffer
recreation requests (hysteresis per §5.5 absorbs the jitter).

#### Test 12 — Sparse cell compaction compliance
Populates a cell with 10,000 logic-only entities and 5 meshes, then harvests.
Pass: DEI < 25% triggers dense compaction per §8.5; the VRAM payload contains
no null-token cascades.
```

- [ ] **Step 7: Mark the addendum as merged and commit both repos**

Prepend to `C:\Users\Sepehr\Desktop\Dev\Helio\SceneDataCorrections.md`:

```markdown
> **MERGED:** All items in this addendum were merged into SceneDB2.0.md Rev 2.2
> (2026-06). The stride limit stated below as 256 bytes was superseded by the
> 128-byte limit in Rev 2.1 §7.1. This file is retained for history only.
```

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: merge SceneDataCorrections addendum (lease revocation, holistic stride, hysteresis, DEI, tests 10-12)"
git -C C:\Users\Sepehr\Desktop\Dev\Helio checkout -b scenedb20
git -C C:\Users\Sepehr\Desktop\Dev\Helio add SceneDataCorrections.md
git -C C:\Users\Sepehr\Desktop\Dev\Helio commit -m "docs: mark SceneDataCorrections as merged into spec Rev 2.2"
```

---

### Task 7: Add the wgpu adaptation appendix (new Appendix C)

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md` (append before "Appendix A" the marker note, content as new "Appendix C" after Appendix B)

- [ ] **Step 1: Append Appendix C**

Append after Appendix B:

```markdown
## Appendix C — wgpu Implementation Mapping (Normative)

The reference implementation targets wgpu (custom Far-Beyond-Pulsar fork) with
WGSL shaders. The following mappings are normative; spec text using Vulkan
terminology is to be read through this table.

| Spec mechanism | wgpu implementation |
|---|---|
| Timeline semaphore retirement tokens (§20) | Monotonic host-side submission serial per queue submit; `Queue::on_submitted_work_done` callback marks the serial complete. The retirement queue drains only entries whose serial is marked complete. Frame-counter arithmetic remains forbidden. |
| Task/mesh shader VG pipeline (§17) | Compute-shader cluster cull (DAG traversal + cone/frustum tests) emitting per-meshlet `DrawIndexedIndirect` records with `instance_count ∈ {0, 1}`; drawn via `multi_draw_indexed_indirect`. One cull dispatch + one indirect draw per view. |
| `vkCmdDrawIndexedIndirectCount` (§14) | GPU writes the atomic draw counter; the CPU clamps host-side after the compute pass (§14.2 already specifies CPU-side clamping). Where readback latency is unacceptable, the full command buffer is submitted with overflowed slots holding `instance_count = 0`. |
| `vkCmdDrawMeshTasksIndirectEXT` (§18) | `multi_draw_indexed_indirect` over the VG pipeline's per-meshlet command buffer. |
| GLSL + scalar block layout (§10) | WGSL with the scalar-fields-only authoring rule (§10). Verified by naga reflection (Test 3). |
| Bindless descriptor arrays (§10) | `binding_array` / partially-bound texture arrays as exposed by the fork; capability-gated with a bound-array fallback. |
| AVX-512 SIMD scans (§8.2) | Portable SIMD with runtime dispatch: AVX-512 → AVX2 → NEON → scalar. The scalar path is the reference implementation; property tests assert bit-identical results across paths. Throughput targets are validated by benchmark, not assumed from instruction width. |
```

- [ ] **Step 2: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "Appendix C"`
Expected: 1 match

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: add normative wgpu adaptation appendix"
```

---

### Task 8: Cut Revision 2.2 — header, changelog, strip the analysis block

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md`

- [ ] **Step 1: Bump the revision line**

Replace `> **Revision 2.1**` with `> **Revision 2.2**` (keep authors, update month to the current month).

- [ ] **Step 2: Delete the embedded analysis block**

Delete everything from the line `# |=====================================|` (the `CLAUDE ANALYSIS` banner) to end of file — every issue in it is now either resolved (Tasks 1–5) or explicitly deferred in Appendix B.

- [ ] **Step 3: Add a revision-history table**

After the revision line, add:

```markdown
> **Rev 2.2 changes:** stable-slot/row-indirection handle semantics (§3.1, §4.4);
> physics writeback sub-phase (§2, §22); intra-frame Hi-Z rebuild pass (§13, §18);
> WGSL layout contract replacing GLSL scalar layout (§6.1, §10); near-plane
> view-space pre-test (§12); VG error radius correction (§16.3); lease slots +
> timeout revocation (§9.2); hysteresis (§5.5); DEI compaction (§8.5); expanded
> Hi-Z kernels (§13.2); Tests 10–12; normative wgpu mapping (Appendix C).
```

- [ ] **Step 4: Verify and commit**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md" -Pattern "CLAUDE ANALYSIS"`
Expected: no matches

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Research add public/drafts/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Research commit -m "spec: cut Revision 2.2"
```

---

### Task 9: Author CONTRACTS.md

The single frozen-contract file both repos build against. Content is extracted from Rev 2.2 — no new design here, only consolidation into implementable signatures.

**Files:**
- Create: `C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\docs\superpowers\specs\CONTRACTS.md`

- [ ] **Step 1: Write CONTRACTS.md**

```markdown
# SceneDB 2.0 — Frozen Cross-Layer Contracts

**Source of truth:** SceneDB2.0.md Rev 2.2. Changes require editing the spec
first, then this file, then code. Code-first contract drift is a review reject.

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
```

- [ ] **Step 2: Commit and mirror to Helio**

```powershell
git -C C:\Users\Sepehr\Desktop\Dev\Pulsar-Native add docs/superpowers/specs/CONTRACTS.md
git -C C:\Users\Sepehr\Desktop\Dev\Pulsar-Native commit -m "docs: freeze SceneDB 2.0 cross-layer contracts"
Copy-Item C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\docs\superpowers\specs\CONTRACTS.md C:\Users\Sepehr\Desktop\Dev\Helio\CONTRACTS.md
git -C C:\Users\Sepehr\Desktop\Dev\Helio add CONTRACTS.md
git -C C:\Users\Sepehr\Desktop\Dev\Helio commit -m "docs: mirror frozen SceneDB 2.0 contracts"
```

---

### Task 10: Re-vendor Rev 2.2 into Pulsar-Native

**Files:**
- Modify: `C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\docs\superpowers\specs\SceneDB2.0.md` (overwrite with Rev 2.2)

- [ ] **Step 1: Copy and commit**

```powershell
Copy-Item C:\Users\Sepehr\Desktop\Dev\Research\public\drafts\SceneDB2.0.md C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\docs\superpowers\specs\SceneDB2.0.md -Force
git -C C:\Users\Sepehr\Desktop\Dev\Pulsar-Native add docs/superpowers/specs/SceneDB2.0.md
git -C C:\Users\Sepehr\Desktop\Dev\Pulsar-Native commit -m "docs: vendor SceneDB 2.0 spec Rev 2.2"
```

- [ ] **Step 2: Verify Stage 0 exit criteria**

Run: `Select-String -Path "C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\docs\superpowers\specs\SceneDB2.0.md" -Pattern "Revision 2.2"`
Expected: 1 match

Stage 0 is complete when: Rev 2.2 committed in Research, vendored in Pulsar-Native, CONTRACTS.md committed in both repos, and `SceneDataCorrections.md` marked merged.
