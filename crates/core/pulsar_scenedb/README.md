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

Test the GPU layer locally:

```bash
cargo test -p pulsar_scenedb --features gpu --test gpu_store
cargo test -p pulsar_scenedb --features gpu --test gpu_layout
```

Note: GPU tests require a local GPU device; CI runs core tests only
(`cargo test -p pulsar_scenedb --lib --tests`) plus the graphics-free guard:

```bash
cargo check -p pulsar_scenedb --no-default-features
```
