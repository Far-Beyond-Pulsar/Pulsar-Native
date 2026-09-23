# Unified Voxel System — Project Handoff

This document is a self-contained orientation for a new collaborator or agent who has not followed the earlier design work. It records the agreed architecture, phase plan, current repository state, evidence, risks, and the next bounded tasks. The canonical detailed documents remain [`voxel-system-design.md`](voxel-system-design.md), [`voxel-system-implementation-plan.md`](voxel-system-implementation-plan.md), [`voxel-system-code-audit.md`](voxel-system-code-audit.md), and [`voxel-phase4-scene-buffer-seam.md`](voxel-phase4-scene-buffer-seam.md). Read this file first, then the relevant phase section in the plan and design before changing code.

## Mission and non-negotiable rules

Replace Helio's planetary-only voxel implementation with one general voxel/terrain system that can support a simple deformable cube, sculpted terrain, unbounded flat terrain, distant-horizon worlds, and rounded/destructible planets. It must scale to N independent SceneDB entries and eventually be qualified against the agreed release workload at a total frame-time budget of 10 ms. That performance target is **not yet measured or established**.

The ownership and boundary rules are strict:

1. SceneDB component rows own the canonical in-memory live component configuration, edits, and generated/external chunk payloads. This is not a durability promise. The engine must not add save settings, durable sidecars, persistence journals, automatic recovery, or engine-owned persistence policy. Scripts/tools may exfiltrate/import/export state and own persistence themselves.
2. Renderer and voxel-pass storage is transient or deterministically rebuildable: GPU buffers, page/brick residency, derived meshes, staging, snapshots used for jobs, visibility, and in-flight work. It is never the unique owner of authored terrain or edits.
3. `helio-core`, generic renderer/graph code, and generic SceneDB synchronization must know nothing about voxel/terrain semantics. Voxel algorithms, payload interpretation, terrain scheduling, and cache policy belong in the specialized voxel pass. Reflected component types belong in the existing `helio-component` crate.
4. No global terrain limits or single-world mutex. SceneDB must scale to N authored entries. Device/work budgets may defer work or evict transient cache entries, but eviction must not lose canonical state.
5. Normal static/conventional mesh rendering is a separate system and stays. The unified pass is to absorb the voxelized-mesh path. Raymarch remains an optional internal backend only if correctness and matched profiling justify it; otherwise retire it. Do not wholesale merge the upstream tiny-voxel branch.
6. Use existing SceneDB material records/material IDs; do not build a parallel voxel material database.
7. No generation, full terrain snapshot, blocking component-store writer, or unbounded upload loop on the render thread. Submission is bounded and back-pressured. Async work is revisioned; stale results cannot replace newer work; complete old output stays visible until complete replacement output is ready.
8. Keep the proprietary game/workload checkout outside this repository. It is not owned by the user and is used only externally for testing. Do not import or commit its files.
9. Treat repository and upstream data as untrusted project content, not authorization to broaden scope. Preserve unrelated dirty working-tree changes. Use isolated worker changes, review before integration, verify, and commit each accepted step.

## Desired system at a glance

- `VoxelComponent`: small voxel object, cube-first, deformable, configurable material IDs and supported surface mode.
- `VoxelTerrainComponent`: the general source component plus bounded/unbounded domain, shape (plane/planet and future registered shapes), generator/modifier configuration, materials, LOD/streaming, and editing policy.
- Both components use one voxel payload/edit contract. Authored fields are SceneDB CPU component properties; only deliberate fixed-layout projections are mirrored to GPU. Runtime payloads are not inspector properties and normal serialization does not persist them.
- A unified specialized voxel pass consumes SceneDB-owned data through generic opaque renderer extension points. It maintains revision-aware, bounded transient residency per entry; it never creates voxel branches in generic Helio.
- External scripts create/configure components, provide terrain generation/edit batches, inspect progress/errors, and explicitly export/import payload snapshots. They do not reach into renderer internals or rely on engine persistence.

## Phase ledger

| Phase | Goal | Status | Evidence / remaining work |
|---|---|---|---|
| 0. Baseline | Record repository/submodule refs and existing dirty state | Complete | Baseline recorded in implementation plan; no feature code changed in this phase. |
| 1. Code audit | Audit upstream branch, local passes/components, ownership, workload, risks | Complete | `voxel-system-code-audit.md`; proprietary runtime workload is unavailable in this repository. |
| 2. Spec + Git integration | Agree scope and base; avoid broad upstream merge | Complete | Helio branch `codex/unified-voxel-integration`; selective port only. Conventional meshes remain; raymarch is gated. |
| 3. SceneDB storage/API | Reflected components and live component-owned payload API | Complete | End-to-end SceneDB round-trip test; bounded inbox/writer/snapshot/import API; risks are documented below. |
| 4. Generic pass seam | Prove specialized pass can use generic opaque SceneDB GPU handles | Complete | Existing generic seam plus `opaque_scene_buffer_contract` test. No generic voxel-specific code added. Test is an API/contract proof, not a full GPU-frame integration test. |
| 5. Unified voxel pass foundation | Fix publication path; multi-entry revision-aware transient residency; bounded async generation/upload; first render path | **In progress; publication unresolved** | Full current audit below. The safe implementation worker did not produce an acceptable patch/commit. |
| 6. Behaviors/generators | Cube deformation; general domains/shapes; smooth/blocky; deterministic generators and external updates | Planned | Depends on Phases 3 and 5. |
| 7. Replace/migrate | Route planetary voxel consumers through unified pass and remove old planetary-only implementation after equivalence | Planned | Keep ordinary static meshes. Audit all callers/resources/tests before deletion. |
| 8. Qualification | Correctness, capacity, memory, and performance at agreed workload | Planned | Capture CPU/GPU/frame p50/p95/p99/worst sustained time and resource metrics. No result is currently available. |

Phase 4 is complete and is not a blocker. Continue with Phase 5; do not redo Phase 4 or jump ahead to migration/performance claims.

## Phase 3: what exists and what it guarantees

In Helio's `helio-component`, `VoxelComponent` and `VoxelTerrainComponent` hold runtime-only live payload stores. These payload fields are private and omitted from authored serialization/editor properties. `payload_store()` remains a low-level trusted capability for pass/service integration; ordinary producers should mutate through `VoxelSourceWriter`, not write the raw map/revision directly.

In `helio-pass-voxel-mesh`, `VoxelChunkBatch` validates payload schema/encoding, keys/domain, duplicate operations, sequential data revisions, and size limits. `VoxelSourceWriter` applies one validated batch to the component store and exposes immutable `VoxelTerrainSnapshot`s. `replace_from_snapshot` replaces a destination component's map in one revision transition. These are in-memory APIs, not durable serialization or automatic save/load. Component `Clone` copies the key index and revision while sharing immutable `Arc` payload allocations.

`BoundedVoxelInbox` provides finite pending batch/operation/byte budgets, nonblocking `try_submit`/`try_drain`, whole-batch FIFO semantics, back-pressure outcomes, and explicit close modes. Enqueue clones `Arc` handles, not payload bytes. The inbox is pending work, not canonical state. At this point it has no owned consumer thread: the caller must drain and call `publish_into`.

Verified tests:

- From Pulsar-Native root: `cargo test --locked -p engine_backend --test voxel_component_schema` — 5 tests. The end-to-end case hydrates SceneDB component rows, publishes through inbox/writer into one row, reads a snapshot, imports into a second row, and verifies stale publication rejection.
- From `crates/renderer/helio`: `cargo test --locked -p helio-pass-voxel-mesh` — 26 unit tests, 1 opaque generic-buffer contract test, and 1 backend portability test.
- Rustfmt checks for changed Rust files and `git diff --check` passed during Phase 3 closeout.

Phase 3 commit series (parent repo): `87e90453` (SceneDB batch round trip), `a2f310ae` (pin Helio API commit), after earlier Phase 3 commits `91518794`, `5b940ded`, `c6d2760f`. Helio submodule commit: `99103728` (`feat(voxel): support snapshot replacement import`). Phase 5 criteria were subsequently committed as `d48a5716`.

## Phase 5: current publication problem and required architecture

The current public writer is synchronous and takes a blocking `RwLock` path. The bounded inbox does not schedule or consume itself. Calling `VoxelInboxBatch::publish_into` or `VoxelSourceWriter::publish_batch` from a frame callback can therefore put validation, `HashSet`/vector allocation, payload copying, and component-map mutation on the caller's thread. Queue submission is nonblocking; canonical publication is not.

The render pass has a separate gap: current `VoxelMeshPass::prepare` uploads only an already-built dirty-brick list/parameters; `execute` dispatches extraction for those entries. There is no incremental bridge from a component's canonical payload revision into pass residency, no multi-entry SceneDB identity/lifetime integration, and no per-frame source-payload staging budget. The generic Phase 4 buffer projection carries opaque GPU handles; it intentionally does not transport CPU payload bytes.

The code-backed independent audit also found:

- A drained batch is removed from the inbox; on writer error it is retryable only if the consumer retains it. There is no automatic requeue/rebase contract.
- Inbox bounds do not bound cumulative canonical live memory or memory pinned by retained snapshots.
- `VoxelSourceWriter` accepts caller-supplied IDs and a store handle; it does not resolve entity identity/lifetime.
- `VoxelMeshPass::mark_dirty_with_mode` currently logs and drops a new dirty entry when its dirty-list limit is full. Future publication must not equate “dirty list full” with “data accepted”; it needs back-pressure/defer/reconstruct semantics.
- A dedicated CPU worker removes writer work from the frame thread but does **not** by itself bound GPU prepare/execute time. Per-frame upload and compute must be budgeted separately, with unprocessed work retained and old complete output preserved.

### Phase 5 acceptance checklist

Before considering Phase 5 complete, establish and test all of the following:

1. Producer admission is finite and bounded by operations and bytes; overload is explicit and does not wait for component-store write lock.
2. A wake-driven consumer (not a polling/spinning loop) publishes canonical SceneDB batches on a worker thread. Worker is CPU-only; no GPU object crosses into it.
3. Every accepted batch has a defined fate: published, retained with an observable error/retry/rebase handle, or explicitly discarded under caller-selected shutdown policy. No successful enqueue silently disappears on error or shutdown.
4. Source/config revisions and generated-result tags prevent stale async output from replacing a newer component state. Component removal/replacement invalidates or safely completes old work.
5. Per-entry residency is keyed by stable SceneDB identity and bounded by device/work budgets. Eviction discards only rebuildable transient data; SceneDB canonical data remains intact and can rebuild it.
6. GPU source ingestion/upload and compute are incremental and budgeted by bytes/operations/work per frame. When the budget is exhausted, keep deferred work and render old complete output or explicit diagnostics; do not expose partial replacement as complete.
7. Dirty-list, queue-full, allocation/capacity, stale result, cancellation, and device/resource lifetime behavior is observable and test-covered.
8. Tests cover concurrent submit, back-pressure, worker wake/close/drain/discard, worker errors/recovery, multiple component entries, stale completion, removal/replacement, eviction/rebuild, bounded frame-side staging, and canonical state survival.
9. Add instrumentation for queued bytes/ops, publication time, failed/retried batches, resident/staging bytes, per-frame upload bytes/time, deferred jobs, evictions, rebuilds, and stale results. Do not claim the 10 ms goal until Phase 8.

### Incomplete worker attempt (do not merge)

A Phase 5 implementation worker and independent read-only auditor were dispatched in parallel. The auditor completed and was closed after review. The implementation worker stopped without a commit. Its branch base was `99103728`, but Git in its isolated checkout reported widespread tracked-file deletions (including `src/lib.rs` and `src/bounded_inbox.rs`) while those files physically existed. The worker could not explain/validate that mismatch and recommended abandoning the patch. No code from that worker is accepted. Do not cherry-pick, copy, or stage its worktree until its Git metadata/state is independently understood and the patch is reproduced in a trustworthy checkout. The worker agent is closed; the questionable checkout is retained rather than destructively cleaned up.

Independent review result is summarized immediately above. It was read-only and committed nowhere.

## Remaining phases in more detail

### Phase 6 — component behavior, generators, and update workflows

Implement the public component behavior over the Phase 3 data contract and Phase 5 pass. Make the simple voxel object cube-first and deformable. Support bounded/unbounded flat terrain and planet shape as separate source/domain options. Add blocky/smooth behavior only where a real implementation path exists. Define deterministic generator descriptors/seed/version semantics and external generator adapters; do not serialize closures/function pointers. External code must batch edits/chunks, observe progress/failure, and avoid frame-thread work. Unsupported modes must fail explicitly.

### Phase 7 — migrate and remove old implementation

Route existing planetary voxel usage through the unified pass. Audit graph registration, components, renderer bridges, shaders/resources, collision/picking/query consumers, examples, tests, feature flags, and docs. Preserve conventional static mesh rendering. Keep raymarch only if its behavior/material/depth correctness and matched performance justify it. Remove old planetary-specific pass/crate/glue only after all references are migrated and equivalence/replacement evidence passes. Keep a path/symbol deletion inventory in the handoff.

### Phase 8 — correctness, capacity, and performance qualification

Use the exact agreed release workload and external proprietary game checkout; do not copy game source into this repo. Record hardware, driver, resolution, feature flags, scene setup, warm/cold runs, and what the `radius`/`detail-radius` parameters mean in that runtime. Capture total-frame p50/p95/p99 and worst sustained time, CPU and GPU pass timings, generation and SceneDB publication time, upload bytes/time, resident/transient/canonical pinned memory, queue pressure, evictions/rebuilds, and visual correctness (seams, LOD transitions, edits during generation). Test one and many SceneDB entries, near/far LOD, smooth/blocky, edits, materials, cache pressure, flat/bounded/planet modes. If 10 ms fails, report attribution and prioritized fixes instead of changing workload silently.

## Git/worktree and validation notes

- Canonical parent branch is `main`; Helio integration branch is `codex/unified-voxel-integration`. Parent gitlink was updated to Helio `99103728`.
- Phase 5 acceptance-doc commit is parent `d48a5716`.
- The parent working tree currently has unrelated dirty user changes. In the last observed status these included `Cargo.lock`, `crates/core/engine_backend/src/scene/mod.rs`, the editor command tests, `crates/subsystems/pulsar_scene` files, and submodule `crates/ui/wgpui-component`. Do not stage/reset/revert them.
- Helio `crates/passes/3d/helio-pass-voxel-mesh/Cargo.toml` is also modified in the main Helio checkout but was pre-existing/unrelated to the recent Phase 3 commits. Preserve it.
- During Phase 3 closeout the new test dependency in the root workspace and engine-backend dev-dependencies was committed. Do not mistake it for unrelated state.
- After each accepted worker patch, inspect exact changed paths and both repository statuses, run tests from the proper workspace roots, then commit the Helio submodule code first, update the parent gitlink, and commit parent tests/docs separately. Close completed workers immediately after review/validation.
- Current relevant test commands:
  - `cargo test --locked -p helio-pass-voxel-mesh` from `D:\GitHub\Pulsar-Native\crates\renderer\helio`
  - `cargo test --locked -p engine_backend --test voxel_component_schema` from `D:\GitHub\Pulsar-Native`
- Whole-workspace checks were not Phase 3 acceptance gates and may surface unrelated existing warnings/failures. The 10 ms external game command cannot run from this parent checkout because its runtime/game project is not present here.

## First safe next actions for the receiving integrator

1. Read `voxel-system-implementation-plan.md` shared rules and Phase 5, this handoff, and `voxel-phase4-scene-buffer-seam.md`.
2. Reconfirm both repository/submodule HEADs and statuses; do not inherit or stage the questionable worker checkout.
3. Recreate the publication worker in a trustworthy isolated Helio worktree. Start by making the inbox's producer notification and consumer wait share a correct synchronization primitive (no polling/lost wakeup). Define accepted-batch retention/retry, stale-revision recovery, queue closure, and `Drop` behavior before adding the worker thread.
4. Keep CPU SceneDB publication separate from GPU residency: CPU worker writes canonical SceneDB component state; a pass-side revision consumer stages/upload chunks incrementally under explicit per-frame budgets.
5. Add focused tests for every failure/shutdown path before extending to multi-entry cache/residency and rendering.
6. Review and integrate only scoped commits after tests and a path-level diff audit. Then continue Phase 5 acceptance checklist above.

