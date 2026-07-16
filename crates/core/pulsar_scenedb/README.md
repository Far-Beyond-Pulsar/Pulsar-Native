# pulsar_scenedb

SceneDB 2.0 Layer 1 storage core. Spec: `docs/superpowers/specs/SceneDB2.0.md`
(Rev 2.3). Contracts: `docs/superpowers/specs/CONTRACTS.md`. Design:
`docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`.

Conformance baseline: `docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md`
(the cross-milestone spec-conformance audit spanning Stage 0 → M2b-β Task 1;
CONTRACTS.md and the milestone designs above are re-synchronized against it).

Seeded from `pulsar_ecs` (kept as reference). See the crate docs for the
module map.

## M1 — Storage Core

Milestone status: M1 (Layer 1) — handles, paged SoA, liveness, compaction,
scalar spatial query.

## M2a — GPU Layer

The `gpu` feature (default-off for C0 graphics-free core) adds GPU-resident
storage: `EngineGpuContext`, `SceneBuffer<T>` SSBOs with coalescing delta-sync,
`GenerationBuffer`, and `GpuStore` with pin-by-serial retirement and
generation-shadow gating for delta-minimality.

## M2b-α — Multi-Cell Scene, Asset Store, Phase Machine

Three pillars complete the α release:

1. **SceneGpuStore (Region-Partitioned)**: Global buffers partitioned into
   per-cell regions via size-class pools (C2 default 256, hard max 1024 per
   class). Per-cell `CellGpuState` holds dirty masks, pending retires, gen
   shadow, slot shadow. Self-healing slot-mirror boundary scan, `register_cell`
   as promotion primitive (§4.1), `rebuild` for device-loss recovery.

2. **Asset Store**: `GeometryArena` (RangeList suballocation), `MeshRegistry`
   (C5: 72 B metadata, XOR-validated), `ClusterBuffer` (C5: 48 B,
   NaN-rejecting error-monotonicity validation). Both backed by corrupted-VRAM
   rebuild gates.

3. **Phase Machine**: Compile-time frame orchestration — `FrameDriver` → 
   SimulateA→SimulateB→Harvest→Boundary. `BoundaryPhase::retire` returns drain
   count for confirmation. Positive doc-tests and `compile_fail` gates prove
   phase invariants hold in real code.

## M2b-β — Streaming Grid, Harvest Pipeline, DEI Compaction

Complete. Three pillars on top of α:

1. **StreamingGrid**: concentric domains (Outer/Margin/Inner) over a
   coordinate grid, hysteresis-banded promotion/demotion, cross-fade alpha,
   and a VRAM/cell-count budget (`StreamingBudget`). `execute_transitions`
   drains queued transitions at the frame boundary (after `retire`, via the
   `&RetiredPhase` witness): Outer→Margin calls `SceneGpuStore::register_cell`
   (declining gracefully on region exhaustion, §8), Margin→Outer calls
   `unregister_cell` pinned by the eviction serial exactly like M2a's
   pin-by-serial pattern — recycled only once that serial's frame boundary
   drains it. `register_cell`'s recycled-region tail scrub (D2-tail
   carry-forward) zero-fills any prior tenant's residual generations beyond
   the new tenant's occupied-slot prefix, in both VRAM and the gen-shadow.

2. **HarvestPipeline**: single-scan per-(cell, view) partition over the
   no-alloc `query_*_in` seams, routing valid tokens (offset by the cell's
   GPU region base) into per-`MeshClass` staging arrays. Below a 25% hit
   ratio, `crate::simd::compress_tokens` (scalar reference; AVX2 verified
   bit-identical) dense-compacts the run instead, appending a
   `remap[dense_i] = original_run_index` segment. `harvest_views` batches
   multiple views, each into its own `(Scratchpad, HarvestStaging)` pair —
   verified thread-safe (no shared mutable state across views) by a
   `std::thread::scope` concurrency test.

3. **Leases**: `LeaseMask`/`HarvestLease` read-lease pool with §9.2.1
   isolation-budget revocation (`revoke_overdue`) and a stale-validation lane
   (`revalidate_run`) for recovering a revoked lease's results within the
   issuing frame.

**Test the GPU suites locally (must run sequentially due to device contention):**

```bash
# Core CPU tests (pass on any platform, run in CI)
cargo test -p pulsar_scenedb --lib --tests

# GPU tests (require local GPU, run sequentially with --test-threads=1)
cargo test -p pulsar_scenedb --features gpu --test gpu_store   -- --test-threads=1
cargo test -p pulsar_scenedb --features gpu --test gpu_harvest -- --test-threads=1
cargo test -p pulsar_scenedb --features gpu --test gpu_assets  -- --test-threads=1
cargo test -p pulsar_scenedb --features gpu --test gpu_layout

# Doc-test phase machine and compile_fail gates
cargo test -p pulsar_scenedb --features gpu --doc

# Graphics-free core guard (CI check)
cargo check -p pulsar_scenedb --no-default-features
```

**Note:** GPU test suites are computationally expensive; device contention will
cause timeouts or failures if run in parallel. Pass `--test-threads=1` to
serialize all GPU-feature tests — this applies to every `--features gpu`
integration-test binary that opens its own headless device (`gpu_store`,
`gpu_harvest`, `gpu_assets` today; `gpu_layout` and the `--doc` suite have not
needed it so far, but treat any new GPU-feature test target the same way by
default). Never invoke two `--features gpu` test binaries concurrently (e.g.
in parallel `cargo test` jobs) — run them one at a time, in sequence. CI runs
core tests only plus the `--no-default-features` check to guard the
graphics-free invariant (C0).

**Benches** (`benches/scenedb_bench.rs`, criterion, `harness = false`; numbers
land in the Task 10 report, not gates — no assert/regression thresholds):

```bash
# CPU-only spatial-query/churn benches (no GPU feature needed)
cargo bench -p pulsar_scenedb --bench scenedb_bench

# Full set incl. M2b-β GPU-feature benches (region sync, harvest partition,
# DEI compaction, promotion/demotion cycle) — same device-contention caveat
# as the GPU test suites above: do not run alongside another GPU-feature
# process.
cargo bench -p pulsar_scenedb --features gpu --bench scenedb_bench

# Compile+smoke only (used in the acceptance matrix; skips full sampling)
cargo bench -p pulsar_scenedb --features gpu --bench scenedb_bench -- --test
```
