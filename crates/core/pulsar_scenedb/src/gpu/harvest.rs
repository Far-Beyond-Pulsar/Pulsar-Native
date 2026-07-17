//! `HarvestPipeline` — single-scan per-view partition emitting global-row
//! tokens (M2b-b Wave 2 T6, design Rev 2 §5; spec §8.3-8.5, C4).
//!
//! One cell, one view, one scan: [`HarvestPipeline::harvest_cell`] queries a
//! resident cell's positional token run (via the no-allocation §8.1
//! `query_*_in` seams landed in T2) and routes every VALID token into the
//! [`MeshClass`]-selected staging array, offsetting it by the cell's GPU
//! region base. The `NULL_ROW` sentinel is dropped, never offset (§2) — a
//! `region_base + NULL_ROW` value would silently wrap into what looks like a
//! plausible-but-wrong global row, so the routing loop (and the DEI compact
//! kernel) both filter it out BEFORE the add, not after.
//!
//! DEI (§8.5): when a run's hit ratio falls below 25%, the plain
//! filter-and-offset scan is replaced by [`crate::simd::compress_tokens`] (the
//! scalar reference; AVX2 lands in T7), which additionally appends the
//! original run index of every hit to `staging.remap` — the M3-frozen
//! `remap[dense_i] = run index` layout that lets a downstream consumer map a
//! dense output slot back to its source row.

use crate::lease::{Lease, LeaseMask};
use crate::registry::NULL_ROW;
use crate::snapshot::{LivenessSnapshot, RevocationFlag};
use crate::spatial::SpatialCell;
use crate::Scratchpad;
use std::sync::Arc;

use super::HarvestPhase;

/// Which GPU-side mesh pipeline a harvested cell's geometry renders through
/// (design Rev 2 §5.2). Routes a harvested run into the matching
/// [`HarvestStaging`] array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshClass {
    Traditional,
    VirtualGeometry,
    HlodProxy,
}

/// The spatial predicate a harvest pass is scanning against — an AABB or a
/// six-plane frustum, mirroring [`SpatialCell::query_aabb_in`]/
/// [`SpatialCell::query_frustum_in`]'s two query shapes.
pub enum View {
    Aabb(crate::spatial::Aabb),
    Frustum(crate::spatial::Frustum),
}

/// Per-view staging arrays (§5.2). Persistent — cleared via [`Self::clear`],
/// never reallocated, once per frame; capacity survives across frames after
/// warm-up (§8.1).
#[derive(Default)]
pub struct HarvestStaging {
    pub traditional: Vec<u32>,
    pub vg: Vec<u32>,
    pub hlod: Vec<u32>,
    /// M3-frozen: `remap[dense_i] = original_run_index`. Only ever grown by
    /// DEI-compacted runs (§8.5); plain-path runs append nothing here.
    pub remap: Vec<u32>,
    /// Expected-generation harvest column (design §3.1, M3-α T8 — Test 2's
    /// CPU-side data path): `traditional_gens[i]` is the registry generation
    /// expected for the handle backing `traditional[i]`, at the moment of
    /// harvest — positionally aligned with `traditional` one-for-one (C4
    /// "aligned across columns"). The downstream M3-β cull shader compares
    /// this against the LIVE generations buffer at the token's global slot
    /// and drops rows whose generation has since moved (stale/reused slot).
    ///
    /// **Sentinel handling:** [`NULL_ROW`] never reaches `traditional`,
    /// `vg`, or `hlod` in EITHER routing path (the plain path's
    /// `if *t != NULL_ROW` filter and [`crate::simd::compress_tokens`]'s
    /// unconditional sentinel strip both drop it before any push), so no
    /// gens column ever needs to hold a value for a sentinel row — every
    /// push into a token array is paired, in the same statement or the same
    /// small loop body, with exactly one push into its gens column. That
    /// pairing is the invariant [`HarvestPipeline::harvest_cell`] asserts
    /// after every cell: `traditional.len() == traditional_gens.len()`
    /// (and the `vg`/`hlod` pairs likewise).
    pub traditional_gens: Vec<u32>,
    pub vg_gens: Vec<u32>,
    pub hlod_gens: Vec<u32>,
    pub stats: HarvestStats,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct HarvestStats {
    pub cells: u32,
    pub tokens_valid: u32,
    pub tokens_total: u32,
    pub dei_compacted_runs: u32,
}

impl HarvestStaging {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear every staging array and zero the stats, WITHOUT freeing —
    /// `Vec::clear` on each array (§8.1: capacity is the observable
    /// no-allocation proxy; a fresh `Vec::new()`/`take` here would defeat the
    /// whole point of a persistent staging buffer).
    pub fn clear(&mut self) {
        self.traditional.clear();
        self.vg.clear();
        self.hlod.clear();
        self.remap.clear();
        self.traditional_gens.clear();
        self.vg_gens.clear();
        self.hlod_gens.clear();
        self.stats = HarvestStats::default();
    }
}

/// A held harvest lease: a cell's [`Lease`] slot (RAII — releases on drop)
/// paired with a revocation flag and the wall-clock (caller-supplied) instant
/// it was acquired at (spec §9.2/§9.2.1).
///
/// Holding a `HarvestLease` across a query means the holder's
/// [`LivenessSnapshot`] (captured at acquire time, or any time thereafter)
/// stays valid to read from even after the lease is revoked — revocation
/// only sets [`RevocationFlag`], it does not retroactively invalidate
/// already-pinned snapshot words. The holder is expected to re-validate
/// (via [`revalidate_run`]) against LIVE state before acting on stale
/// results; see that function's doc for the within-frame-only caveat.
///
/// No `std::time` anywhere in this crate's paths: `held_since_ms` and every
/// clock reading that interacts with it (`now_ms` in
/// [`HarvestPipeline::acquire_lease`]/[`HarvestPipeline::revoke_overdue`]) is
/// a plain caller-supplied `f64` millisecond value. The World driver owns
/// the real wall clock (or a deterministic test clock) and threads it
/// through; this crate never reads system time itself, which keeps the
/// isolation-budget check (C4: 2.0 ms) trivially deterministic in tests.
pub struct HarvestLease<'a> {
    lease: Lease<'a>,
    /// One-shot revocation flag (spec §9.2.1). Shared (`Arc`) so a driver
    /// tracking many outstanding leases can hold its own clone of the flag
    /// independent of the `HarvestLease`'s lifetime.
    pub revocation: Arc<RevocationFlag>,
    /// Caller-supplied clock reading (ms) at the moment this lease was
    /// acquired. Injectable — never sourced from `std::time` in-crate.
    pub held_since_ms: f64,
    /// Attribution for Test 10's "persistent revocations from the same
    /// client" diagnostic: threaded through to the `tracing::warn!` emitted
    /// by [`HarvestPipeline::revoke_overdue`] on each revocation.
    pub client: &'static str,
}

impl HarvestLease<'_> {
    /// The underlying cell-lease slot index (delegates to [`Lease::slot`]).
    #[inline]
    #[must_use]
    pub fn slot(&self) -> u32 {
        self.lease.slot()
    }
}

/// Stateless (β single-thread form) driver for one cell/view harvest scan.
/// Holds no state of its own — every buffer it touches (`Scratchpad`,
/// `HarvestStaging`) is caller-owned so the caller controls persistence and
/// threading.
pub struct HarvestPipeline(());

impl HarvestPipeline {
    #[must_use]
    pub fn new() -> Self {
        Self(())
    }

    /// Query one resident inner cell against one view and route its run into
    /// the staging arrays, adding `region_base` to every VALID token (§2 —
    /// the sentinel is never offset; it is dropped here, in both the plain
    /// and DEI-compacted paths). DEI (§8.5): when `valid/total < 0.25` the run
    /// is dense-compacted via [`crate::simd::compress_tokens`], appending a
    /// remap-table segment to `staging.remap`; otherwise a plain
    /// filter-and-offset scan runs. Returns the number of valid tokens
    /// routed (== the query's hit count).
    ///
    /// `_h`: the [`HarvestPhase`] witness — proof this call happens in the
    /// read-only harvest sub-phase (C4), after the frame's Release fence, so
    /// the liveness words captured below observe a stable, published
    /// simulate-phase snapshot.
    ///
    /// **C3/C4 freshness contract:** `region_base` must be re-resolved in the
    /// ISSUING frame via `grid.gpu_id(coord)` → `store.row_region_base(id)` —
    /// never cached across a frame boundary. Transitions execute only at
    /// boundaries (C3), so a base resolved this frame is stable through
    /// Harvest; a base cached across a boundary that evicted and re-promoted
    /// the cell into a different region would emit wrong global-row tokens
    /// SILENTLY (a stale `CellId` fails loud; a stale `u32` does not). The
    /// World driver owns this chain (M4).
    pub fn harvest_cell(
        &self,
        cell: &SpatialCell,
        region_base: u32,
        class: MeshClass,
        view: &View,
        pad: &mut Scratchpad,
        staging: &mut HarvestStaging,
        _h: &HarvestPhase,
    ) -> u32 {
        let len = cell.rows_in_use() as usize;
        let (tokens, words) = pad.get_u32_u64(len, len.div_ceil(64));
        let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, words);
        let n = match view {
            View::Aabb(q) => cell.query_aabb_in(q, &words[..nw], tokens),
            View::Frustum(f) => cell.query_frustum_in(f, &words[..nw], tokens),
        };
        // §3.1 expected-generation alignment (M3-a T8): bound once per cell,
        // consumed by both the plain and DEI paths below. `col0` is
        // LOCAL-row-indexed (never offset by `region_base` — only the
        // emitted TOKEN in `dest` gets that offset, the gen lookup never
        // does); `regs` is the registry's slot-indexed generation array, so
        // `regs[col0[local_row] as usize]` is the generation the handle
        // currently owning that row is expected to carry.
        let regs = cell.storage().registry().generations();
        let col0 = cell.storage().slot_column();
        let (dest, dest_gens) = match class {
            MeshClass::Traditional => (&mut staging.traditional, &mut staging.traditional_gens),
            MeshClass::VirtualGeometry => (&mut staging.vg, &mut staging.vg_gens),
            MeshClass::HlodProxy => (&mut staging.hlod, &mut staging.hlod_gens),
        };
        if len > 0 && (n as f32 / len as f32) < 0.25 {
            let remap_start = staging.remap.len();
            crate::simd::compress_tokens(&tokens[..len], region_base, dest, &mut staging.remap);
            staging.stats.dei_compacted_runs += 1;
            // DEI remap holds LOCAL run indices — exactly what `col0` needs.
            // Only the NEW segment this call appended (`remap_start..`) maps
            // to this cell's hits; earlier segments belong to prior cells in
            // the same (persistent) staging buffer.
            for &ri in &staging.remap[remap_start..] {
                dest_gens.push(regs[col0[ri as usize] as usize]);
            }
        } else {
            for (local_row, t) in tokens[..len].iter().enumerate() {
                if *t != NULL_ROW {
                    dest.push(region_base + *t);
                    dest_gens.push(regs[col0[local_row] as usize]);
                }
            }
        }
        // Invariant (§3.1): each dest/dest_gens pair stays positionally
        // aligned one-for-one — every push above is paired, so this can
        // never legitimately drift; a debug-only guard is enough to catch a
        // future edit that breaks the pairing.
        debug_assert_eq!(
            dest.len(),
            dest_gens.len(),
            "harvest gens column must stay positionally aligned with its token array (§3.1)"
        );
        staging.stats.cells += 1;
        staging.stats.tokens_valid += n;
        staging.stats.tokens_total += len as u32;
        n
    }

    /// Acquire a harvest lease from `mask` (spec §9.2), tagging it with
    /// `client` for revocation attribution and `now_ms` as its acquire-time
    /// clock reading. `None` if the 64-slot pool ([`crate::lease::LEASE_SLOTS`])
    /// is exhausted — spec §9.2's blocking-retry loop around exhaustion is
    /// the World driver's scope, not this crate's; a caller that wants to
    /// block simply calls this in a loop with its own backoff/yield policy.
    #[must_use]
    pub fn acquire_lease<'a>(
        &self,
        mask: &'a LeaseMask,
        now_ms: f64,
        client: &'static str,
    ) -> Option<HarvestLease<'a>> {
        let lease = mask.acquire()?;
        Some(HarvestLease {
            lease,
            revocation: Arc::new(RevocationFlag::new()),
            held_since_ms: now_ms,
            client,
        })
    }

    /// §9.2.1 isolation check (C4: 2.0 ms — a hold-duration TIMEOUT, the
    /// trigger condition for revocation, not a latency budget on any single
    /// operation; see `tests/stress_gpu.rs`'s storm 3 review note). Revokes
    /// every lease in `leases` held past `now_ms - held_since_ms >=
    /// budget_ms`: sets its [`RevocationFlag`] AND force-releases its
    /// `LeaseMask` slot immediately (M3-b, contract #32 closure) — this is
    /// the piece the perf-val T6 review found missing: without it, a
    /// revoked-but-undropped lease blocked an `any_held()`-gated compaction
    /// indefinitely, because the mask bit only ever cleared on the holder's
    /// own `Drop`.
    ///
    /// **Release timing, chosen reading:** IMMEDIATE, at revocation — not
    /// deferred to the holder's next check or its eventual `Drop`. This maps
    /// §9.2.1's "compaction proceeds on the primary layout immediately" onto
    /// the mask literally: the holder was never on the critical path in the
    /// spec's design (its reads continue against its own already-pinned
    /// [`crate::snapshot::LivenessSnapshot`], never the live liveness mask or
    /// the live row layout), so there is no reason for the SLOT to remain
    /// "held" for compaction-gating purposes one instant longer than the
    /// revocation itself. See [`Lease::force_release`] for why this is safe
    /// against the slot being reissued before the original holder's `Drop`
    /// eventually runs (that later `Drop` becomes a no-op by construction —
    /// no double-release, no ABA).
    ///
    /// Returns the number of leases revoked by this call; each revocation is
    /// logged via `tracing::warn!` with the lease's `client` attribution, so
    /// a client that repeatedly blows the budget shows up as repeated warns
    /// under the same `client` value ("persistent revocations from the same
    /// client", Test 10).
    pub fn revoke_overdue(&self, leases: &[&HarvestLease<'_>], now_ms: f64, budget_ms: f64) -> u32 {
        let mut revoked = 0u32;
        for lease in leases {
            let held_ms = now_ms - lease.held_since_ms;
            if held_ms >= budget_ms {
                lease.revocation.revoke();
                lease.lease.force_release();
                revoked += 1;
                tracing::warn!(
                    client = lease.client,
                    held_ms,
                    budget_ms,
                    slot = lease.slot(),
                    "harvest lease revoked: exceeded §9.2.1 isolation budget"
                );
            }
        }
        revoked
    }

    /// §9.2.1 / contract #32 compaction-readiness gate — the production
    /// consumer of [`LeaseMask::any_held`] the perf-val T6 review found
    /// missing ("nothing in production consumes `any_held()` yet"). `mask`
    /// is the cell's per-cell lease pool (CONTRACTS C4: "per-cell atomic u64
    /// bitmask"); `leases` is every currently outstanding [`HarvestLease`]
    /// drawn from it that the CALLER is tracking — this crate does not
    /// itself maintain that set (see the ownership-gap paragraph below).
    /// Revokes whichever of `leases` are overdue (delegating to
    /// [`Self::revoke_overdue`], which now force-releases the slot
    /// immediately) and reports whether a frame boundary may run
    /// `compact`/`gpu::RetiredPhase::compact_gated` against this cell THIS
    /// boundary.
    ///
    /// - `mask.any_held() == false` after the revoke pass (nothing was
    ///   outstanding; or every outstanding lease was just revoked by this
    ///   call; or an earlier boundary already revoked it and its holder has
    ///   since dropped) -> `true`. This single check covers BOTH the
    ///   ordinary "no lease held" case and the overdue-revoke-then-proceed
    ///   case from the very same call — §9.2.1's "compaction proceeds on the
    ///   primary layout immediately".
    /// - `mask.any_held() == true` (at least one lease is held AND was not
    ///   overdue, so `revoke_overdue` left it alone) -> `false`: defer
    ///   compaction this boundary — unchanged from Test 10's existing
    ///   `any_held()`-gated behavior ("drop -> `!any_held()` -> compaction
    ///   may proceed").
    ///
    /// **Safety argument for proceeding despite a lease revoked THIS call**
    /// (its holder may still be mid-query, unaware it was revoked): the
    /// holder's in-flight reads run against a
    /// [`crate::snapshot::LivenessSnapshot`] pinned at (or after)
    /// acquisition — never the live `LivenessMask` and never live page rows
    /// directly — and per §9.2.1 any positional token it acts on afterward
    /// is expected to be re-validated against live generations before use
    /// ([`revalidate_run`]). So compaction moving rows underneath the
    /// snapshot cannot tear a read the holder is mid-way through (the
    /// snapshot is an owned, immutable copy — compaction never touches it),
    /// and cannot cause the holder to silently act on a moved/reused row
    /// (revalidation catches it). **This argument holds only as long as
    /// every read path reachable from a held lease goes through the pinned
    /// snapshot and never the live mask/page directly** — true for every
    /// query path this crate ships today (`SpatialCell::query_aabb_in`/
    /// `query_frustum_in` both take an explicit `liveness_words: &[u64]`
    /// argument and never reach into the live `LivenessMask` themselves; see
    /// this task's report for the full sweep). A FUTURE read path that
    /// bypassed the snapshot and read the live mask/page instead would break
    /// this argument — a real hazard to watch for, not one this gate can
    /// detect on its own.
    ///
    /// **Ownership gap, documented rather than papered over:** neither
    /// `gpu::CellSlot` nor `gpu::SceneGpuStore` carries a `LeaseMask` or an
    /// outstanding-lease set today — leases are a caller-owned object,
    /// per-cell only BY CONVENTION (CONTRACTS C4 says "per-cell", but
    /// nothing in this crate's types enforces a 1:1 `CellId`<->`LeaseMask`
    /// binding). `compaction_ready` is therefore a pure function of whatever
    /// `(mask, leases)` pair the caller supplies for a given cell. Wiring
    /// "one `LeaseMask` per registered `CellId`, tracked by the store
    /// itself and threaded automatically through the boundary" is a real
    /// architecture change (would touch `CellSlot`, `register_cell`, and
    /// every existing call site constructing a bare `CellSlot { id, cell }`)
    /// that this task deliberately does NOT make — see the M3-b T2 report.
    /// That driver-level wiring is M4 World-driver scope.
    #[must_use]
    pub fn compaction_ready(
        &self,
        mask: &LeaseMask,
        leases: &[&HarvestLease<'_>],
        now_ms: f64,
        budget_ms: f64,
    ) -> bool {
        if mask.any_held() {
            self.revoke_overdue(leases, now_ms, budget_ms);
        }
        !mask.any_held()
    }

    /// Multi-view harvest (spec §8.4): scan every `(cell, region_base, class)`
    /// against every `view`, routing each view's hits into its OWN staging
    /// array — one [`Scratchpad`] and one [`HarvestStaging`] PER VIEW, never
    /// shared across views. `pads`/`stagings` are indexed in lockstep with
    /// `views` (`pads[v]`/`stagings[v]` back `views[v]`); a mismatched length
    /// is a caller bug, asserted at entry rather than silently truncated or
    /// index-panicking mid-scan.
    ///
    /// §8.4's safety claim: because [`Self::harvest_cell`] takes `&self` (this
    /// pipeline holds no state) and only `&SpatialCell` (read-only — every
    /// per-cell mutation path takes `&mut SpatialCell` and is unreachable from
    /// here), queries over different views have no shared mutable state to
    /// race on and MAY run on separate threads, each with its own
    /// scratch/staging pair, over the SAME cell references. This method
    /// itself is a sequential (single-thread) driver over that same call —
    /// the concurrency claim is exercised directly by
    /// `concurrent_views_match_sequential` in `tests/gpu_harvest.rs`, not by
    /// this function.
    ///
    /// **C3/C4 freshness contract:** each `region_base` in `cells` must be
    /// re-resolved in the ISSUING frame via `grid.gpu_id(coord)` →
    /// `store.row_region_base(id)` — never cached across a frame boundary.
    /// Transitions execute only at boundaries (C3), so a base resolved this
    /// frame is stable through Harvest; a base cached across a boundary that
    /// evicted and re-promoted the cell into a different region would emit
    /// wrong global-row tokens SILENTLY (a stale `CellId` fails loud; a stale
    /// `u32` does not). The World driver owns this chain (M4).
    pub fn harvest_views(
        &self,
        cells: &[(&SpatialCell, u32 /* region_base */, MeshClass)],
        views: &[View],
        pads: &mut [Scratchpad],
        stagings: &mut [HarvestStaging],
        _h: &HarvestPhase,
    ) {
        assert_eq!(views.len(), pads.len(), "one Scratchpad per view (§8.4)");
        assert_eq!(views.len(), stagings.len(), "one HarvestStaging per view (§8.4)");
        for v in 0..views.len() {
            for &(cell, region_base, class) in cells {
                self.harvest_cell(cell, region_base, class, &views[v], &mut pads[v], &mut stagings[v], _h);
            }
        }
    }
}

impl Default for HarvestPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Stale-validation lane (spec §9.2.1): re-validate a positional token `run`
/// against `cell`'s LIVE liveness mask (NOT any pinned snapshot — that is the
/// point), writing [`NULL_ROW`] over any token whose row has since died.
/// Returns the surviving (still-live) count.
///
/// This is the recovery half of a revoked lease: the holder queried against a
/// [`LivenessSnapshot`] that is intentionally pinned (§9.2.1 double-buffered
/// state — a revoked reader must not see its OWN in-flight read torn), so its
/// `run` may reference rows that have died (freed, or freed-and-reused by a
/// different object) since capture. `revalidate_run` is how the holder
/// reconciles before acting on those tokens.
///
/// **C4 frame-scoped caveat:** liveness alone cannot distinguish "this row
/// died and stayed dead" from "this row died AND was compacted away AND its
/// slot was reused this frame by an unrelated allocation" — both look
/// identical to a bare `is_live` check (the reused row reads live again, just
/// as the wrong object). This lane only recovers from revocation WITHIN the
/// issuing frame, before any compaction/reuse could occur (the harvest
/// sub-phase is read-only, §8/C4); it is not a general cross-frame
/// staleness fix. A `run` carried across a frame boundary needs a fresh
/// query, not `revalidate_run`.
///
/// **HAZARD:** operates on positional LOCAL token runs (`query_*_in` output)
/// ONLY — never feed it global tokens from [`HarvestStaging`]; a global
/// (region-offset) token would misindex the cell's liveness words (no bounds
/// check) or silently check the wrong row.
pub fn revalidate_run(cell: &SpatialCell, run: &mut [u32]) -> u32 {
    let liveness = cell.storage().liveness();
    let mut survivors = 0u32;
    for tok in run.iter_mut() {
        if *tok == NULL_ROW {
            continue;
        }
        if liveness.is_live(*tok) {
            survivors += 1;
        } else {
            *tok = NULL_ROW;
        }
    }
    survivors
}
