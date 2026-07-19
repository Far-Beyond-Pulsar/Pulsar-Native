# SceneDB 2.0 — Performance-Validation Report (Spec Contract vs. Measurements)

**Campaign:** perf-val T1–T8 · **Branch:** `scenedb` · **Date:** 2026-07-17
**Status:** CAMPAIGN CLOSED. This document is the durable deliverable — every other
campaign artifact (`.superpowers/sdd/perfval-task-*`, `stress-recon-*`) is a gitignored
working note; this report is the record.

---

## 1. Header

### 1.1 Host

| | |
|---|---|
| CPU | AMD Ryzen 7 9800X3D, 8C/16T, 96 MB L3 (3D V-Cache) |
| RAM | 64 GB DDR5 (61.7 GB usable) |
| GPU | NVIDIA GeForce RTX 5080 (discrete), Vulkan backend, driver NVIDIA 610.47 / 32.0.16.1047 (AMD Radeon iGPU present, unused — `HighPerformance` adapter preference selects the 5080 in every harness) |
| OS | Windows 11 Pro, build 26200 |
| Toolchain | `rustc 1.97.0` (`2d8144b78`, 2026-07-07), channel pinned via `rust-toolchain.toml` (`1.97`, components `cargo`/`rustfmt`/`clippy`, target `wasm32-unknown-unknown` also installed but unused here) |
| criterion | `=0.8.2`, pinned identically in `pulsar_scenedb` and `pulsar_ecs` (`[dev-dependencies]`) — pinning is load-bearing for the T5 `--baseline` cross-binary comparison and for bench-ID stability across the whole campaign |

All GPU-touching commands were run serially, one process at a time (`--test-threads=1`
where applicable) — no two device-touching processes ever ran concurrently on this
host, per the campaign's global constraint.

### 1.2 Methodology summary

- **CPU timing:** criterion `=0.8.2` statistical sampling (100 samples/measurement-time
  default unless a task explicitly reduced sampling for wall-time budget, e.g. T7's
  `sample_size(30)` / 2 s measurement-time for its 16-ID scaling sweep — stated inline
  wherever used).
- **GPU timing:** a from-scratch `TIMESTAMP_QUERY` harness (T3), the first in this crate
  to opt into `Features::TIMESTAMP_QUERY | TIMESTAMP_QUERY_INSIDE_ENCODERS` at
  `request_device` (every prior harness used `DeviceDescriptor::default()`, which has
  zero GPU-timing capability). The bracket is **two submits, not one**:
  `submit([start_ts])` → caller's work (which only *enqueues* into wgpu's
  pending-writes staging belt, not submits) → `submit([end_ts, resolve, copy])`.
  This is not a stylistic choice: it was verified directly against the vendored
  `wgpu-core-30.0.0` source (`device/queue.rs`, `Queue::submit_pending_submission`)
  that `executions.insert(0, pending_execution)` **always** splices the queue's pending
  writes at position 0 of a submission's command-buffer list — a single-submit bracket
  (`submit([start, end])` with the payload write in between) would run the copy
  *before both timestamps*, reading ~0 regardless of payload size. The two-submit form
  is the only ordering wgpu 30 actually guarantees. Numbers from this harness are
  amortized-per-cell GPU-timeline costs (256-repeat amplification to rise above a
  15–70 µs single-submission noise floor on this host), not isolated
  microarchitectural copy-engine numbers — every table below states this explicitly
  where the harness is used.
- **Legacy-model simulation (T4):** the `sync_scene` (`renderer.rs:686-817`) comparator
  is deliberately built as a **lower bound** on real legacy cost — full per-frame
  snapshot-Vec allocation + one `write_buffer` per cell's full capacity, but with
  `serde_json` component instancing, light object destroy/recreate, and the picker-BVH
  rebuild all omitted (the real `sync_scene` pays all three on top of what's simulated
  here). Every delta-sync-vs-legacy number in this report is therefore conservative in
  the direction that favors legacy, i.e. real-world speedups are ≥ what's recorded.
- **Every number in this report has an exact command line recorded in its source task
  report** (`.superpowers/sdd/perfval-task-{1..7}-report.md`); this report cites the
  commands compactly but the byte-exact invocation is always retrievable from the
  underlying artifact.

### 1.3 The campaign's three methodology laws

**(a) Fixture-vacuity is the recurring failure mode of aligned-column tests; self-verifying guards are the house pattern.**
Discovered outside this campaign proper (M3-α Task 8, three rejection rounds): tests
built over aligned GPU columns (generation shadows, harvest gens) kept passing while
testing nothing, because their fixtures used only fresh handles (`gen == 1`
everywhere), so mutants that skipped a column entirely, off-by-one'd a remap range, or
hard-coded a constant all survived. The fix that stuck was **self-verifying guards
baked into the fixture itself** — assert the fixture's own non-identity property
(`col0 != identity`, at least one local-vs-`col0` divergence, gen-uniqueness after a
free/realloc churn) *before* trusting the test's main assertion. This campaign applied
the same discipline throughout: T2's armed-proof (inject a probe allocation, confirm
the gate trips, revert), T4's four in-bench honesty asserts (zero-mutation-zero-bytes,
legacy-bytes-fixed, M=100%-padding-bound, monotonic-in-M), T6's exact-ratio
self-checks on every DEI straddle fixture before the branch assertion. Every gate this
campaign shipped was proven capable of failing, not merely observed to pass.

**(b) The ±20% cross-binary layout-noise calibration band (±85% sub-2µs).**
T5's ECS head-to-head (`pulsar_ecs::World` vs `pulsar_scenedb`'s ECS-compat `World`)
discovered, after an initial confabulated-mechanism defect was caught in review, that
`world.rs`/`component.rs`/`query.rs` in `pulsar_scenedb` are **formatting-only copies**
of `pulsar_ecs`'s (seeded at M1a) — same `CID_CACHE` thread-local, same `TypeId` linear
scan, zero references to the reflection registry on the insert/query path. The
head-to-head therefore compared **identical source compiled into two separate crates**.
Every delta recorded — flagged "regressions" and "wins" alike — is a
compilation-context artifact (codegen/inlining/link-layout differences between the two
crate contexts), not an architectural difference, because there is no code difference
to have one. Measured magnitude of this artifact on this host/toolchain: **up to ~±20%
at 10k–500k-scale/tens-of-µs-to-ms operations**, and **up to ~±85% at sub-2µs operation
sizes** (control-group divergence between two bench files measuring the same op in the
same crate, same session). **This is now a first-class calibration bound for this
codebase**: a criterion delta under ~20% between two binaries on this host is not, by
itself, evidence of an architectural difference without deeper isolation (a code diff
showing an actual implementation divergence, not just a benchmark number).

**(c) Brackets and gates must be provably capable of failing (T3's dead-bracket lesson).**
T3's first working GPU-timing harness shipped a monotonicity self-check
(`assert!(m1024 ≤ m1 + slack)` etc.) whose slack was derived from the harness's own
measured noise floor. Review found the gate **could not fail in either of the two
failure modes it existed to catch**: a broken single-submit bracket reading ~0 flat
(floor ≈ 0 → slack ≈ 150 ns → trivially passes) and the actual noisy/inverted regime
the implementer hit mid-development (15–70 µs noise, N=0 mean *higher* than N=1024's →
floor-derived slack scales with the very noise it should be catching, so any inversion
passes). The numbers were honest in that specific run, but the assert was decorative,
not load-bearing. General law adopted for the rest of the campaign: every gate needs an
explicit demonstration that a wrong implementation would make it fail — either by
literal mutation-injection-and-revert (T1's seam-identity check, T2's `Vec::new()`
probe in three gate shapes, T4's four honesty-assert mutations, T6's three storm
mutations, all independently reproduced by review) or by an argument from the numbers'
physical impossibility under the wrong reading (T3's bandwidth-sanity table: the
raw-256-cell reading of its own numbers implies 2.9–3.7 TB/s, physically impossible on
this hardware, which is what pins the per-cell-amortized reading as the only honest
one).

---

## 2. The 50-claim verdict table

Verdict legend: **MET** (number + margin) · **MET-QUALITATIVE** (qualitative claim,
structurally/indirectly supported, no dedicated quantitative bench built) ·
**MET-WITH-NUANCE** (met, but a caveat changes how it should be read) · **REVISED-MET**
(met, and the campaign's re-measurement replaced a stale/wrong prior number) ·
**REFRAMED** (the contract's language, not the implementation, is what's wrong — stated
correction given) · **MISS** (gap, with the remediation item that closes it) ·
**DEFERRED** (needs M3-β/γ or M4; exactly which milestone is named).

### 2.1 The motivating bet (legacy elimination)

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 1 | Delta-sync replaces per-frame copy-and-reconcile | **MET** | T4 centerpiece, per scale (final-review correction — the original blanket "5.2–5.9× at all three scales" overstated the small-scale regime): at **10k/100k rows, 5.2–5.9× CPU** speedup at realistic partial-mutation rates (0.1–1%) (5.2–6.6× across the campaign's verified re-runs); at **1k rows, 1.07–1.57×** — absolute frame costs there are single-digit µs and the fixed boundary overhead dominates, a different regime where delta still wins but modestly. **100–1000× fewer bytes** moved at every scale. Conservative (legacy modeled as a lower bound — see §1.2). Loses only at the degenerate M=100% (whole-scene-every-frame) case: by 25–30% at 10k/100k (0.69–0.84×), and by ~2.7× at the 1k scale (0.37× — same fixed-overhead regime); the loss is real per-row bookkeeping economics (~13–15 ns/row scenedb vs ~10–11 ns/row legacy), not an artifact — see §3.1. |
| 2 | Zero-mutation frame uploads nothing | **MET** | `(ranges, bytes) == (0, 0)` asserted in-bench (T4) at every S, and independently as a dedicated zero-alloc test (T2 `alloc_gate_gpu`: 0 heap allocations too, not just 0 bytes). |
| 3 | Minimal coalesced ranges | **MET** | Contiguous dirty runs coalesce to exactly 1 range regardless of run length (T2, T4); scattered dirty rows produce exactly one range per maximal contiguous run — proven at both the allocation-count level (T2: 16 ranges → 64 allocs, 64 ranges → 259 allocs) and the whole-frame level (T4: 100 scattered rows @ 10k/1% → 100 ranges, same 6,400 B as the contiguous case). No gap threshold exists in `sync_region`'s coalescing (strict adjacency) — recorded as a real property, not a bug. |
| 4 | Generation writes are delta-minimal | **MET** | Pre-existing M2a generation-write-count gate, unchanged; green in every campaign-task matrix run (85/26/… gpu suites), not independently re-measured this campaign beyond regression-checking. |
| 5 | CPU out of the GPU inner loop | **DEFERRED — M3-β** | No renderer consumes harvest output yet; unfalsifiable until M3-β passes exist. T3's timestamp harness is ready to measure it once they do. |
| 6 | Test 13 — teardown costs zero scene re-upload | **DEFERRED — M3-β** | Asset write-counter instrumentation landed in M3-α (11/11 write sites); the full Test-13 assertion set needs a real teardown/rebuild consumer, which is M3-β scope. |
| 7 | Test 14 — device-loss rebuild, byte-identical | **MET** | Pre-existing (M2a + M2b-α extension), unchanged, green throughout the campaign's regression matrix. |

### 2.2 Storage & cache architecture

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 8 | SoA beats AoS for spatial scans | **MET-QUALITATIVE** | No dedicated AoS-comparison bench was built this campaign (out of scope for T1–T7's task list). T7's scan-scaling sweep corroborates the underlying mechanism: both kernels touch exactly 24 B/row (six `f32` bounds columns), achieving 21–44 GB/s well under any bandwidth ceiling at every N tested — consistent with "zero wasted bandwidth," but the explicit SoA-vs-AoS delta itself remains unmeasured. |
| 9 | 64-byte column alignment | **MET** | Structural layout asserts, unchanged, M1 unit tests green throughout. |
| 10 | Page capacity 256 default / 1024 ceiling; L2-fit inequality | **MET-WITH-NUANCE** | The 1024 ceiling is enforced and tested (MET). The L2-fit performance claim behind the worked examples (64 B × 256 = 16 KB, etc.) has **no cache-sweep bench** — not built this campaign either; T7's cache-tier analysis (§3.3) answers a related but different question (scan throughput is flat across L1→L3, i.e. compute-bound not cache-bound at the *scales tested*, which doesn't validate the page-capacity worked examples directly). |
| 11 | 128 B/element combined stride guardrail | **MET-WITH-NUANCE** | Guardrail enforced as a hard runtime `Result` (tested); the cache-locality payoff behind the rationale ("Cache locality is a correctness property for performance SLAs") has never been benchmarked via a stride-sweep, same gap as #10, not closed this campaign. |
| 12 | Handle dereference = two O(1) array reads | **MET-QUALITATIVE** | Structural; no dedicated micro-bench built (contract itself calls this low-value/textbook). |
| 13 | 64-bit packed handles over pointers | **MET-QUALITATIVE** | Same as #12 — qualitative, low-value, not benched. |
| 14 | Deferred compaction removes per-access read–write synchronization | **MET** | Test 1's throughput clause is the proxy (unchanged); T6's four storms additionally exercise concurrent contention paths (lease mask CAS under 4-thread load) without incident. |
| 15 | Sentinel-preserving token output | **MET** | Structural tests unchanged and green; note this is about positional-alignment preservation for GPU lockstep addressing specifically, distinct from #22's bandwidth framing (see #22). |

### 2.3 Query hot path & no-alloc rules

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 16 | No allocation during queries | **MET** | T2: `query_aabb_in`/`query_frustum_in` — 0 heap allocs, verified **cold** (warm-up removed entirely in review, still 0) as well as steady-state. Armed-proof: injected `Vec::with_capacity(4)` tripped the gate (`left: 1, right: 0`), reverted clean. |
| 17 | Zero heap alloc across the whole frame pipeline after warm-up | **MET-WITH-NUANCE** | T2: harvest warm path (plain + DEI branches, incl. gens columns) — 0 allocs; store boundary sync at zero dirty rows — 0 allocs; store boundary sync — **allocation count independent of dirty-row count within one coalesced range** (measured constant: 4 allocations/boundary for a contiguous run, wgpu-side `write_buffer` bookkeeping, not SceneDB's own heap — SceneDB itself holds no scratch `Vec` anywhere in the sync path). **The nuance that must travel with this claim:** for *scattered* dirty patterns, allocations scale ∝ range count (~4/range: 16 ranges → 64 allocs, 64 ranges → 259 allocs) — the committed gate's own doc says "N contiguous dirty rows" and is honest about this scope, but the plan/contract phrasing ("independent of N") is only true per-range, not per-mutated-row, when ranges don't coalesce to one. |
| 18 | Scratchpad decay policy (50%/8 frames → halve) | **MET** | Unit-tested (M1b), unchanged. |
| 19 | SIMD scan throughput ("16 entities/cycle" aspiration; binding form = bench-validated) | **REVISED-MET** | T1 corrected a mislabeled bench (`scalar_aabb_scan_*` had been measuring the *dispatched* path both times — the old ~1.25× "unimpressive" delta was two invocations of identical code, not a real scalar/AVX2 comparison). True scalar arm, measured for the first time: **AABB 1.86–1.95× faster dispatched-vs-scalar @1024 rows**, **frustum 3.37–3.95×** (bands span implementer + reviewer reproductions per the source hierarchy; final-review correction — the original band quietly omitted the review's own 1.86×/3.95× endpoints). T7 extends this to N∈{1k,16k,256k,1M}: ratios stay **1.66–1.95× (AABB)** and **3.09–3.40× (frustum)** across four orders of magnitude, narrowing only modestly with N (constant per-cell/per-batch overhead amortizing). T7's review independently measured and bounded the harness's one confound (per-cell liveness-`Vec` allocation, symmetric across arms) at 3–5% of the dispatched arm — de-confounded ratios **2.01× / 3.44×** at N=1024, i.e. true kernel deltas are slightly *higher* than reported, not lower. The "16 entities/cycle" AVX-512 aspiration is **REFUTED as a literal target** (measured ~0.055 (frustum-scalar) to 0.36 (AABB-dispatched) entities/cycle at 5.2 GHz assumption, 45–292× short — the 292× endpoint corresponds to the 0.055 frustum-scalar floor) but the contract itself already demoted this to "bench-validated, not assumed from instruction width" — so this is confirmation of an already-demoted aspiration, not a new failure. Root cause identified via instruction-count roofline (§3.3): AABB needs ~18 vector ops/8-lane batch, frustum ~90–91 (≈5×, not the ~7.6× a report typo briefly implied and review corrected) — the kernels are compute/overhead-bound at every cache tier (L1→L3), never memory-bound, at every N tested (up to 1M rows / 24 MB, still inside the 9800X3D's 96 MB L3). |
| 20 | SIMD bit-identity (scalar/AVX2/NEON) | **MET** | Pre-existing property tests unchanged; additionally re-pinned by new campaign tests (T2's seam==dispatched test, T6's DEI independent-oracle bit-identity, T7's per-N-tier scalar==dispatched cross-check) — all green. |
| 21 | Multi-view queries scale by threading | **DEFERRED — M3-β** | Test 1 covers thread-safety; no scaling bench across concurrent views was built this campaign — not one of T1–T7's task list. Multi-view consumers arrive with M3-β's passes; measure there. |

### 2.4 Harvest, DEI, partition

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 22 | DEI < 25% ⇒ dense compaction bounds sentinel bandwidth; Test 12 no-sentinel-bandwidth proof | **REFRAMED** | T6 storm 4 (24%/25%/26% straddle, review-confirmed): branch selection is exact and strict (`<` not `≤`, proven at exactly 25.0%) — that part of the claim stands. **But the contract's framing is inverted for what's actually implemented**: BOTH branches are already sentinel-free before upload (plain path filters `!= NULL_ROW`; DEI path compress-stores to the same dense shape plus a remap segment) — "no sentinel bandwidth in the VRAM payload" holds **unconditionally**, it is not what the 25% gate decides. What the gate actually selects, measured: at 24% hit rate the DEI branch uploads **1920 B** (960 dense + 960 remap) vs the plain path's **1040 B** at 26% — **DEI uploads MORE bytes, not fewer**, at its own operating point. The 25% threshold selects a *compute strategy* (SIMD compress-store + row-identity remap vs. a scalar filter loop), not a bandwidth saving. Both branches do save bandwidth relative to a hypothetical positional `len×4` upload carrying embedded `0xFFFF_FFFF` sentinels (the contract's implied baseline, which neither branch actually produces): 2080 B saved at 24%, 2960 B at 26% — but that comparison is against a shape that doesn't exist in this codebase, not a live A/B. **Corrected language for Rev 2.4** (routed via R-PERF-3): "Sentinel-free upload holds unconditionally on both branches; the 25% threshold selects compaction strategy (SIMD-compress-vs-scalar-filter) and remap-table availability, not sentinel-bandwidth elimination." |
| 23 | Single-scan partition | **MET** | Pre-existing `harvest_partition_1024` bench, unchanged; freshly re-measured this campaign at **1.147 µs** [1.143, 1.151] (100 samples, default sampling) — the first recorded number for this bench (see #50). |
| 24 | AVX2 compress-store DEI arm bit-identical + faster | **MET-WITH-NUANCE** | Bit-identity solid (pre-existing property tests + T6's independent oracle). Speed-vs-scalar was never separately benched this campaign either — `dei_compact_1024_sparse` (freshly measured at **731.8 ns** [729.2, 734.5], see #50) measures the DEI branch's own cost, not a scalar-vs-AVX2 delta within DEI specifically. |
| 25 | Expected-generation column costs +4 B/valid-token/view | **MET** | Structural, landed M3-α, byte accounting confirmed via harvest tests; unchanged this campaign. |
| 26 | No harvest pins (D1) — frame-scoped tokens avoid compaction starvation | **MET-WITH-NUANCE** | M2a Test 6 remains the gate and is unchanged/green; the contract's own gap note ("no explicit ratchet test") was not closed this campaign — `rows_in_use` non-ratcheting is still proven only by Test 6's long-run churn shape, not a dedicated assertion on the ratio itself. |

### 2.5 Streaming grid, hysteresis, residency

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 27 | Hysteresis prevents thrash cost (10% pad default; zero transitions under sub-band jitter) | **MET** | T6 storm 1, using `grid.rs`'s own Test-11 fixture (padded promote 330, demote floor 310, band [310,330), unpadded 340): **600 simulated in-band frames → 0/600 domain transitions** (drained every frame, not sampled). Non-vacuity proven two ways: a probe 1 unit below the demote floor (309) demotes in exactly 1 step (310 is a live, nearby, crossable edge — not a number the jitter is vacuously far from), and a widened phase (amplitude 50 > 20-unit band) produces 20 promotions + 20 demotions across 40 frames. Review confirmed via fixture mutation (`hysteresis: 20.0 → 0.0` tripped the gate immediately). |
| 28 | Promotion/demotion = O(1) region alloc/free | **MET** | T1: `promotion_demotion_cycle` bench, honest (post-pump-fix) at **4.23–4.46 µs**, flat regardless of iteration count (86k+ iterations completed panic-free — a real promote→demote→drain cycle every time, not saturating any bound). T6 storm 2 confirms O(1) recycling holds under 500-cycle churn at a tighter residency budget (64 cells / 16 slots). |
| 29 | Region recycling avoids realloc; VRAM closed-form budget; recycled tail scrubbed | **MET** | T6 storm 2: wgpu SSBO byte sizes (transform 64 KiB, generation 4.5 KiB, slot-mirror 4 KiB, instance-info 8 KiB) **bit-for-bit unchanged** from construction across 500 promote/demote cycles — zero buffer reallocations. Region-pool footprint (distinct physical bases touched) **plateaus at exactly 16** (the residency budget) from cycle 50 through cycle 500 — proving genuine reuse, not merely a bounded structure. Post-storm VRAM readback of all 64 rows (incl. both boundary rows) across all 16 resident cells: **byte-exact**, zero cross-tenant corruption in the post-storm state (see nuance below — "during the storm" is not directly observable, only the post-storm result is checked). Latency distribution recorded: min 14.3 µs / mean 45.2 µs / p50 15.9 µs / **p95 58.5 µs** / p99 271.5 µs / max 8.43 ms (single scheduler-jitter outlier, debug-build wgpu queue submission on a loaded host — no budget asserted against this distribution, recorded only). |
| 30 | Streaming budget inequalities validated at startup | **MET** | Structural constructor-level pass/fail, unchanged this campaign; not independently re-measured beyond regression-checking. |
| 31 | Every cell ≥1 draw contribution per frame; cross-fade stipple zero overdraw | **DEFERRED — M3-γ** | Tests 7/8 absent; needs draw-count/pixel-level GPU instrumentation from a real renderer. |
| 32 | Lease pool 64 slots; 2.0 ms revocation timeout bounds worst-case compaction stall | **MISS** (undeliverable by current primitives) — the timeout-semantics sub-claim is **REFRAMED** | T6 storm 3 (1000 revocations under 4-thread concurrent `LeaseMask` contention): revocation's trigger semantics are proven correct (overdue → revoked, fresh → not) and its own execution cost measured at **p99 = 100 ns, mean 44.2 ns** — four orders of magnitude under 2.0 ms. **But review found the contract's actual promise — "bounds worst-case compaction stall from a stuck reader" — is undeliverable by primitives that exist in this crate today**: `revoke_overdue` deliberately does not release the lease slot; frame-boundary compaction is gated on `any_held() == false`; §9.2.1's "compaction proceeds immediately via the pinned-snapshot bypass" path is unbuilt; nothing in production consumes `any_held()` yet (M4 World-driver scope). So a revoked-but-not-dropped lease still blocks an `any_held()`-gated compaction **indefinitely**, on either reading of the contract. Separately, the 2.0 ms figure itself is a **hold-duration timeout** (how long a holder may keep a lease into the isolation phase before revocation triggers), not a latency budget on any operation — the storm's headline "PASS vs 2.0 ms" assert is honestly labeled ("synchronous flag-set under contention") but cannot fail in any way that would signal a real regression (only multi-millisecond OS preemption between two `Instant::now()` calls could trip it), so it is not a meaningful gate against the contract as literally stated. Routed as R-PERF-2 (build the bypass + a production `any_held()` consumer) and R-PERF-3 (correct the Rev 2.4 language). |

### 2.6 Concurrency & frame structure

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 33 | Phase separation near-free: Test 1 ≤5% overhead / 60 s no-deadlock | **MET-QUALITATIVE** | Pre-existing Test 1, unchanged and green throughout; the infra recon's own gap note ("5% clauses may be asserted structurally, not numerically — audit test bodies") was **not** independently re-audited this campaign — none of T1–T7 read Test 1's assertion body to confirm the 5% figure is a live numeric compare rather than a race-freedom-only check. Carried as an open item, not closed. |
| 34 | Timeline-serial retirement survives stutter (Test 6, 10 FPS bursts) | **MET** | Pre-existing, unchanged, green. |
| 35 | Retirement ordering (gen written before slot reissued; u32::MAX permanent retirement) | **MET** | Pre-existing invariant tests, unchanged. |
| 36 | Compile-time phase machine (misuse = compile error) | **MET** | `trybuild`/`compile_fail` gates green, unchanged. |

### 2.7 GPU-side layout & cull contracts (mostly M3-β+)

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 37 | Byte-exact host↔shader layouts | **MET** | Test 3 (`gpu_layout`) in CI on every PR, naga reflection, no adapter needed; M3-α additions (MeshMetadata, MaterialRow R8, InstanceInfo, etc.) all landed and green throughout the campaign. |
| 38 | Meshlet limits fill a GPU wavefront (≤64 v / ≤124 t) | **DEFERRED — M3-β+** | Host-side enforcement absent (packed u8 makes ≤255 vacuous, flagged since M3-α); the wavefront-fill claim itself needs real GPU profiling. |
| 39 | VG error threshold (1.0 px default, Test 8 tolerance) | **DEFERRED — M3-γ** | Test 8 absent. |
| 40 | Backface cone culling eliminates a significant meshlet fraction | **DEFERRED — M3-γ** | No consumer exists to measure survival counters against. |
| 41 | Hi-Z mip-boundary dual-sampling, triggered only within 5% of a mip transition | **DEFERRED — M3-β** | No Hi-Z pass exists yet. |
| 42 | Near-plane pre-test keeps the W≤0 bypass population small | **DEFERRED — M3-β** | No cull pass exists yet. |
| 43 | Bounded atomicAdd command allocation; Test 5 clamp behavior | **DEFERRED — M3-β** | Test 5 headless run planned, not built; counter-readback-latency-vs-conservative-max-count open item (M3 §9a) untouched. |
| 44 | Per-view command buffers over view tags (concurrent dispatch) | **DEFERRED — M3-β** | No multi-view dispatch exists yet. |
| 45 | Traditional-first draw ordering improves VG cull efficiency | **DEFERRED — M3-γ** | Needs culled-meshlet counters under both orderings; no consumer exists. |
| 46 | GPU-vs-CPU cull equality (exact visible-set match) | **DEFERRED — M3-β** | Planned, not built. |
| 47 | Seam bind-group budget: `SceneDbBinding`'s storage-buffer count fits the default per-stage limit (8) | **MISS** | The seam now uses **9** read-only storage buffers (8 scene + `MaterialRegistry`'s row added at M3-α Task 11), exceeding the WebGPU default per-stage limit of 8 — flagged and documented on the type since M3-α Task 9/11 as a **hard M3-β requirement** (raise device limits or split the bind group). Not fixed this campaign (out of scope for perf-val); registered in design §12 and carried forward as R-PERF-4. |
| 48 | Texture store bindless ceiling 16384 slots | **MET** | Structural, landed M3-α, unchanged. |
| 49 | Tombstone headroom default 64/slot region | **MET** | Structural, region-pool tests unchanged. |
| 50 | Existing bench baselines to defend (regression floor) | **REVISED-MET** | Every baseline this contract row names is now recorded, several corrected from stale/mislabeled figures. See §3.4 for the full table — headline corrections: `scalar_aabb_scan_1024` was **762 ns** in the pre-campaign contract doc (itself the AVX2/dispatched path, mislabeled) → now genuinely **1.120 µs** (T1) / **1.09 µs** (T7 cross-check, same bench). `dispatched_aabb_scan_1024` 609 ns → **579–598 ns** (T1) confirmed. `frustum_scan_1024` 1.22 µs → **1.19–1.28 µs** confirmed as the dispatched path (a true `scalar_frustum_scan_1024` now also exists: **4.70–4.75 µs**). The five previously-**unbaselined** benches all now have recorded numbers (three of the five — `alloc_free_compact_256`, `harvest_partition_1024`, `dei_compact_1024_sparse` — were still unrecorded at the end of T7 and were spot-run for this report; see §3.4 for the honest disclosure and exact numbers). |

### 2.8 Tally

| Verdict | Count |
|---|---|
| MET (incl. MET-QUALITATIVE, REVISED-MET) | 29 |
| MET-WITH-NUANCE | 5 |
| REFRAMED | 1 (#22; #32 carries a REFRAMED sub-aspect too, see below) |
| MISS | 2 (#32, #47) |
| DEFERRED | 13 (#5, #6, #21, #31, #38–46) |
| **Total** | **50** |

Note on #32: it is presented above under MISS because its most consequential aspect —
the contract's "bounds worst-case compaction stall" promise — is undeliverable by
current primitives; it *also* carries a REFRAMED aspect (the 2.0 ms figure is a
hold-duration timeout, not a latency budget). Both aspects are stated in full in §2.5's
row and are not double-counted in the tally.

---

## 3. The matrices

### 3.1 T4 — delta-sync vs. legacy full-resync, 15-cell head-to-head

CPU wall time (µs, mean/p95, 50 timed frames after 10 warm-up per cell), `SyncStats`
bytes, and coalesced-range count. Both paths pumped (`submit(empty)+poll`) every
iteration; legacy path rebuilds a fresh full-capacity snapshot `Vec` and re-uploads the
whole region every frame regardless of mutation (lower-bound-faithful simulation of
`sync_scene`, see §1.2). Corrected at the confirmation run (`ab5085cd`, review-verified)
after an initial report-integrity defect (a `ranges` cell that the deterministic code
cannot produce, plus a confabulated footnote) was caught and fixed — the table below is
the corrected, reviewer-confirmed state.

```
S       M%      scenedb_cpu_us(mean/p95)   legacy_cpu_us(mean/p95)   speedup   scenedb_bytes  legacy_bytes  byte_ratio  ranges
1000    0       1.68/1.70                  5.80/12.80                3.46x     0              65536         inf         0
1000    0.1     3.70/4.50                  5.80/12.80                1.57x     64             65536         1024.00x    1
1000    1       5.39/12.70                 5.80/12.80                1.07x     640            65536         102.40x     1
1000    10      5.70/12.90                 5.80/12.80                1.02x     6400           65536         10.24x      1
1000    100     15.73/23.50                5.80/12.80                0.37x     64000          65536         1.02x       1

10000   0       15.81/15.90                108.55/140.90             6.86x     0              655360        inf         0
10000   0.1     18.51/22.50                108.55/140.90             5.87x     640            655360        1024.00x    1
10000   1       19.04/19.10                108.55/140.90             5.70x     6400           655360        102.40x     1
10000   10      31.57/40.60                108.55/140.90             3.44x     64000          655360        10.24x      1   <- corrected: was reported as 10 with a confabulated footnote; code deterministically produces 1 (one global mutation prefix fits inside cell 0)
10000   100     142.17/154.30              108.55/140.90             0.76x     640000         655360        1.02x       10

100000  0       158.53/161.70              1016.99/1299.70           6.42x     0              6422528       inf         0
100000  0.1     172.20/193.30              1016.99/1299.70           5.91x     6400           6422528       1003.52x    1
100000  1       183.07/188.50              1016.99/1299.70           5.56x     64000          6422528       100.35x     1
100000  10      290.25/307.40              1016.99/1299.70           3.50x     640000         6422528       10.04x      10
100000  100     1467.40/2015.70            1016.99/1299.70           0.69x     6400000        6422528       1.00x       98
```

Reviewer's independent reproduction (`ab5085cd`, full serialized run) confirmed the
same shape: 10k speedups **7.55/6.55/6.23/3.73/0.84×** at M=0/0.1/1/10/100%, byte ratios
identical, 100k/M=1% at 5.24×/100.35×. Crossover is stable at exactly M=100% for
S=10,000 and S=100,000 across repeats; **S=1,000's crossover point is noise-sensitive**
(observed at M=100% in the original run, at M=10% in the reviewer's confirmation run —
disclosed in the source report as a known instability of single-digit-microsecond
magnitudes, not cited as a stable finding).

**Scattered vs. contiguous (S=10,000, M=1%, same 6,400 B mutated either way):**

| Pattern | ranges | CPU mean | CPU p95 |
|---|---:|---:|---:|
| Contiguous (100-row prefix) | 1 | 19.04 µs | 19.10 µs |
| Scattered (100 rows, stride 100) | 100 | 116.75 µs | 225.10 µs |

**5–7× CPU cost at equal byte volume**, purely from range-count (confirmed at the
allocation level by T2's independent finding: allocs ∝ range count, ~4/range,
wgpu-side). This is the data behind R-PERF-1.

**GPU-ns pair at 10k, M=1% (T3's `GpuTimer`, 32-repeat amortized, directional only):**

| Path | mean ns | p95 ns |
|---|---:|---:|
| delta | 19,942.9 | 23,383.0 |
| full_upload | 42,139.0 | 46,070.0 |

Delta comes in at roughly half the GPU-timeline nanoseconds of the full-upload shape at
this scale. **Caveat inherited directly from T3:** at these payload sizes (delta
≈205 KB / full ≈21 MB across 32 reps) the two-submit bracket's fixed driver/kernel
round-trip overhead (tens of µs) is the same order of magnitude as the numbers
themselves — treat this pair as **directional color**, not precision measurement; §3.1's
CPU-side matrix is the load-bearing evidence.

### 3.2 T6 — stress-storm distributions

**Storm 1 (hysteresis, contract #27):** 600 in-band frames → **0/600** transitions
(exact, drained every frame). Out-of-band phase (40 frames, amplitude 50 > 20-unit
band) → **20 promoted / 20 demoted** (non-vacuity proof).

**Storm 2 (eviction/recycle):** 500 promote/demote cycles, 64 logical cells / 16-slot
residency budget.

| Metric | Value |
|---|---|
| Distinct region bases touched (cycle 50 → cycle 500) | 16 → 16 (plateau, == budget) |
| `register_cell` failures | 0 / 500 |
| VRAM spot-check (post-storm, 1024 rows across 16 resident cells) | byte-exact |
| SSBO sizes (post-storm vs. construction) | unchanged (transform 64 KiB / gen 4.5 KiB / mirror 4 KiB / info 8 KiB) |
| Promote+demote latency (ns) | min 14,300 · mean 45,169 · p50 15,900 · **p95 58,500** · p99 271,500 · max 8,431,700 (single debug-build scheduler-jitter outlier) |

**Storm 3 (lease revocation, contract #32/C4):** 1000 revocations under 4-thread
concurrent `LeaseMask` contention.

| Metric | Value |
|---|---|
| min / mean / p50 / p95 / p99 / max (ns) | 0 / 44.2 / 0 / 100 / **100** / 1,000 |
| p99 vs. 2.0 ms (2,000,000 ns) budget | 0.0001× — four orders of magnitude of headroom |

Measures the synchronous `revoke_overdue` flag-set call itself (the only quantity this
crate's advisory-flag API can honestly expose) — see §2.5 #32 for why this is not a
meaningful gate against the contract's actual "compaction stall bound" promise.

**Storm 4 (DEI straddle, contract #22):** 1000-row cell, three hit-rate fixtures.

| Hit rate | Branch | dense | remap | total |
|---:|---|---:|---:|---:|
| 24.0% | DEI (`dei_compacted_runs=1`) | 960 B | 960 B | 1,920 B |
| 25.0% (exact) | plain (`dei_compacted_runs=0`) — proves strict `<` | — | — | 1,000 B (`len × 4`, positional, all this fixture's rows) |
| 26.0% | plain (`dei_compacted_runs=0`) | 1,040 B | 0 B | 1,040 B |

Byte deltas: (a) DEI's own dense-vs-remap overhead = 960 B; (b) DEI-total(24%) vs.
plain-total(26%) = 880 B, **DEI costs more**; (c) hypothetical fully-positional/lockstep
upload (`len×4` = 4,000 B, a shape neither branch actually produces) = the contract's
implied baseline, included for context only.

### 3.3 T7 — query-scan scaling study

ns/row and scalar-vs-dispatched ratio, N ∈ {1,024 / 16,384 / 256,000 / 1,000,448} rows
(K cells of 1,024 rows each, K ∈ {1, 16, 250, 977} — `SpatialCell` hard-caps at 1,024
rows/cell):

| N (rows) | AABB scalar ns/row | AABB dispatched ns/row | **ratio** | Frustum scalar ns/row | Frustum dispatched ns/row | **ratio** |
|---:|---:|---:|---:|---:|---:|---:|
| 1,024 | 1.0587 | 0.5417 | **1.954×** | 3.5091 | 1.0417 | **3.369×** |
| 16,384 | 1.0729 | 0.5845 | **1.836×** | 3.5522 | 1.0438 | **3.403×** |
| 256,000 | 1.0951 | 0.6590 | **1.662×** | 3.5780 | 1.1570 | **3.092×** |
| 1,000,448 | 1.1013 | 0.6132 | **1.796×** | 3.5934 | 1.1572 | **3.105×** |

Review's de-confounding (measured the shared per-cell liveness-`Vec` allocation
directly, ≈32 ns/call, 3–5% of the dispatched arm — well under the 20% materiality
bar): de-confounded N=1024 ratios are **AABB 2.01×** / **frustum 3.44×** (both slightly
*higher* than the raw ratios above, as predicted by the "shared additive constant
compresses ratios toward 1×" argument).

**Cache-tier fit** (24 B/row read for both kernels — six `f32` bounds columns):

| N | Total footprint | Cache tier | AABB dispatched GB/s | Frustum dispatched GB/s |
|---:|---|---|---:|---:|
| 1,024 | 24 KB | L1 (48 KB/core) | 44.3 | 23.0 |
| 16,384 | 384 KB | L2 (1 MB/core) | 41.1 | 23.0 |
| 256,000 | 6.14 MB | L3 (96 MB) | 36.4 | 20.7 |
| 1,000,448 | 24.0 MB | L3 (still 25% of 96 MB) | 39.1 | 20.7 |

Every tier tested is cache-resident (never reaches DRAM, even at 1M rows); ns/row is
flat within ~10–20% across four orders of magnitude — the scan is compute/overhead-bound
at every scale tested, not cache- or memory-bandwidth-bound. Entities/cycle (5.2 GHz
assumption, stated not measured): AABB scalar 0.182 / dispatched 0.355; frustum scalar
0.055 / dispatched 0.185 — all 45–292× short of the spec's "16/cycle" AVX-512
aspiration, which the contract itself already demoted to a validated-by-benchmark
figure, not a literal target.

**Instruction-count roofline** (why frustum's delta is bigger than AABB's): AABB AVX2
= 18 vector ops per 8-row batch (6 loads + 6 compares + 5 ANDs + 1 movemask); frustum
AVX2 = ~90–91 (6 loads + 14 ops/plane × 6 planes + movemask) — roughly **5×** the
arithmetic intensity at identical data volume (24 B/row, both kernels), which is what
lets frustum's SIMD arm pull further ahead before the shared fixed overhead reasserts
itself.

### 3.4 T5 — ECS-compat head-to-head (context, not a numbered contract claim)

Not one of the 50 contract items (T5 measures the ECS-compat `World` layer's
engineering quality, tangential to SceneDB's actual architectural bets), but essential
methodology context for §1.3(b): 95 shared criterion IDs across `pulsar_ecs` (legacy)
vs. `pulsar_scenedb`'s ECS-compat `World`, all traced to **identical source** compiled
into two crates. Result: the overwhelming majority of families (component access,
archetype migration/pressure, query tuple width/selectivity, churn) land at **par**
(±0.5–5%, confirmatory — same code, zero compat-layer tax). The movers — `spawn_n`
with 4–8 components at n≥10k (+9–13%, "regression"), empty-spawn at n≥10k (−14–18%,
"win"), `large_scale/query/500000` (−19.6%, reproduced isolated) — are all,
per §1.3(b), cross-binary codegen/layout artifacts, not architecture. **No remediation
items came out of T5.**

### 3.5 T1/T2/T3 harness-repair baselines (feed §2's #19/#50, cited compactly)

| Bench | Old (broken/mislabeled) | New (honest) |
|---|---|---|
| `region_sync_1024_dirty_rows` | 13.08 µs (rode an ever-growing staging pool, never converged at default sampling) | **8.02 µs** (T1) / 9.52 µs (review reproduction) |
| `promotion_demotion_cycle` | never previously recorded | **4.46 µs** (T1) / 4.23 µs (review reproduction) |
| GPU-bench process memory | 17.4–19.1 GB, monotonic growth, non-convergent | ~330–490 MB, bounded, non-monotonic |
| `gpu_timing` (T3, per-cell amortized, 256-repeat) | harness did not exist | delta N=0/1/64/1024: 1,884.7 / 3,090.6 / 3,343.1 / 5,882.8 ns; `full_upload` N=1024: 4,503.6 ns; crossover in (64, 1024]. Post-review-hardening re-run (defect fixes for the vacuous self-check + hoisted CPU alloc): floor 2,064 / delta 3,122 / 3,356 / 6,148 ns, full 4,543 ns — same shape, confirms stability of the trend across the hardening fix. |

---

## 4. Remediation & follow-through register

| # | Item | Data behind it | Owner milestone |
|---|---|---|---|
| **R-PERF-1** | **CLOSED (M3-β T3, measured REJECT — §4.1 below).** Gap-tolerant dirty coalescing experiment for `sync_region` — investigate a bounded gap threshold (instead of strict adjacency) to reduce range-count blowup on scattered mutation patterns. | Scattered dirtiness = 5–7× CPU at equal byte volume (T4: 6.96×/5.82× reproduced; T2: allocs ∝ range count, ~4 allocs + 1 `write_buffer` per range, 16 ranges→64 allocs, 64 ranges→259 allocs). | M3-β (sync path) |
| **R-PERF-2** | Build §9.2.1's pinned-snapshot compaction bypass and wire a production consumer of `any_held()`. | Contract #32's "bounds worst-case compaction stall" is currently undeliverable: `revoke_overdue` doesn't release the lease slot; frame-boundary compaction is gated on `any_held()==false`; nothing in production calls `any_held()` today. | M3-β/M4 |
| **R-PERF-3** | Rev 2.4 spec-language corrections: (a) C4/§9.2.1 wording — state the 2.0 ms figure as a hold-duration timeout, not a latency budget; (b) contract #22 — reframe DEI as a compute-strategy/remap-availability selector, not a sentinel-bandwidth saving (both branches are sentinel-free unconditionally; DEI uploads more bytes at its own operating point); (c) contract #27 — confirmed as written, no change needed. Route to the Rev 2.4 proposal already drafted in the Research repo (`Dev/Research/public/drafts/SceneDB2.0-Rev2.4-PROPOSAL.md`). | T6 storm 3 + review (C4/§9.2.1); T6 storm 4 + review (#22); T6 storm 1 + review (#27, confirmed sound as-is). | Research repo, Rev 2.4 (spec-of-record edit, user-applied) |
| **R-PERF-4** | M3-β hard requirements already registered in design §12: (a) bind-group storage-buffer budget is now 9 vs. the WebGPU default per-stage limit of 8 (contract #47, MISS) — raise device limits or split `SceneDbBinding`; (b) `mesh_index` bounds-check against the mesh table in the cull shader (recycled-tail garbage defense, flagged since M3-α Task 4); plus cull-efficiency measurement via T3's ready-to-reuse GPU timestamp harness once M3-β passes exist (unlocks contract items #5, #31, #38–46). | M3-α Task 9/11 (budget flag), Task 4 review (bounds-check flag), T3 harness (measurement readiness). | M3-β |
| **R-PERF-5** | Bench/documentation upkeep: (a) the perf-val plan's "core 133" line is stale — the featureless matrix has been **136** since T2 added `alloc_gate.rs`'s 3 CPU tests (fully traced, not a regression); (b) `query_single`'s benchmark-group name collides between `ecs_bench.rs` and `ecs_detailed_bench.rs` (same crate, both files) — harmless today (workloads identical) but a latent last-writer-wins footgun if the two ever drift, worth a one-line rename; (c) criterion baseline-file policy — **`target/criterion` is NOT committed** (gitignored in both crates) and this is the intended policy: this report, not a committed baseline directory, is the record of what was measured and when. Any future regression check re-derives its own baseline via `--save-baseline`/`--baseline` at comparison time. | T6 review (133→136); T5 §1a/review defect 4 (collision); this report (policy statement). | Ongoing / next maintenance pass |

### 4.1 R-PERF-1 verdict (M3-β T3) — measured REJECT

A gap threshold `G` was implemented as a compile-time constant
(`gpu::GAP_MERGE_THRESHOLD`) generalizing `SceneBuffer::sync_region`'s
run-detection loop: two dirty runs separated by fewer than `G` clean rows
merge into one upload range (re-uploading the bridging clean rows' bytes,
which is sound — see the constant's doc for the full correctness argument;
summary: the CPU column is the sole source of truth for every buffer
`sync_region` touches, nothing GPU-side writes back to them, and the
generation buffer never routes through `sync_region` at all, so a clean
row's dirty bit being false already means VRAM == current CPU bytes for that
row — re-uploading it is a byte-identical no-op). `G = 0` reproduces the
pre-existing strict-adjacency behavior exactly, now pinned by
`tests/gpu_store.rs::sync_region_gap_of_one_row_splits_at_g0`.

**Sweep** (`legacy_model_bench`, S=10,000, one process per G, `--test-threads=1`
discipline, host per §1.1):

| G | stride-100 scatter (T4's case, ~99-row gaps) | stride-10 scatter (~9-row gaps) | contiguous (M=1%) regression? |
|---:|---|---|---|
| 0 (today) | ranges=100, bytes=6,400, CPU=118.4µs | ranges=1,000, bytes=64,000, CPU=929.4µs | ranges=1, bytes=6,400, CPU=24.0µs |
| 4 | ranges=100, bytes=6,400, CPU=121.2µs (unchanged: gap 99 and 9 both ≥ 4) | ranges=1,000, bytes=64,000, CPU=950.1µs (unchanged) | ranges=1, bytes=6,400, CPU=24.6µs — no regression |
| 16 | ranges=100, bytes=6,400, CPU=116.8µs (unchanged: gap 99 ≥ 16) | ranges=10, bytes=634,240 (**9.9×**), CPU=68.2µs (**13.6×** faster) | ranges=1, bytes=6,400, CPU=22.5µs — no regression |
| 64 | ranges=100, bytes=6,400, CPU=119.7µs (unchanged: gap 99 ≥ 64) | ranges=10, bytes=634,240 (9.9×), CPU=65.1µs (14.3× faster) | ranges=1, bytes=6,400, CPU=22.5µs — no regression |
| 128 (exploratory, beyond the required set — run to falsify the "accept" hypothesis) | ranges=10, bytes=576,640 (**90×**), CPU=52.1µs (2.3× faster) | ranges=10, bytes=634,240, CPU=66.1µs | n/a (not part of the required sweep) |

The G=128 row's byte count triggered the bench's own honesty assertion
(`contiguous and stride-100 scattered mutate the same TOTAL byte volume at
M=1%`) — it fired exactly as designed: `left: 6400, right: 576640`. That is
not a bug in the bench; it is the gap-merge mechanism doing precisely what
it is supposed to at that G, and the assertion catching that the byte-volume
invariant this campaign's claim #3 (minimal coalesced ranges) depends on no
longer holds once G bridges that large a gap.

**Geometry.** At stride 100 the gaps between dirty rows are ~99 clean rows
— only `G > 99` merges anything there. At stride 10 the gaps are ~9 — `G ≥
10` already collapses the whole pattern. The required sweep set {0, 4, 16,
64} therefore brackets the stride-10 (dense-scatter) transition but never
reaches the stride-100 transition at all.

**Decision: REJECT.** `GAP_MERGE_THRESHOLD` ships at `0` — no behavior
change from today.

1. **The register's actual motivating case (T4: stride-100/1% scatter,
   6.96×/5.82× CPU cost) is untouched by every G in the required sweep.**
   Ranges, bytes, and CPU time at G=4/16/64 are all identical (within noise)
   to G=0 for that pattern. Closing R-PERF-1's own cited evidence requires
   `G > 99`, which was outside the required set but measured anyway (G=128)
   specifically to check whether "accept, just pick a bigger G" was viable.
   It is not: G=128 buys a 2.3× CPU improvement at a **90× byte-volume
   cost** (6,400 B → 576,640 B, ~88% of the full 655,360 B region) — a
   materially worse trade than the "bytes rise slightly" this task's brief
   anticipated, and directly in tension with claims #1 and #3 of this
   report (100–1000× fewer bytes than legacy; minimal coalesced ranges).
2. **The one workload the required sweep DOES help (stride-10 dense
   scatter, 13.6–14.3× CPU win at G≥16) is not the R-PERF-1 case at all**,
   and its cost is not "slight" either: bytes rise 9.9×, i.e. the upload
   becomes a near-total-region reupload (634,240 of 655,360 B) — the same
   order of degradation as the M=100% legacy-crossover row. Accepting a
   nonzero G "scoped to dense-scatter only" is not actually achievable with
   one crate-wide compile-time constant: every `SceneBuffer<T>::sync_region`
   call in the process shares it, so shipping it nonzero silently imposes
   that bandwidth tax on ANY future caller whose dirty pattern happens to
   have small gaps, whether or not that caller wants the CPU/bandwidth
   tradeoff. That is a footgun, not a scoped accept.
3. Net: no G value both (a) closes the case R-PERF-1 was opened for and (b)
   respects this campaign's own byte-efficiency claims. `G = 0` is kept as
   the shipped, tested default; the mechanism itself stays in the code
   (parameterized by the constant, tested at G=0) as a measured, closed
   experiment rather than a live config surface — see
   `.superpowers/sdd/m3b-task-3-report.md` for the full writeup.

The claim #3 note ("No gap threshold exists in `sync_region`'s coalescing
(strict adjacency) — recorded as a real property, not a bug") in §2's
50-claim table stands **unchanged**: it was true before this task and
remains the measured, deliberate choice after it.

---

## 5. CI

**Verified: no gap.** `crates/core/pulsar_scenedb/Cargo.toml` declares six `[[bench]]`
targets — `ecs_bench`, `hotpath_profiler`, `ecs_detailed_bench`, `scenedb_bench`
(unconditional) and `gpu_timing`, `legacy_model_bench` (both `required-features =
["gpu"]`, added by T3/T4 this campaign). The existing CI gate
(`.github/workflows/ci.yml:117-120`, added at M3-α Task 1 specifically to close this
class of blind spot — "the one gpu-gated target the test matrix never builds"):

```
cargo check -p pulsar_scenedb --features gpu --benches
```

`--benches` compiles **every** `[[bench]]` target whose `required-features` are
satisfied by the invocation's feature set; passing `--features gpu` satisfies both
gpu-gated targets *and* still includes the four unconditional ones. This single
existing step therefore already covers `gpu_timing` and `legacy_model_bench` — both
were added to this exact Cargo.toml section by their respective campaign tasks and
confirmed compiling clean under this gate in every task report from T3 onward (T4, T6,
T7 all re-ran this check as part of their own verification tables). **No CI change is
needed; no step was added.**

---

## 6. Commit

Report committed as:
```
docs(scenedb): performance-validation report — spec contract vs measurements (perf-val T8)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

`.superpowers/sdd/progress.md` appended with a one-line campaign-closed VERDICT entry
(see below).
