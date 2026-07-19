# M3-β Implementation Plan — Cull/Indirect Passes + Binding Tests 13/2/4/5

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (fresh implementer per task, adversarial review between tasks). Dual-workspace: Pulsar root (branch `scenedb`) + Helio submodule `crates/renderer/helio` (branch `scenedb20-m3`; submodule commits push first, then gitlink bumps).

**Goal:** The renderer consumes SceneDB: a GPU cull pass validates harvested tokens against live generations, emits indirect draw commands, and draws through the seam — gated by Test 13 (stateless teardown), Test 2 (stale-token drop), Test 4 (GPU-path transform sweep), Test 5 (overflow clamp), and GPU-vs-CPU cull equality. The perf campaign's remediation register (R-PERF-1/2/4) executes here.

**Architecture:** SceneDB side grows the per-view token/expected-gen GPU upload (design §3.1's data path onto the device) plus two core remediations (gap-coalescing experiment, §9.2.1 pinned-snapshot bypass). Helio side (`helio-scenedb`) grows the cull compute pass, the indirect command machinery, a minimal draw executor, and the test harnesses. Hi-Z integration and VG/HLOD stay out (γ); the cull term list here is: generation validation → mesh_index bounds check → near-plane W≤0 bypass → frustum.

**Tech stack:** wgpu 30 (both sides), naga reflection harnesses (both sides), the T3 GpuTimer pattern for pass timing, criterion `=0.8.2`.

**Normative inputs (read before any task):** design `specs/2026-07-16-scenedb20-m3-helio-inversion-design.md` §3/§3.1/§4/§5/§9/§12; `specs/2026-07-17-scenedb20-perf-validation-report.md` (R-PERF register + methodology laws); CONTRACTS.md C0/C4/C5 (+ §14 command-slot semantics per spec); M3-α seam code (`crates/renderer/helio/crates/helio-scenedb/`).

## Global Constraints

- C0: no scenedb→Helio edge; visibility/commands/Hi-Z are Helio-owned DERIVED data; scene data stays SceneDB-owned.
- GPU suites/benches strictly `--test-threads=1`, serialized across processes.
- All WGSL structs: 4-byte scalar fields only; every new shader struct gets Test 3 rows in BOTH reflection harnesses (SceneDB naga + helio-scenedb harness) in the same commit.
- Windows: author .rs/.toml/.wgsl/.md via Write/Edit tools only.
- Full dual matrix green after every task (Pulsar 9 commands + storms/alloc gates + helio-scenedb suite + helio-core check + the 3 CI-guard greps).
- Self-verifying fixture guards on every new perf/behavioral test (methodology law a); every gate provably able to fail (law c); no cross-binary perf comparisons under the ±20% band (law b).
- Commit prefixes: `feat(scenedb)`/`perf(scenedb)`/`test(scenedb)` Pulsar; `feat(helio-scenedb)`/`test(helio-scenedb)` submodule.

---

### Task 1: Per-view token + expected-gen GPU upload (SceneDB)

**Files:** `src/gpu/harvest.rs`, `src/gpu/scene_store.rs` or new `src/gpu/view_upload.rs`, `tests/gpu_harvest.rs`, CONTRACTS.md (C5 note: token buffer u32 rows + aligned gen u32 rows, global-row-valued, sentinel-free per T6 finding).

**Produces:** `ViewTokenBuffers { tokens: wgpu::Buffer, expected_gens: wgpu::Buffer, count: u32 }` + `upload_view(&mut self, ctx, staging: &HarvestStaging, class: MeshClass) -> ViewTokenBuffers`-shaped API (exact placement per the harvest module's idiom — the implementer reads first). One upload counter (Test 13 pattern). Capacity-managed like SceneBuffer (grow-on-demand, no per-frame realloc after warm-up — extend the T2 alloc gate to this path).
- Tests: upload → readback: token[i] pairs with expected_gens[i] positionally (gen-diverse fixture with self-verifying guards — T8's fixtures are the template); zero-alloc steady state after warm-up; upload counter increments/no-increment.
- [ ] Commit `feat(scenedb): per-view token + expected-gen GPU upload (M3-b T1, design §3.1)`

### Task 2: R-PERF-2 — §9.2.1 pinned-snapshot bypass + `any_held()` consumer (SceneDB core)

**Files:** `src/gpu/phase.rs` (BoundaryPhase), `src/lease.rs`, `src/gpu/harvest.rs` (`revoke_overdue`), `src/cell.rs`/`src/page.rs` as needed, tests.

Closes contract #32 (perf-report MISS): compaction must not stall indefinitely on a revoked-but-undropped lease. Shape (from spec §9.2.1 + T6 review): BoundaryPhase's compact step consults `any_held()`; when a lease is held past the C4 2.0 ms timeout, `revoke_overdue` fires AND compaction proceeds against the primary layout while the straggler reads the pinned snapshot (LivenessSnapshot double-buffer is the existing seam — the implementer maps §9.2.1's exact wording onto it and documents any spec-vs-primitive gap honestly rather than force-fitting). `revoke_overdue` gains the slot-release semantics the T6 review found missing.
- Tests: compaction-under-held-lease proceeds after timeout (storm-3 fixture extended); straggler reading the snapshot sees consistent pre-compaction data; the release path mutation-killed.
- [ ] Commit `feat(scenedb): pinned-snapshot compaction bypass — any_held consumer (M3-b T2, §9.2.1 / contract #32)`

### Task 3: R-PERF-1 — gap-tolerant coalescing experiment (SceneDB)

**Files:** `src/gpu/buffer.rs` (+ bench extension in `benches/legacy_model_bench.rs` or `scenedb_bench.rs`).

Data-justified experiment, DECISION as deliverable: add a gap-threshold G to `sync_region`'s run-length coalescing (merge runs separated by < G clean rows, uploading the gap bytes). Measure the T4 scattered case (10k rows, 1% scattered) at G ∈ {0 (today), 4, 16, 64}: CPU frame time + ranges + bytes. Accept the G that wins (expect: ranges collapse, bytes rise slightly, CPU drops toward contiguous) or REJECT with data if none does. Wire the accepted G as a documented constant (not config); update the T2 alloc-gate scope note + perf report §addendum with the outcome.
- [ ] Commit `perf(scenedb): gap-tolerant dirty coalescing — measured decision (M3-b T3, R-PERF-1)`

### Task 4: Bind-group restructure for the pass budget (#47) (helio-scenedb)

**Files:** submodule `crates/helio-scenedb/src/lib.rs`, `src/wgsl.rs`, `tests/binding_layout.rs`, `tests/seam_smoke.rs`.

Resolve the 9>8 storage-budget MISS before the cull pass adds its own buffers: split `SceneDbBinding` into **group 0 = cull-read set** (tokens+gens come per-view at group 2; group 0: transforms, instance info, slot mirror, generations, mesh table) and **group 1 = draw/material set** (cluster DAG, meshlets, materials, cell metadata) — ≤ 8 per group with headroom documented per pass; keep `ShaderStages` per-group minimal (COMPUTE for group 0's cull consumers, VERTEX_FRAGMENT for group 1's draw consumers — cull never binds group 1). Update WGSL, harness, smoke test; smoke test should now pass under DEFAULT limits (assert that — it's the point).
- [ ] Submodule commit `feat(helio-scenedb): split scene binding — cull/draw groups under default limits (M3-b T4, contract #47)` + gitlink bump.

### Task 5: The cull compute pass (helio-scenedb)

**Files:** submodule `crates/helio-scenedb/src/cull.rs` (new), `src/wgsl.rs` (CULL_WGSL: DrawCommand struct + pass), `tests/` (+ Test 3 rows both harnesses for DrawCommand + any new struct).

`CullPass { pipeline, bind groups }` + `record(&mut encoder, view: &ViewTokenBuffers, uniforms)`. Shader per design §4 (β term list): thread i < count → token = tokens[i] (global row) → **gen validation**: `generations[slot_mirror[row]]`… exact indexing per §3.1 (implementer derives from the shipped WGSL + spec §14; expected_gens[i] mismatch → drop + `stale_drops` atomic telemetry) → **mesh_index bounds check** vs mesh-table length uniform (T4 recycled-tail defense — REQUIRED, drop + telemetry) → transform fetch → local AABB from MeshMetadata → |M₃ₓ₃| world AABB (§11) → W≤0 near-clip bypass flag (§12) → frustum planes (uniform) → passed: bounded-atomic command-slot alloc, write DrawCommand (indexed-indirect layout: index_count, instance_count=1, first_index, base_vertex, first_instance=slot) + `visible_instance_ids[slot] = row` (§14.2 command-slot-keyed, row-valued). Overflow: atomic result ≥ capacity → skip write (count still increments; CPU clamps on readback — §14.2 conservative default; readback-latency measurement deferred to T9's timing task, recorded in report).
- Tests (headless, real device): small scene → dispatch → readback commands + visible ids: exact expected visible set (hand-computable fixture); telemetry zero. Self-verifying guards (fixture must contain ≥1 of each: visible, frustum-culled, near-clip-flagged row).
- [ ] Submodule commit `feat(helio-scenedb): cull compute pass — gen validation + bounds + frustum + indirect emit (M3-b T5, design §4)` + gitlink bump.

### Task 6: Test 2 (stale-token drop) + GPU-vs-CPU cull equality

**Files:** submodule tests.

Test 2 (binding): inject a stale token+gen pair (free_deferred + boundary so the live gen bumps; upload the OLD pair) → dispatch → assert: no command written for it, `stale_drops` == injected count, everything else unaffected. Gen-diverse fixture + guards (T8 law). Mesh-index-out-of-range injection twin for the bounds check.
Equality: same token set culled by a CPU reference (straight Rust port of the term list, written independently of the WGSL — no shared constants beyond the contract structs) — visible sets must match EXACTLY across ≥3 randomized fixtures (seeded, printed); mismatch prints the differing rows.
- [ ] Submodule commit `test(helio-scenedb): Test 2 stale-drop + GPU-vs-CPU cull equality (M3-b T6)` + gitlink bump.

### Task 7: Indirect draw executor + Test 4 (transform sweep)

**Files:** submodule `crates/helio-scenedb/src/draw.rs` (new), tests.

Minimal draw executor proving the C5 end-to-end path (NOT the full Helio integration — that is M4): render pass drawing `multi_draw_indexed_indirect` (or count-clamped loop — check wgpu-30 surface; document choice) over GeometryArena's VERTEX/index buffers, a trivial WGSL vertex/fragment (fetch transform via visible_instance_ids + instance index, output flat color by mesh_index) into an offscreen target. Test 4: sweep a transform through positions across frames (write_transform → boundary → cull → draw → readback target) and assert the rendered bounding pixels track the CPU-computed positions (coarse grid assert, not pixel-perfect rasterization comparison; document tolerance).
- [ ] Submodule commit `feat(helio-scenedb): indirect draw executor + Test 4 transform sweep (M3-b T7)` + gitlink bump.

### Task 8: Test 5 (overflow clamp) + Test 13 (stateless teardown — THE C0 GATE)

**Files:** submodule tests (+ any SceneDB-side counter accessor gaps found — tiny Pulsar commits allowed).

Test 5: command capacity < visible count → assert: atomic count readback > capacity, commands written == capacity exactly, zero corruption beyond (buffer tail untouched — canary bytes), CPU clamp path exercised, silent-drop count reported.
Test 13 per design §5's EXACT assertion set: build scene via SceneDB → render N frames with executor A → DROP A entirely → construct executor B → N frames. Assert across the window: Σ SyncStats.bytes == 0; Δ generation_write_count == 0; ALL asset-store upload counters (mesh/cluster/meshlet/material/texture/geometry) unchanged; streaming transitions frozen (or write_cell_metadata counted separately — document); offscreen-target hash byte-identical A-final vs B-converged (no TAA/jitter in this harness — trivially satisfied, note it); device + every scene SSBO alive (buffer IDs/pointers unchanged). This is C0's binding acceptance criterion — the ownership law proven executable.
- [ ] Submodule commit `test(helio-scenedb): Tests 5 + 13 — overflow clamp + stateless renderer teardown (M3-b T8, C0 gate)` + gitlink bump.

### Task 9: Cull-efficiency + pass timing (R-PERF-4 measurement)

**Files:** submodule bench or timed test (T3 GpuTimer pattern copied per its doc).

Measure with timestamps: cull dispatch GPU time at N ∈ {1k, 10k, 100k tokens} (visible fractions 100%/50%/10%); indirect-draw time for the visible set; §14.2 counter-readback latency (the deferred §9(a) decision — measure `map_async` round-trip on the atomic counter buffer, decide clamp strategy: readback vs conservative max-count, RECORD the decision + numbers). Compare cull+draw total vs a no-cull draw-everything baseline — the first real "CPU out of the GPU inner loop" evidence (perf-report DEFERRED #5 partially closes; note what M4 editor integration still owes).
- [ ] Submodule commit `perf(helio-scenedb): cull/draw pass timing + readback-latency decision (M3-b T9)` + gitlink bump.

### Task 10: Docs wrap + dual-matrix + report addenda

**Files:** Pulsar: perf report §addendum (T3/T9 outcomes: gap-coalescing decision, readback decision, deferred-claims #5/#6/Test-13 verdict flips), design doc Status line, lib.rs milestone doc, README matrix update; CONTRACTS.md C4/#32 note if T2 changed semantics (cite Rev 2.4 routing).

Full dual acceptance matrix (every suite both workspaces + storms + alloc gates + guards), verdict-flip table for the perf report's DEFERRED claims now measured (which flipped to MET, with numbers), M3-γ carry-forward list.
- [ ] Commit `docs(scenedb): M3-beta docs — cull/indirect complete, deferred-claims verdicts flipped (M3-b T10)`

## Deferred

M3-γ: VG cluster traversal, meshlets raster, HLOD per-cell proxy draws (§4.1), Tests 7/8, Hi-Z §13 term + §18 re-point (γ leads with it), margin-fade α column. M4: real Helio pass integration behind SceneResources, sync_scene dismantling, editor visual verification, engine-wide wgpu-30.

## Verification (end-to-end)

T8's Test 13 is the milestone gate (C0 executable). Named new gates: Test 2 + bounds-check twin (T6), GPU-vs-CPU equality (T6), Test 4 (T7), Test 5 (T8), contract #32 closure (T2), #47 closure (T4 — smoke under default limits). The perf report's DEFERRED set (#5, #6, Test-13 rows) gets verdict-flipped by T10 with T9's numbers.
