# SceneDB 2.0 Performance-Validation Campaign Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute this plan task-by-task (fresh implementer per task, adversarial review between tasks).

**Goal:** Measure the implementation against the spec's performance contract — every quantitative budget and architectural bet — and produce a committed numbers-vs-expectations report with remediation items, before M3-β builds the render passes on top.

**Architecture:** This is a measurement campaign, not feature work: harness repairs first (they gate everything), then instrumentation (alloc counting, GPU timestamps), then the measurement matrix (head-to-heads, stress storms, scaling studies), then the report. Production `src/` code is touched ONLY where a harness needs a `#[doc(hidden)]` seam or a bench-honesty bug demands it — every production hunk needs explicit justification in the task report.

**Tech stack:** criterion `=0.8.2` (pinned, matches `pulsar_ecs` twins), wgpu 30 (`TIMESTAMP_QUERY` — RTX 5080/Vulkan confirmed), a counting `GlobalAlloc` test harness, the existing `SyncStats`/upload-counter instrumentation.

**Normative inputs (read before any task):**
- `.superpowers/sdd/stress-recon-perf-contract.md` — the 50-item performance contract (claim → source → target → measurability). THE campaign checklist.
- `.superpowers/sdd/stress-recon-infrastructure.md` — bench catalog, hang root cause, ECS twin comparability, timestamp capability, host specs (Ryzen 7 9800X3D 8C/16T, 64 GB, RTX 5080, Win11).

## Global Constraints

- GPU suites/benches: ALWAYS `--test-threads=1`; never run two device-touching processes concurrently.
- criterion pinned `=0.8.2` both crates; identical bench IDs are load-bearing for `--baseline` comparison — never rename a twin ID.
- Windows: author `.rs` only via Write/Edit tools (BOM hazard).
- Numbers are recorded WITH context: host specs header, warm/cold state, sample counts, and the exact command line — a number without its command is not a result.
- The full pre-existing matrix stays green after every task: core 133 / gpu-lib 85 / 26 / 11 / 52 / 8 / doc 6 / benches-check + helio-scenedb 8.
- Commit prefix `perf(scenedb):` for harness/bench code, `docs(scenedb):` for the report.

---

### Task 1: Bench-harness repairs (honesty + statistical GPU benching)

**Files:** `crates/core/pulsar_scenedb/benches/scenedb_bench.rs`.

Fix the two defects the infra recon diagnosed:
1. **GPU-bench staging leak / hang:** the two device-touching benches (`region_sync_1024_dirty_rows`, `promotion_demotion_cycle`) never `queue.submit`/`device.poll` inside the iteration loop → unbounded staging growth (~64 KB/iter, 17+ GB observed) and criterion's iteration scaling never converges. Fix inside `iter_custom`'s untimed section: after each timed boundary run, `queue.submit(std::iter::empty())` + `device.poll(wgpu::PollType::wait_indefinitely())`. This ALSO makes the timed number honest steady-state (today's 13 µs rides an ever-growing staging pool). Then verify statistical sampling completes at DEFAULT criterion settings within sane wall time (<90 s per bench) and record the new (honest) numbers vs the old smoke numbers in the task report.
2. **Mislabeled scalar bench:** `scalar_aabb_scan_*` measures the same runtime-dispatched path as `dispatched_aabb_scan_*`. Give the scalar arm a real measurement: expose the scalar kernel to benches via a `#[doc(hidden)] pub fn query_aabb_scalar_for_bench(...)` seam on `SpatialCell` (thin wrapper over the `pub(crate)` scalar arm; doc: bench-only, not API) and point `scalar_aabb_scan_*` at it. The dispatched/scalar pair must now show the real SIMD delta.
- [ ] Both GPU benches run statistically at defaults, no growth (watch process RSS), numbers recorded; scalar/dispatched pair shows a genuine delta; full matrix green; commit `perf(scenedb): honest GPU bench pumping + true scalar arm (perf-val T1)`.

### Task 2: Allocation-counting gate (§8.1 no-alloc steady state)

**Files:** Create `crates/core/pulsar_scenedb/tests/alloc_gate.rs`; `src/` only if a warm-up seam is missing.

A counting `GlobalAlloc` wrapper (`#[global_allocator]` in that test binary only; count alloc/realloc calls, thread-local arm/disarm guard so setup doesn't count). Gates, each after an explicit warm-up pass: (a) `query_aabb_in`/`query_frustum_in` — zero allocs per query; (b) harvest warm path incl. the DEI branch and the gens columns (T8's warm-up test pattern is the model) — zero allocs per `harvest_cell`; (c) `SceneGpuStore` steady-state sync with zero dirty rows — zero allocs; with N dirty rows — allocs independent of N (record the constant). Each gate is a `#[test]` with the count asserted, so §8.1 regressions fail CI-side (these are CPU-only tests — no GPU feature needed where possible; the store gate needs `gpu` + a device, put it in the gpu-gated suite).
- [ ] Gates green + provably armed (temporarily insert a `Vec::new()` push in a probe copy to see a gate trip — evidence in report, not committed); matrix green; commit `perf(scenedb): allocation-counting gates for §8.1 no-alloc claims (perf-val T2)`.

### Task 3: GPU timestamp harness

**Files:** Create `crates/core/pulsar_scenedb/benches/gpu_timing.rs` (+ `[[bench]]` entry, `required-features = ["gpu"]`); shared helper module inside the bench file (integration benches can't share modules — copy the test_context pattern).

Request `TIMESTAMP_QUERY` (+ `TIMESTAMP_QUERY_INSIDE_ENCODERS`) in the bench's device descriptor; build a small `GpuTimer` (query set of 2, resolve buffer, `queue.get_timestamp_period()`, readback → ns). Measure GPU-side time for: (a) boundary sync of N dirty rows (N ∈ {0, 1, 64, 1024}) — timestamps around the encoder that carries the copies (write_buffer is queue-internal: wrap the submission with begin/end timestamps on an encoder in the same submit; document the caveat that queue-internal staging copies may execute adjacent to, not strictly inside, the bracket — the delta across N still isolates the payload cost); (b) full-buffer re-upload of the same region (the legacy model's shape) for the same Ns. Single-shot with 100-iteration averaging inside the bench (criterion statistical mode optional — timestamps make each iteration self-measuring).
- [ ] GPU ns numbers for delta-vs-full across N recorded; harness reusable by later tasks; matrix green (new bench compiles under the benches-check CI gate); commit `perf(scenedb): GPU timestamp harness — delta vs full upload timing (perf-val T3)`.

### Task 4: The centerpiece — delta-sync vs legacy full-resync head-to-head

**Files:** Create `crates/core/pulsar_scenedb/benches/legacy_model_bench.rs` (+ `[[bench]]` gpu-gated entry).

Two simulated frame loops over identical scene content at scales S ∈ {1k, 10k, 100k rows} × mutation rates M ∈ {0%, 0.1%, 1%, 10%, 100%}:
- **SceneDB path:** `write_transform` on the mutated subset → boundary (delta sync). Record CPU frame time + `SyncStats` bytes.
- **Legacy-model path (sync_scene's shape, faithfully):** per frame, iterate ALL rows (not just mutated), rebuild a CPU-side snapshot Vec (fresh allocation, mirroring the DFS clone), and re-upload the FULL buffer via `write_buffer`. Record CPU frame time + bytes. Do NOT caricature: no serde_json in the loop (note in the report that the real sync_scene ALSO pays serde_json + light destroy/recreate + BVH rebuild — our simulation is a LOWER BOUND on legacy cost, so the measured edge is conservative).
- Zero-mutation frame MUST show SceneDB at 0 bytes uploaded (`SyncStats` assert — claim #1/#2's sharpest edge).
Produce the 15-cell matrix for both paths (time + bytes) + the crossover analysis (at what mutation rate, if any, does delta stop winning). Use T3's GpuTimer for the GPU-side of both paths at 10k.
- [ ] Matrix captured with commands; zero-mutation zero-bytes asserted as a test not just a bench; commit `perf(scenedb): delta-sync vs legacy full-resync head-to-head (perf-val T4)`.

### Task 5: ECS head-to-head baselines

**Files:** none (measurement task) — output is `.superpowers/sdd/perf-ecs-headtohead.md`.

`cargo bench -p pulsar_ecs --bench ecs_bench -- --save-baseline legacy` (+ detailed twin), then `cargo bench -p pulsar_scenedb --bench ecs_bench -- --baseline legacy` (+ twins). Capture criterion's comparison output for every shared ID; tabulate speedup/regression per operation family (spawn, despawn, query iteration, component add/remove, archetype moves). Flag any operation where scenedb regresses vs legacy ECS >10% — those become remediation items. `hotpath_profiler` twin run too (single-shot percentiles).
- [ ] Full comparison table with per-family verdicts; regressions flagged; no code changes; ledger + report doc updated.

### Task 6: Stress storms (streaming, residency, leases, DEI edge)

**Files:** Create `crates/core/pulsar_scenedb/tests/stress_gpu.rs` (gpu-gated `[[test]]` entry, `--test-threads=1`).

Four storms, each asserting the spec's protective claim, sized to run <60 s each:
1. **Hysteresis thrash-guard (claim #27):** camera jitter at 60 Hz amplitude < band width across the 310/330 edges for 10 s simulated: assert ZERO domain transitions/evictions/region allocs after the initial settle (counter-instrumented via existing seams); then amplitude > band width: transitions occur (proves the guard isn't vacuous).
2. **Eviction/recycle storm:** 64 cells over a 16-cell residency budget, round-robin promote/demote × 500: assert region-pool recycling (no unbounded pool growth — pool size plateaus), serial-pin safety (no VRAM corruption: spot readback), tail-scrub correctness under recycling pressure, and record promote/demote latency distribution.
3. **Lease revocation latency (C4 2.0 ms):** under a worst-case harvest load, measure request-revocation→lease-dropped latency distribution across 1000 revocations; assert p99 ≤ 2.0 ms (the budget) and record the histogram.
4. **DEI threshold straddle (claim #22):** cells engineered at 24%/26% hit rates: assert the strict <25% branch selection each side, bit-identical output both paths (existing oracle), and record the bandwidth delta (tokens uploaded dense vs remap) proving the compression edge at the boundary.
- [ ] All four storms green with recorded distributions; matrix green; commit `perf(scenedb): stress storms — hysteresis/residency/lease/DEI gates (perf-val T6)`.

### Task 7: Query-scan scaling study (SIMD claim #19/#50)

**Files:** extend `benches/scenedb_bench.rs` (scaling group).

Scalar vs dispatched (true scalar arm from T1) at N ∈ {1k, 16k, 256k, 1M rows} for AABB and frustum, plus a memory-traffic analysis in the report: bytes touched per row vs achieved GB/s vs the 9800X3D's bandwidth — answer WHY the delta is 1.25× at 1k (compute- vs memory-bound?) and whether it widens at scale. Compute effective entities/cycle at each N against the "16 entities/cycle" aspiration. If the scan is memory-bound, say so plainly — that's a finding, not a failure (the claim was demoted to bench-validated).
- [ ] Scaling table + analysis; commit `perf(scenedb): query-scan scaling study (perf-val T7)`.

### Task 8: The report + CI regression baselines

**Files:** Create `docs/superpowers/specs/2026-07-17-scenedb20-perf-validation-report.md`; `.github/workflows/ci.yml` (one step).

The report: host header; the 50-claim contract table with per-claim verdict — **MET** (number + margin) / **MET-QUALITATIVE** / **MISS** (number + gap + remediation item) / **DEFERRED** (needs M3-β/GPU passes — list exactly which claims and why); the head-to-head matrices (T4, T5); the storm distributions (T6); the scaling analysis (T7); a "conservatism" note (legacy simulation is a lower bound); and a remediation-items section feeding the M3-β plan. CI: add a compile-only guard for the two new bench targets if not already covered by the existing benches-check (verify — it builds ALL bench targets, so likely covered; if so, note and skip). Decide + document the criterion baseline-file policy (committed baselines vs CI-run: recommend NOT committing target/criterion data; the report's numbers are the record).
- [ ] Report committed `docs(scenedb): performance-validation report — spec contract vs measurements (perf-val T8)`; ledger closed.

## Deferred

GPU cull-efficiency claims (DEI GPU bandwidth, backface cone, Hi-Z, wavefront-fill) — need M3-β passes + the T3 timer; editor-loop end-to-end (real sync_scene head-to-head, not simulated) — needs M4 front half; Unity/Unreal comparative benching — out of scope (no comparable in-tree harness; report notes the methodology if ever wanted).

## Verification (end-to-end)

T8's report is the deliverable. Hard gates added along the way: zero-alloc tests (T2), zero-bytes-on-clean-frame test (T4), the four storm asserts (T6), lease p99 ≤ 2.0 ms (T6.3). The full pre-existing matrix stays green throughout.
