# SceneDB — Rebase onto Reorganized `main` + Progress Report

**Date:** 2026-07-12
**Branch:** `scenedb` (rebased onto `origin/main` @ `7aff16c8`)
**Backup:** `backup/scenedb-pre-reorg-2026-07-12` @ `800800a9` (pre-rebase tip, do not delete until pushed + verified in CI)

---

## 1. What happened on `main`

`main` moved **300 commits** past the old merge-base (`590d390b`) and reorganized the
repo: the flat `crates/*` layout became categorized —
`crates/{core, editor, subsystems, agent-providers}` as workspace-member globs, plus
`crates/{ui, graphics, third-party}` and `plugins/vendor` as **git submodules**
consumed via `[patch]` path deps. `pulsar_ecs` (SceneDB's seed/reference crate) moved
to `crates/core/pulsar_ecs`. All crates SceneDB depends on (`pulsar_core`,
`pulsar_reflection`, `profiling`) changed only by rustfmt formatting — **no API
changes** affecting SceneDB.

The pre-rebase working tree could not even build: leftover submodule checkouts
(`crates/graphics` etc.) broke the old branch's `crates/*` member glob. The rebase
resolves this permanently.

## 2. Rebase procedure (as executed)

1. **Safety:** backup branch created; uncommitted M2a design Rev 2 edits snapshotted
   (sha256 `adf83bb1…`) and stashed across the rebase, then restored bit-identical.
2. `git rebase origin/main` — all **48 commits replayed**; exactly **one conflict**,
   in the seed commit `c8cae8e6` (`Cargo.toml`): resolved by keeping main's
   reorganized manifest and adding the `pulsar_scenedb` member + workspace-dep
   entries. The `Cargo.lock` sync commit auto-merged to a clean +14-line package
   entry.
3. **Relocation commit:** `git mv crates/pulsar_scenedb crates/core/pulsar_scenedb`
   (pure rename, 0 insertions/0 deletions, 29 files) to match main's taxonomy
   (`crates/core/` = "Engine fundamentals — ECS, reflection, …"); workspace dep path
   updated; `Cargo.lock` reconciled by cargo (6 lines: `ahash`/`criterion` name
   disambiguation, `rand` 0.8.6→0.8.7).

## 3. Empirical verification

| Check | Result |
|---|---|
| Commits replayed (`origin/main..scenedb`) | 48 / 48 |
| `git range-diff` old vs new series | 47 content-identical (`=`); only seed commit differs (Cargo.toml resolution) |
| Crate tree hash pre vs post rebase | identical: `70dd7123…` (byte-for-byte) |
| `docs/superpowers` tree pre vs post | identical (7 files) |
| Uncommitted M2a Rev 2 doc restored | sha256 match `adf83bb1…` |
| Submodule checkouts vs main's gitlinks | all 13 clean, exact commit match |
| Pre-rebase tests (clean worktree @ `800800a9`) | 117 passed, 0 failed (51 lib + 2 contention + 3 stale_handle + 61 stress) |
| Post-rebase tests (`cargo test -p pulsar_scenedb --lib --tests`) | 117 passed, 0 failed — identical to baseline (51 + 2 + 3 + 61) |

## 4. SceneDB progress report (verified against the tree, 2026-07-12)

**Crate:** `crates/core/pulsar_scenedb` — src 4,179 LOC · tests 2,095 LOC · benches
1,977 LOC · 117 `#[test]` fns, all green.

| Milestone | Status | Evidence |
|---|---|---|
| **Stage 0** — spec + frozen contracts | ✅ DONE | `specs/SceneDB2.0.md` (Rev 2.3), `specs/CONTRACTS.md` (C0–C7 incl. Ownership Law C0) |
| **M1a** — Layer 1 storage core (9 tasks) | ✅ DONE, dual-reviewed | Handles, 64B-aligned paged SoA (128B stride), atomic liveness, swap-and-pop compaction, scalar AABB query; baseline ~762 ns/1024 rows |
| **M1b** — TypeToken/SIMD/leases (11 tasks) | ✅ DONE, dual-reviewed — **Layer 1 COMPLETE** | TypeToken↔reflection bridge, CellType stride check, runtime-dispatched AVX2 AABB+frustum (bit-for-bit vs scalar), LeaseMask pool, Scratchpad, double-buffered LivenessSnapshot, Part VI Tests 1 & 2(host); dispatched AABB ~609 ns/1024, frustum ~1.22 µs/1024 |
| **M2a** — GPU-resident store, delta-sync, retirement (`pulsar_scenedb_gpu`) | 🔶 DESIGN done (committed `800800a9`); **Rev 2 post-adversarial-review edits uncommitted in worktree**; implementation NOT started (no `pulsar_scenedb_gpu` crate exists) | `specs/2026-06-13-scenedb20-m2a-gpu-store-design.md` — Rev 2 fixes row/slot index-space conflation, free-seam assumption, compaction × in-flight-GPU-rows |
| **M2b** — asset integration & streaming | ⬜ not started | master design §5b |
| **M3** — Helio Layer 3 (stateless consumer) | ⬜ not started | master design §6; Helio branch `scenedb20` |
| **M4** — integration, `scenedb2` flag cutover, ECS replacement | ⬜ not started | master design §7 |

**Known M2 carry-forward (from M1b):** Scratchpad needs `get_u64` for liveness-word
snapshots to honor §8.1 no-alloc (`query_aabb`/`query_frustum` still allocate a
`Vec<u64>` per call); the phase machine wires leases/snapshot in.

## 5. Next steps

1. Commit the M2a design Rev 2 doc once its author review is finished.
2. `git push --force-with-lease origin scenedb` (remote still holds the pre-rebase
   47-commit series; local had 1 unpushed commit before the rebase).
3. Write the M2a implementation plan. **Key call (2026-07-12, post-rebase): no
   separate GPU crate** — M2a lands as the feature-gated `pulsar_scenedb::gpu`
   module (`gpu` feature, optional wgpu workspace dep, already pinned to the
   Helio-matched fork rev). See M2a design Rev 3 + amended CONTRACTS.md C0.
4. Delete `backup/scenedb-pre-reorg-2026-07-12` only after the pushed branch is
   verified.

**Housekeeping note:** an old `stash@{1}` ("WIP on scenedb: e17678d2 … column-slice
soundness holes") predates this session — review and drop it if obsolete.
