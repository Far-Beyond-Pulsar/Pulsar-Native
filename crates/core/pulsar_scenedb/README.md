# pulsar_scenedb

SceneDB 2.0 Layer 1 storage core. Spec: `docs/superpowers/specs/SceneDB2.0.md`
(Rev 2.2). Contracts: `docs/superpowers/specs/CONTRACTS.md`. Design:
`docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`.

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

**Test the GPU suites locally (must run sequentially due to device contention):**

```bash
# Core CPU tests (pass on any platform, run in CI)
cargo test -p pulsar_scenedb --lib --tests

# GPU tests (require local GPU, run sequentially with --test-threads=1)
cargo test -p pulsar_scenedb --features gpu --test gpu_store -- --test-threads=1
cargo test -p pulsar_scenedb --features gpu --test gpu_assets -- --test-threads=1
cargo test -p pulsar_scenedb --features gpu --test gpu_layout

# Doc-test phase machine and compile_fail gates
cargo test -p pulsar_scenedb --features gpu --doc

# Graphics-free core guard (CI check)
cargo check -p pulsar_scenedb --no-default-features
```

**Note:** GPU test suites are computationally expensive; device contention will
cause timeouts or failures if run in parallel. Pass `--test-threads=1` to
serialize all GPU-feature tests. CI runs core tests only plus the
`--no-default-features` check to guard the graphics-free invariant (C0).
