//! Concentric streaming grid — pure logic (design Rev 2 §4, spec §5/§5.3/§5.5).
//!
//! Classifies each tracked cell into a residency [`Domain`] (`Outer` →
//! `Margin` → `Inner`) from the observer set, with §5.5 hysteresis to damp
//! boundary jitter, and tracks a per-cell cross-fade `alpha` (§5.2).
//! Classification itself (`classify`, `commit_transition`, `advance_crossfade`)
//! stays PURE LOGIC: it decides *what* should transition and queues the
//! decision as a [`Transition`], touching neither `SceneGpuStore` nor wgpu.
//! [`execute_transitions`] (M2b-β T5) is this module's one exception: it
//! drains [`StreamingGrid::take_transitions`] and wires each transition to
//! `SceneGpuStore::register_cell`/`unregister_cell`, and
//! [`StreamingGrid::write_cell_metadata`] packs the per-cell α/domain SSBO
//! straight from wgpu — both live here because the grid's committed
//! domain/α state is exactly what they read. This module lives under `gpu`
//! because it depends on [`super::RegionClassConfig`] for the budget check
//! below and, now, on the store/wgpu types those two items touch.
//!
//! ## β simplification: grid is XZ-planar
//!
//! A cell's bounds are unbounded on Y (`[-inf, inf]`): observer altitude
//! never affects classification. Spec §5 allows a `[min_y, max_y]` cell
//! extent; that's out of scope for β and is a documented simplification, not
//! an oversight.
//!
//! ## Classification: the hysteresis band machine (authoritative)
//!
//! A cell's **base bounds** are its world AABB from `coord × cell_width`
//! (XZ only, Y unbounded — see above). All AABB tests below use **closed**
//! intervals: touching faces count as intersecting (crate-wide §8.2
//! discipline; see `spatial.rs`). Let `pad = pad_fraction × cell_width`
//! (§5.5 Δpad) and `hyst = hysteresis` (§5.5 δhyst).
//!
//! Per cell, four concentric zones are derived from the base bounds, each
//! tested for intersection against the observer union (any-of):
//!
//! | zone             | base grown by                | role                   |
//! |------------------|------------------------------|------------------------|
//! | `inner_promote`  | `pad`                        | Margin→Inner trigger   |
//! | `inner_demote`   | `pad + hyst`                 | Inner→Margin hold zone |
//! | `margin_promote` | `margin_radius + pad`        | Outer→Margin trigger   |
//! | `margin_demote`  | `margin_radius + pad + hyst` | Margin→Outer hold zone |
//!
//! Transition rules — **at most one step per `classify` call**, evaluated
//! from the cell's *committed* domain only:
//!
//! - `Outer`: intersects `margin_promote` → queue `→Margin`. Else nothing.
//! - `Margin`: intersects `inner_promote` → queue `→Inner`; else if NOT
//!   intersecting `margin_demote` → queue `→Outer`; else nothing.
//! - `Inner`: NOT intersecting `inner_demote` → queue `→Margin`. Else
//!   nothing.
//!
//! The promotion boundary stands `pad` proud of the unpadded region edge
//! (§5.5 PromotionBoundary = CellBounds + Δpad: an observer promotes
//! *earlier* than the plain edge), and the demotion boundary stands a
//! further `hyst` beyond that. The gap between them IS the §5.5 hysteresis
//! band: an observer parked (or jittering) anywhere inside it triggers no
//! transition in either direction. Multi-ring promotion (`Outer→Inner`)
//! therefore takes two `classify` calls — one step each — as does the
//! symmetric demotion cascade.
//!
//! **α**: a promoting transition (`to` more resident than `from`) sets
//! `alpha_target = 1.0`; a demoting transition sets `alpha_target = 0.0`
//! (applied in [`StreamingGrid::commit_transition`], never in `classify`).
//! [`StreamingGrid::advance_crossfade`] moves `alpha` linearly toward
//! `alpha_target` by `distance / fade_distance`, clamped to `[0, 1]`.
//!
//! ## Drain-every-boundary contract
//!
//! `classify` never mutates a cell's committed `domain`/`alpha_target` — it
//! only queues [`Transition`]s. The caller MUST drain the queue via
//! [`StreamingGrid::take_transitions`] once per boundary, execute, and
//! report success via [`StreamingGrid::commit_transition`] (a declined
//! transition simply isn't committed; the next `classify` re-evaluates from
//! the unchanged committed state and re-queues it). Calling `classify` with
//! an undrained queue is a contract violation: a stale queued transition
//! could contradict what the newer classification would decide (e.g. a
//! queued `Inner→Margin` surviving a frame in which the observer moved back
//! inside), and the executor would apply it. `classify` debug-asserts the
//! queue is empty, and — belt and braces for release builds — drops any
//! stale queued transition for a cell before queueing that cell's new one,
//! so the queue never holds two transitions for the same coord.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{CellId, RegionClassConfig, RetiredPhase, SceneGpuStore};
use crate::spatial::{Aabb, SpatialCell};

/// §5.5 tunables. `pad_fraction` default is 0.10 (§5.5 Δpad); `hysteresis`
/// is δhyst, additional world units layered on top of the pad for the
/// demotion test only.
#[derive(Clone, Copy, Debug)]
pub struct GridConfig {
    pub cell_width: f32,
    /// World units beyond the inner union that count as `Margin`.
    pub margin_radius: f32,
    /// §5.5 Δpad fraction of `cell_width`; default 0.10.
    pub pad_fraction: f32,
    /// §5.5 δhyst, world units beyond the pad, demotion-only.
    pub hysteresis: f32,
}

/// Dense grid coordinate: cell `(x, z)` spans world
/// `[x * cell_width, (x+1) * cell_width) × [z * cell_width, (z+1) * cell_width)`
/// (Y unbounded — see module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellCoord {
    pub x: i32,
    pub z: i32,
}

/// Residency domain, ordered `Outer < Margin < Inner` (least to most
/// resident). The enum's declared variant order is documentation-only —
/// [`domain_rank`] is the authoritative ordering used for the α-target
/// promotion/demotion distinction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Domain {
    Inner,
    Margin,
    Outer,
}

fn domain_rank(d: Domain) -> u8 {
    match d {
        Domain::Outer => 0,
        Domain::Margin => 1,
        Domain::Inner => 2,
    }
}

/// A single queued domain change for one cell. `from` is the committed
/// domain at queue time; under the drain-every-boundary contract (module
/// docs) it is always the cell's current domain when the executor sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub coord: CellCoord,
    pub from: Domain,
    pub to: Domain,
}

/// §5.3 VRAM budget inputs, checked once at construction (α-audit
/// bounded-extent input: `max_materialized_cells` bounds the HLOD term
/// regardless of how large the world actually is).
#[derive(Clone, Copy, Debug)]
pub struct StreamingBudget {
    pub vram_hlod_budget: u64,
    pub vram_geometry_budget: u64,
    /// Bounded worst-case count of simultaneously materialized cells.
    pub max_materialized_cells: u32,
    pub proxy_mesh_bytes: u64,
    pub mean_cell_geometry_bytes: u64,
}

/// §5.3 budget-validation failures, surfaced at [`StreamingGrid::new`].
#[derive(Debug, PartialEq)]
pub enum BudgetError {
    HlodOverBudget,
    GeometryOverBudget,
}

#[derive(Debug)]
struct GridCellState {
    domain: Domain,
    dense_id: u32,
    alpha: f32,
    alpha_target: f32,
    /// The store-side region assignment while resident (Margin/Inner);
    /// `None` while `Outer`. Set by [`execute_transitions`] at a successful
    /// Outer→Margin promotion, cleared at Margin→Outer eviction — the grid
    /// never allocates or frees this itself, only records what the executor
    /// reports (module docs: `execute_transitions` is the one place this
    /// module touches `SceneGpuStore`).
    gpu_id: Option<CellId>,
}

/// Pure-logic concentric streaming grid — see module docs for the full
/// band-machine/cross-fade contract.
#[derive(Debug)]
pub struct StreamingGrid {
    cfg: GridConfig,
    cells: HashMap<CellCoord, GridCellState>,
    next_dense_id: u32,
    transitions: Vec<Transition>,
    /// Test 13 instrumentation (see `upload_count` below). `write_cell_metadata`
    /// takes `&self` (it only reads `cells`/`next_dense_id`, no mutation), so
    /// this needs interior mutability — `AtomicU64` with `Relaxed` ordering,
    /// unlike the plain `u64` counters on the other asset stores (whose
    /// write methods already take `&mut self`). `Relaxed` is sufficient
    /// because this is monotonic instrumentation only: the count itself
    /// carries no data other stores/threads synchronize on, so there is no
    /// ordering relationship to preserve with any other memory operation.
    upload_count: AtomicU64,
}

impl StreamingGrid {
    /// Validates the §5.3 budget once, up front: `max_materialized_cells ×
    /// proxy_mesh_bytes ≤ vram_hlod_budget` (HLOD/proxy term) and
    /// `(Σ inner_classes.max_resident_cells) × mean_cell_geometry_bytes ≤
    /// vram_geometry_budget` (resident-geometry term).
    pub fn new(
        cfg: GridConfig,
        budget: StreamingBudget,
        inner_classes: &[RegionClassConfig],
    ) -> Result<Self, BudgetError> {
        let hlod_used = budget.max_materialized_cells as u64 * budget.proxy_mesh_bytes;
        if hlod_used > budget.vram_hlod_budget {
            return Err(BudgetError::HlodOverBudget);
        }
        let resident_cells: u64 = inner_classes
            .iter()
            .map(|c| c.max_resident_cells as u64)
            .sum();
        let geometry_used = resident_cells * budget.mean_cell_geometry_bytes;
        if geometry_used > budget.vram_geometry_budget {
            return Err(BudgetError::GeometryOverBudget);
        }
        Ok(Self {
            cfg,
            cells: HashMap::new(),
            next_dense_id: 0,
            transitions: Vec::new(),
            upload_count: AtomicU64::new(0),
        })
    }

    /// Track a content-bearing cell (assigns a dense id, starts `Outer`).
    /// Idempotent: re-materializing an already-tracked coord returns its
    /// existing dense id and leaves its state untouched.
    pub fn materialize(&mut self, coord: CellCoord) -> u32 {
        if let Some(state) = self.cells.get(&coord) {
            return state.dense_id;
        }
        let id = self.next_dense_id;
        self.next_dense_id += 1;
        self.cells.insert(
            coord,
            GridCellState {
                domain: Domain::Outer,
                dense_id: id,
                alpha: 0.0,
                alpha_target: 0.0,
                gpu_id: None,
            },
        );
        id
    }

    pub fn domain(&self, coord: CellCoord) -> Option<Domain> {
        self.cells.get(&coord).map(|s| s.domain)
    }

    pub fn alpha(&self, coord: CellCoord) -> Option<f32> {
        self.cells.get(&coord).map(|s| s.alpha)
    }

    pub fn dense_id(&self, coord: CellCoord) -> Option<u32> {
        self.cells.get(&coord).map(|s| s.dense_id)
    }

    /// The store-side region assignment for a resident cell — `None` for an
    /// `Outer` cell or an untracked coord. Set/cleared by
    /// [`execute_transitions`] only.
    pub fn gpu_id(&self, coord: CellCoord) -> Option<CellId> {
        self.cells.get(&coord).and_then(|s| s.gpu_id)
    }

    /// Executor-only: record (`Some`) or clear (`None`) a cell's store-side
    /// region assignment. A no-op for an untracked coord.
    pub fn set_gpu_id(&mut self, coord: CellCoord, id: Option<CellId>) {
        if let Some(state) = self.cells.get_mut(&coord) {
            state.gpu_id = id;
        }
    }

    /// §5 classification via the §5.5 hysteresis band machine (module docs).
    /// Queues at most one single-step [`Transition`] per cell; applies NO
    /// state change to `domain`/`alpha_target` — that happens only in
    /// [`Self::commit_transition`].
    ///
    /// CONTRACT: the caller must drain [`Self::take_transitions`] every
    /// boundary, before the next `classify` — an undrained queue can hold a
    /// transition the newer observer positions would no longer justify.
    /// Debug builds assert this; release builds additionally self-heal by
    /// evicting any stale queued transition for a cell before queueing that
    /// cell's new one.
    pub fn classify(&mut self, observer_aabbs: &[Aabb]) {
        debug_assert!(
            self.transitions.is_empty(),
            "classify() called with undrained transitions — drain via take_transitions() every boundary"
        );
        let pad = self.cfg.pad_fraction * self.cfg.cell_width;
        let hyst = self.cfg.hysteresis;
        let mr = self.cfg.margin_radius;
        let cell_width = self.cfg.cell_width;

        for (&coord, state) in self.cells.iter() {
            let base = base_bounds(coord, cell_width);
            let to = match state.domain {
                Domain::Outer => {
                    // margin_promote: base + (margin_radius + pad)
                    if any_intersect(&grow(base, mr + pad), observer_aabbs) {
                        Some(Domain::Margin)
                    } else {
                        None
                    }
                }
                Domain::Margin => {
                    // inner_promote: base + pad
                    if any_intersect(&grow(base, pad), observer_aabbs) {
                        Some(Domain::Inner)
                    // margin_demote: base + (margin_radius + pad + hyst)
                    } else if !any_intersect(&grow(base, mr + pad + hyst), observer_aabbs) {
                        Some(Domain::Outer)
                    } else {
                        None // inside the Margin band: hold
                    }
                }
                Domain::Inner => {
                    // inner_demote: base + (pad + hyst)
                    if !any_intersect(&grow(base, pad + hyst), observer_aabbs) {
                        Some(Domain::Margin)
                    } else {
                        None // inside the Inner band (or still inside): hold
                    }
                }
            };
            if let Some(to) = to {
                // Belt and braces for release builds (the debug_assert above
                // is the contract): never leave two queued transitions for
                // the same coord — the newer classification wins.
                self.transitions.retain(|t| t.coord != coord);
                self.transitions.push(Transition { coord, from: state.domain, to });
            }
        }
    }

    /// Drain queued transitions (caller executes them at the boundary).
    pub fn take_transitions(&mut self) -> Vec<Transition> {
        std::mem::take(&mut self.transitions)
    }

    /// Confirm an executed transition (caller reports success/decline by
    /// simply not calling this for a declined one). Sets the cell's domain
    /// to `t.to` and its α target: promotion (`to` more resident than
    /// `from`) → 1.0, demotion → 0.0.
    pub fn commit_transition(&mut self, t: Transition) {
        if let Some(state) = self.cells.get_mut(&t.coord) {
            state.domain = t.to;
            state.alpha_target =
                if domain_rank(t.to) > domain_rank(t.from) { 1.0 } else { 0.0 };
        }
    }

    /// §5.2: advance cross-fade by observer world-distance travelled,
    /// linearly, clamped to `[0, 1]`.
    pub fn advance_crossfade(&mut self, distance: f32, fade_distance: f32) {
        let step = distance / fade_distance;
        for state in self.cells.values_mut() {
            let target = state.alpha_target;
            if target > state.alpha {
                state.alpha = (state.alpha + step).min(target);
            } else if target < state.alpha {
                state.alpha = (state.alpha - step).max(target);
            }
            state.alpha = state.alpha.clamp(0.0, 1.0);
        }
    }

    /// Packs `(f32 alpha, u32 domain)` for every materialized cell into
    /// `buf` at byte offset `dense_id * 8` — the M3 stipple-pass contract.
    /// Domain encoding: `Outer` = 0, `Margin` = 1, `Inner` = 2.
    ///
    /// Simple full rewrite of every materialized entry's 8 bytes on every
    /// call (bounded by `next_dense_id ≤ max_cells_metadata`, §8);
    /// delta-tracking (skipping unchanged entries) is a recorded future
    /// optimization, not built here.
    pub fn write_cell_metadata(&self, queue: &wgpu::Queue, buf: &wgpu::Buffer) {
        let mut data = vec![0u8; self.next_dense_id as usize * 8];
        for state in self.cells.values() {
            let domain_code: u32 = match state.domain {
                Domain::Outer => 0,
                Domain::Margin => 1,
                Domain::Inner => 2,
            };
            let offset = state.dense_id as usize * 8;
            data[offset..offset + 4].copy_from_slice(&state.alpha.to_le_bytes());
            data[offset + 4..offset + 8].copy_from_slice(&domain_code.to_le_bytes());
        }
        assert!(
            data.len() as u64 <= buf.size(),
            "materialized cell count {} needs {} bytes, exceeding the cell-metadata buffer's {} bytes (max_cells_metadata too small)",
            self.next_dense_id,
            data.len(),
            buf.size()
        );
        queue.write_buffer(buf, 0, &data);
        self.upload_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Test 13 instrumentation: the teardown gate asserts these do not move
    /// across the renderer drop/rebind window. `Relaxed` load — see the
    /// `upload_count` field doc for why no stronger ordering is needed.
    #[doc(hidden)]
    pub fn upload_count(&self) -> u64 {
        self.upload_count.load(Ordering::Relaxed)
    }
}

/// Outcome tally for one [`execute_transitions`] call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TransitionStats {
    /// Outer→Margin transitions that succeeded (`register_cell` returned
    /// `Ok`).
    pub promoted: u32,
    /// Margin→Outer transitions executed (`unregister_cell`).
    pub demoted: u32,
    /// Outer→Margin transitions that `register_cell` declined
    /// (`Err(RegionError)`, §8 graceful degradation) — the cell stays in its
    /// current domain and is re-classified on the next `classify` call.
    pub declined: u32,
    /// Drained transitions dropped because the grid's committed domain no
    /// longer matched `t.from` at execution time (T3 reviewer: release-build
    /// stale-queue hole). A dropped transition is safe by construction —
    /// the next `classify()` re-derives intent from committed state.
    pub dropped_stale: u32,
}

/// Boundary transition executor (M2b-β T5): drains
/// [`StreamingGrid::take_transitions`] and applies each against `store` and
/// `cells`, reporting outcomes via [`TransitionStats`].
///
/// - **Outer→Margin**: `store.register_cell(cell.storage(), class_of(coord))`.
///   `Ok(id)` → commit the transition and record `id` via
///   [`StreamingGrid::set_gpu_id`] (`stats.promoted += 1`). `Err(RegionError)`
///   → DECLINE: the transition is not committed (the cell stays in its
///   current domain, grid state unchanged), `stats.declined += 1`, and a
///   `tracing::warn!` records the exhaustion (§8 graceful degradation).
/// - **Margin→Outer**: `store.unregister_cell(id, cell.storage_mut(),
///   eviction_serial)` using the `id` recorded at promotion, then clears it
///   via `set_gpu_id(coord, None)` and commits (`stats.demoted += 1`).
/// - **Margin↔Inner**: commit-only — a domain-flag change with no store
///   interaction.
///
/// `_w: &RetiredPhase` is a witness, not a value read here: it proves the
/// caller has run this frame's `retire` boundary stage (so eviction serials
/// and pending-retire drains are already resolved) before promoting or
/// evicting any cell this boundary.
///
/// Before applying ANY drained transition, its coord's *current* committed
/// domain is checked against `t.from`; a mismatch means the queue held a
/// transition from before some other write invalidated it (T3 reviewer,
/// release-build stale-queue hole) — it is silently dropped and counted in
/// `stats.dropped_stale`. A dropped transition is safe by construction — the
/// next `classify()` re-derives intent from committed state.
///
/// **Trust contract on `class_of`:** its return value is used as-is to index
/// `SceneGpuStore`'s internal per-class region pools (`register_cell`'s
/// `row_pools[class]`/`slot_pools[class]`); this function does not validate
/// it against the store's configured class count. A `class_of` that returns
/// an index outside the range the `SceneGpuStore` was constructed with (its
/// `SceneGpuConfig::classes` length) panics inside `register_cell` on the
/// next Outer→Margin promotion, not here — the caller owns keeping
/// `class_of`'s range in sync with the store's class configuration.
pub fn execute_transitions(
    grid: &mut StreamingGrid,
    store: &mut SceneGpuStore,
    cells: &mut HashMap<CellCoord, SpatialCell>,
    class_of: &dyn Fn(CellCoord) -> usize,
    eviction_serial: u64,
    _w: &RetiredPhase,
) -> TransitionStats {
    let mut stats = TransitionStats::default();
    for t in grid.take_transitions() {
        if grid.domain(t.coord) != Some(t.from) {
            // A dropped transition is safe by construction — the next
            // classify() re-derives intent from committed state.
            stats.dropped_stale += 1;
            continue;
        }
        let cell = cells
            .get_mut(&t.coord)
            .expect("execute_transitions: materialized coord must have a tracked SpatialCell");
        match (t.from, t.to) {
            (Domain::Outer, Domain::Margin) => {
                let class = class_of(t.coord);
                match store.register_cell(cell.storage(), class) {
                    Ok(id) => {
                        grid.set_gpu_id(t.coord, Some(id));
                        grid.commit_transition(t);
                        stats.promoted += 1;
                    }
                    Err(err) => {
                        stats.declined += 1;
                        tracing::warn!(
                            coord = ?t.coord,
                            error = ?err,
                            "region exhausted — declining Outer→Margin promotion; cell stays Outer"
                        );
                    }
                }
            }
            (Domain::Margin, Domain::Outer) => {
                let id = grid
                    .gpu_id(t.coord)
                    .expect("Margin cell must carry a gpu_id assigned at its Outer→Margin promotion");
                store.unregister_cell(id, cell.storage_mut(), eviction_serial);
                grid.set_gpu_id(t.coord, None);
                grid.commit_transition(t);
                stats.demoted += 1;
            }
            _ => {
                // Margin↔Inner: domain-flag change only, no store
                // interaction (module docs).
                grid.commit_transition(t);
            }
        }
    }
    stats
}

/// A cell's world AABB from `coord × cell_width`. XZ-planar (β
/// simplification): Y is unbounded so observer altitude never affects
/// classification.
fn base_bounds(coord: CellCoord, cell_width: f32) -> Aabb {
    let x0 = coord.x as f32 * cell_width;
    let z0 = coord.z as f32 * cell_width;
    Aabb {
        min: [x0, f32::NEG_INFINITY, z0],
        max: [x0 + cell_width, f32::INFINITY, z0 + cell_width],
    }
}

/// Grow an AABB by `r` in every axis (Y stays effectively unbounded: ±inf ±
/// r is still ±inf).
fn grow(a: Aabb, r: f32) -> Aabb {
    Aabb {
        min: [a.min[0] - r, a.min[1] - r, a.min[2] - r],
        max: [a.max[0] + r, a.max[1] + r, a.max[2] + r],
    }
}

/// Closed-interval AABB intersection (crate-wide §8.2 discipline: touching
/// faces count as a hit).
fn aabb_intersect(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] && a.max[i] >= b.min[i])
}

fn any_intersect(region: &Aabb, observers: &[Aabb]) -> bool {
    observers.iter().any(|o| aabb_intersect(region, o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> GridConfig {
        GridConfig { cell_width: 100.0, margin_radius: 150.0, pad_fraction: 0.10, hysteresis: 20.0 }
    }

    fn budget() -> StreamingBudget {
        StreamingBudget {
            vram_hlod_budget: u64::MAX,
            vram_geometry_budget: u64::MAX,
            max_materialized_cells: 1024,
            proxy_mesh_bytes: 1024,
            mean_cell_geometry_bytes: 1 << 20,
        }
    }

    fn observer_at(x: f32) -> Aabb {
        Aabb { min: [x - 10.0, -10.0, -10.0], max: [x + 10.0, 10.0, 10.0] }
    }

    // ── Test 11 gate: threshold derivations (closed intervals) ────────────
    //
    // cfg: cell_width 100, margin_radius 150, pad = 0.10 × 100 = 10,
    // hyst = 20. Cell (5,0): base x ∈ [500, 600]. Observer half-width 10.
    //
    //   margin_promote = base ± (150+10)    = [340, 760] on x
    //     → intersects when center + 10 ≥ 340  ⟺  center ≥ 330
    //     (the UNPADDED margin edge would be [350, 750] ⟺ center ≥ 340:
    //      promotion at center 331 < 340 is possible ONLY because Δpad
    //      advanced the boundary — §5.5 PromotionBoundary = bounds + Δpad)
    //   margin_demote  = base ± (150+10+20) = [320, 780] on x
    //     → holds while center + 10 ≥ 320  ⟺  center ≥ 310;
    //       demotes when center < 310
    //   inner_promote  = base ± 10          = [490, 610] on x
    //     → needs center ≥ 480; never touched by these positions
    //
    //   ⇒ Margin band (cell held Margin, zero transitions either way):
    //     center ∈ [310, 330). Jitter range [312, 328] sits inside it.

    #[test]
    fn test11_pad_advances_promotion_band_holds_and_hysteresis_delays_demotion() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let far = CellCoord { x: 5, z: 0 }; // base x ∈ [500, 600]
        g.materialize(far);

        // (a) Just below the padded promote threshold (center 329 < 330):
        // no transition.
        g.classify(&[observer_at(329.0)]);
        assert!(g.take_transitions().is_empty(), "329 < padded threshold 330: no promotion");
        assert_eq!(g.domain(far), Some(Domain::Outer));

        // (a) Just past it (center 331 ≥ 330, yet well short of the UNPADDED
        // threshold 340): promotes — proof that Δpad advances the boundary.
        g.classify(&[observer_at(331.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts.len(), 1, "exactly one transition at the padded boundary");
        assert_eq!(ts[0], Transition { coord: far, from: Domain::Outer, to: Domain::Margin });
        g.commit_transition(ts[0]);

        // (b) Jitter inside the band [312, 328] — past the demote-hold
        // threshold (310), short of the promote threshold (330): the §5.5
        // band must hold with ZERO transitions in either direction.
        for i in 0..200 {
            let center = 320.0 + ((i % 9) as f32 - 4.0) * 2.0; // ∈ [312, 328]
            g.classify(&[observer_at(center)]);
            assert!(
                g.take_transitions().is_empty(),
                "band frame {i} (center {center}) caused a transition"
            );
        }
        assert_eq!(g.domain(far), Some(Domain::Margin), "held Margin through the band");

        // (c) Retreat past the demotion boundary (center 305 < 310):
        // exactly one demotion — hysteresis delayed it 20 units beyond
        // where the padded promote boundary sits.
        g.classify(&[observer_at(305.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0], Transition { coord: far, from: Domain::Margin, to: Domain::Outer });
    }

    #[test]
    fn test11_decisive_crossing_promotes_exactly_once_and_demotion_lags_by_hysteresis() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let far = CellCoord { x: 5, z: 0 }; // cell spanning x ∈ [500, 600]
        g.materialize(far);
        g.classify(&[observer_at(0.0)]);
        assert!(g.take_transitions().is_empty());
        assert_eq!(g.domain(far), Some(Domain::Outer));
        // Decisive move deep into margin range of the far cell:
        g.classify(&[observer_at(480.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts.len(), 1, "exactly one transition — single step, no skip past Margin");
        assert_eq!(ts[0], Transition { coord: far, from: Domain::Outer, to: Domain::Margin });
        g.commit_transition(ts[0]);
        // Retreat to just inside the demotion boundary → NO demotion (hysteresis):
        g.classify(&[observer_at(480.0 - cfg().hysteresis + 1.0)]);
        assert!(g.take_transitions().is_empty(), "inside hysteresis band: no demotion");
        // Retreat past it → demotion:
        g.classify(&[observer_at(300.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].to, Domain::Outer);
    }

    #[test]
    fn cascade_promotes_one_step_per_classify_as_observer_converges() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let c = CellCoord { x: 0, z: 0 }; // base x ∈ [0, 100]
        g.materialize(c);
        // Far away: stays Outer.
        g.classify(&[observer_at(500.0)]);
        assert!(g.take_transitions().is_empty());
        // Call N — converge into margin_promote ([-160, 260] ⟺ center ≤ 270):
        // ONE step only, Outer→Margin — no skip past Margin even though the
        // observer will keep closing.
        g.classify(&[observer_at(200.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts, vec![Transition { coord: c, from: Domain::Outer, to: Domain::Margin }]);
        g.commit_transition(ts[0]);
        // Call N+1 — converge into inner_promote ([-10, 110] ⟺ center ≤ 120):
        // second step, Margin→Inner.
        g.classify(&[observer_at(105.0)]);
        let ts = g.take_transitions();
        assert_eq!(ts, vec![Transition { coord: c, from: Domain::Margin, to: Domain::Inner }]);
        g.commit_transition(ts[0]);
        assert_eq!(g.domain(c), Some(Domain::Inner));
    }

    #[test]
    #[should_panic(expected = "undrained")]
    fn classify_with_undrained_transitions_panics_in_debug() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        g.materialize(CellCoord { x: 0, z: 0 });
        g.classify(&[observer_at(50.0)]); // queues Outer→Margin
        g.classify(&[observer_at(50.0)]); // undrained — contract violation
    }

    #[test]
    fn budget_violation_fails_construction() {
        let mut b = budget();
        b.vram_hlod_budget = 10; // 1024 cells × 1 KiB proxies ≫ 10 bytes
        assert_eq!(StreamingGrid::new(cfg(), b, &[]).unwrap_err(), BudgetError::HlodOverBudget);
    }

    #[test]
    fn geometry_budget_violation_fails_construction() {
        let b = budget();
        let classes = [RegionClassConfig { capacity: 64, max_resident_cells: 10 }];
        // 10 resident cells × 1 MiB (mean_cell_geometry_bytes) ≫ this tiny cap.
        let mut b2 = b;
        b2.vram_geometry_budget = 1024;
        assert_eq!(
            StreamingGrid::new(cfg(), b2, &classes).unwrap_err(),
            BudgetError::GeometryOverBudget
        );
    }

    #[test]
    fn crossfade_advances_by_world_distance_and_clamps() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let c = CellCoord { x: 0, z: 0 };
        g.materialize(c);
        g.classify(&[observer_at(50.0)]);
        for t in g.take_transitions() {
            g.commit_transition(t);
        }
        // Now heading resident (Outer→Margin promotion): α target 1.
        g.advance_crossfade(25.0, 100.0);
        assert!((g.alpha(c).unwrap() - 0.25).abs() < 1e-6);
        g.advance_crossfade(1000.0, 100.0);
        assert_eq!(g.alpha(c).unwrap(), 1.0, "clamped");
    }

    #[test]
    fn materialize_is_idempotent_and_starts_outer_with_zero_alpha() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let c = CellCoord { x: 3, z: -2 };
        let id_a = g.materialize(c);
        let id_b = g.materialize(c); // re-materialize: same coord
        assert_eq!(id_a, id_b, "re-materializing returns the existing dense id");
        assert_eq!(g.domain(c), Some(Domain::Outer));
        assert_eq!(g.alpha(c), Some(0.0));
        // A second, distinct cell gets a distinct dense id.
        let other = g.materialize(CellCoord { x: 3, z: -1 });
        assert_ne!(id_a, other);
        // Untracked coord: everything reads back None.
        let untracked = CellCoord { x: 99, z: 99 };
        assert_eq!(g.domain(untracked), None);
        assert_eq!(g.alpha(untracked), None);
        assert_eq!(g.dense_id(untracked), None);
    }

    #[test]
    fn far_cell_with_no_observers_stays_outer() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let c = CellCoord { x: 40, z: 40 };
        g.materialize(c);
        g.classify(&[]);
        assert!(g.take_transitions().is_empty());
        assert_eq!(g.domain(c), Some(Domain::Outer));
    }
}
