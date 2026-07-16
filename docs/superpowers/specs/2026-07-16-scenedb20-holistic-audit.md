# SceneDB 2.0 — Holistic Spec-Conformance Audit (Stage 0 → M2b-β T1)

**Date:** 2026-07-16 · **Audited commit:** `060175b5` · **Scope:** everything
built since Stage 0, audited as one system against the spec of record
(SceneDB2.0.md Rev 2.3) and CONTRACTS C0–C7 — the first cross-milestone pass
(prior reviews were per-task and per-milestone).

**Method:** five parallel read-only auditors — A: C0+C6 end-to-end; B: C1–C3;
C: C4+§8/§9; D: C5+C7+§6/§7/§10; E: whole doc-stack coherence. Full findings
live in the run ledger directory (`.superpowers/sdd/audit-{A..E}-*.md`,
session-local); this document is the durable synthesis and disposition.

## Verdict

**The machinery is sound; the paperwork and two enforcement claims were not.**
Zero behavioral defects were found in the shipped retirement, region,
delta-sync, layout, or SIMD code. Both violations are *enforcement* failures
(a build-graph edge and a CI claim), and the doc stack lags several shipped
normative decisions.

## Findings & disposition

### Violations (fixed in the audit-remediation code batch)

| # | Finding | Fix |
|---|---|---|
| V1 (A) | **C0 dependency direction breached at the build graph:** `pulsar_scenedb → pulsar_reflection` whose `default` features included `prims-helio → helio` (git). Invisible to the CI graphics-free guard (`--no-default-features` does not strip a *dependency's* defaults, and per-edge `default-features = false` is defeated by workspace feature unification). | `prims-helio` removed from `pulsar_reflection`'s defaults; consumers that need Helio prim registrations declare it explicitly; CI gains a `cargo tree … \| grep -i helio` **no-Helio-edge guard**. |
| V2 (D) | **C5's enforcement sentence was false:** "Test 3 … in CI on every PR" — the `gpu` feature never built in CI, so no GPU test target ever ran there. | CI gains the Test 3 gate (`--features gpu --test gpu_layout` — naga reflection only, no GPU adapter required). C5's sentence amended to match reality (Test 3 in CI; device-dependent suites local, sequential). |

### High-value gaps (fixed in the code batch)

- **§9.2.1 fence was nobody's deliverable** (C): `LivenessMask`/`LivenessSnapshot`
  correctly documented a phase-boundary Release/Acquire edge as their
  precondition, but nothing emitted it. The phase machine now owns it
  (fences at the Simulate→Harvest and boundary transitions), and the
  liveness/snapshot docs point at it.
- Slot-mirror fill loop gains a release-mode region-bound assert (A);
  `register_token_column` documents its aliasing hazard (β T1 review).

### Deferred with owners (already recorded in design §11 / plan; audit confirms)

- Public `free()`/`compact()` reachable on GPU-registered cells via
  `CellSlot.cell` — the C6 bypass both A and B flagged. Owner: **M4**
  World-level demotion (design §6 amendment). Interim: doc warnings; the
  pinned-row debug_assert catches the deferred-path mix-up only.
- Phase-machine witness-hoarding + boundary liveness (§11), threaded write
  window, `rebuild` no-pins precondition being debug-only — **β/M4** as
  recorded.
- NEON bit-identity property tests are `cfg(aarch64)` and never execute on
  x86 CI (C) — recorded; needs an ARM runner or cross-run to be a live gate.
- `pulsar_reflection` unconditionally pulls the gpui-ce/ui stack into
  scenedb's transitive graph (audit A note) — crate-split follow-up, owner
  M4/repo-hygiene.

### Doc-stack corrections (audit-remediation docs batch)

From E (5 contradictions, 6 unrecorded amendments, 6 stale claims):

1. **CONTRACTS.md** absorbs three shipped normative decisions, code-first
   drift now regularized: C5 — cluster **node 0 reserved** and
   `cluster_table_offset` is a **node index**, not a byte offset (spec §6.1's
   "byte offset into global cluster DAG buffer" contradicts both the code and
   spec §16.2 — spec-side fix goes on the Rev 2.4 list); C5 — the row-indexed
   **global-slot mirror** joins the owned-buffer inventory; C6 — the eviction
   exception (queued retires commit **CPU-side only** at eviction; VRAM
   untouched; region bytes protected by the serial pin).
2. **M2a design §4**: the "delta-sync threads the M1 Scratchpad" claim is
   corrected — the shipped coalescer streams ranges and needs no scratch;
   `Scratchpad::get_u64` serves the harvest path instead.
3. **Master design**: status table refreshed (M2a, M2b-α complete; M2b-β in
   progress); stale "owned by M2.0"/pre-split remnants swept.
4. **M2b design §4.1**: the eviction-timing refinement recorded now (not
   deferred to β Task 4's landing).
5. **README/lib.rs**: stale "Rev 2.2" spec pins → Rev 2.3.
6. **Spec §19 meshlet buffer**: built nowhere, deferred nowhere → now
   explicitly deferred to **M3** (design §10; C0 note: SceneDB owns it — the
   M3 plan must add it to the asset store beside `ClusterBuffer`).

### Spec Rev 2.4 punch list (the vendored spec is edited only in the Research repo)

R1 `cluster_table_offset` unit: byte offset → **node index** (§6.1 vs §16.2).
R2 Generation-buffer write triggers: add first-mirrored-write-after-alloc +
bulk rebuild (currently retirement-only). R3 §7/§7.1 "compile-time
assertion"/"registration macros" → runtime builder reality (or keep as
aspiration with an explicit note). R4 §10 buffer inventory: add the
slot-mirror buffer. R5 §9.2.1: name the phase machine as the fence owner.
R6 §5.1 HLOD proxy cells: permanently-resident size class + budget term.
R7 §19: meshlet-buffer ownership/milestone statement.

## Bottom line

Two enforcement violations fixed, one unowned precondition given an owner,
the contract stack re-synchronized with shipped reality, and a seven-item
punch list staged for the next spec revision. No shipped behavior was wrong;
what drifted was what *claimed to be checking* it. M2b-β resumes on a fully
audited base.
