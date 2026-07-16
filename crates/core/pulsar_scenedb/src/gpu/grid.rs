//! Concentric streaming grid — pure logic (design Rev 2 §4, spec §5/§5.3/§5.5).
//!
//! Classifies each tracked cell into a residency [`Domain`] (`Outer` →
//! `Margin` → `Inner`) from the observer set, with §5.5 hysteresis to damp
//! boundary jitter, and tracks a per-cell cross-fade `alpha` (§5.2). This
//! module is PURE LOGIC: it decides *what* should transition and queues the
//! decision as a [`Transition`]; it never touches `SceneGpuStore` or wgpu.
//! The executor that drains [`StreamingGrid::take_transitions`] against the
//! GPU store is the next task (M2b-β T4) — this module lives under `gpu`
//! because that executor is its only intended caller and it depends on
//! [`super::RegionClassConfig`] for the budget check below.
//!
//! ## β simplification: grid is XZ-planar
//!
//! A cell's bounds are unbounded on Y (`[-inf, inf]`): observer altitude
//! never affects classification. Spec §5 allows a `[min_y, max_y]` cell
//! extent; that's out of scope for β and is a documented simplification, not
//! an oversight.
//!
//! ## Classification semantics (authoritative — read before editing)
//!
//! A cell's **base bounds** are its world AABB from `coord × cell_width`
//! (XZ only, Y unbounded — see above). All AABB tests below use **closed**
//! intervals: touching faces count as intersecting (crate-wide §8.2
//! discipline; see `spatial.rs`).
//!
//! **Plain target** (no pad, used only to decide *which way* a cell wants to
//! move, never to gate the move itself):
//! - `Inner` if the base bounds intersect the observer union (any-of).
//! - else `Margin` if the base bounds **grown by `margin_radius`** intersect
//!   the observer union.
//! - else `Outer`.
//!
//! **Hysteresis-gated commit** (§5.5 — this is what actually fires a
//! [`Transition`]):
//! - To **promote** toward a domain `D` (`Outer→Margin` or `Margin→Inner`),
//!   the observer union must intersect `D`'s own region
//!   (`Inner` region = base bounds; `Margin` region = base bounds grown by
//!   `margin_radius`) **grown further by `pad`** (`pad = pad_fraction ×
//!   cell_width`). This is strictly *decisive* (this task's Test 11 depends
//!   on the promotion boundary standing pad-units proud of the plain
//!   boundary tested above).
//! - To **demote** away from the currently-held domain, the observer union
//!   must fall entirely *outside* the held domain's own region grown by
//!   `pad + hysteresis`; if it still intersects at that larger size, the
//!   cell holds (no demotion at all — this is the flap-damping band).
//!
//! **Promotion may skip a domain in one `classify` call** — e.g.
//! `Outer → Inner` directly, when the plain target is already `Inner` and
//! the `Inner` promotion test is decisive. Spec explicitly allows this
//! ("Outer→Margin→Inner across two boundaries is fine and simpler"): it is
//! *simpler* to jump straight to the decisive target than to force an
//! artificial one-frame dwell in `Margin`. **Demotion is always exactly one
//! step** (`Inner→Margin` or `Margin→Outer`), even if the plain target has
//! fallen further than that — gradual eviction, not a violent one-frame
//! drop. At most one [`Transition`] is queued per cell per `classify` call.
//!
//! **α**: a promoting transition (`to` more resident than `from`) sets
//! `alpha_target = 1.0`; a demoting transition sets `alpha_target = 0.0`.
//! [`StreamingGrid::advance_crossfade`] moves `alpha` linearly toward
//! `alpha_target` by `distance / fade_distance`, clamped to `[0, 1]`.
//!
//! `classify` never mutates a cell's committed `domain`/`alpha_target` — it
//! only queues [`Transition`]s. The caller (executor) drains them via
//! [`StreamingGrid::take_transitions`], performs the GPU-side work, and
//! reports the outcome via [`StreamingGrid::commit_transition`] (only on
//! success — a declined transition simply isn't committed, and the next
//! `classify` will re-evaluate from the unchanged committed state and queue
//! it again). A caller that calls `classify` more than once without ever
//! draining the queue will see the same [`Transition`] pushed again each
//! call (the queue only ever grows by `push`, `take_transitions` drains it)
//! — a documented simplification; every test in this module drains the
//! queue every `classify` call, which is the intended usage.

use std::collections::HashMap;

use super::RegionClassConfig;
use crate::spatial::Aabb;

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
/// [`domain_rank`] is the authoritative ordering used by the classifier.
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

/// A single queued domain change for one cell. `from` is the domain held at
/// queue time (not necessarily still current if multiple transitions were
/// queued without being drained — see module docs).
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
}

/// Pure-logic concentric streaming grid — see module docs for the full
/// classification/hysteresis/cross-fade contract.
#[derive(Debug)]
pub struct StreamingGrid {
    cfg: GridConfig,
    cells: HashMap<CellCoord, GridCellState>,
    next_dense_id: u32,
    transitions: Vec<Transition>,
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
            GridCellState { domain: Domain::Outer, dense_id: id, alpha: 0.0, alpha_target: 0.0 },
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

    /// §5 classification with §5.5 hysteresis (see module docs for the full
    /// semantics). Queues at most one [`Transition`] per cell; applies NO
    /// state change to `domain`/`alpha_target` — that happens only in
    /// [`Self::commit_transition`].
    pub fn classify(&mut self, observer_aabbs: &[Aabb]) {
        let pad = self.cfg.pad_fraction * self.cfg.cell_width;
        let hyst = self.cfg.hysteresis;
        let margin_radius = self.cfg.margin_radius;
        let cell_width = self.cfg.cell_width;

        for (&coord, state) in self.cells.iter_mut() {
            let current = state.domain;
            let base = base_bounds(coord, cell_width);
            let target = plain_target(base, margin_radius, observer_aabbs);
            if target == current {
                continue;
            }

            let new_domain = if domain_rank(target) > domain_rank(current) {
                // Promotion: jump as far toward `target` as decisively
                // supported, but never past it, and never skip past a step
                // whose own decisive test hasn't been checked.
                if target == Domain::Inner
                    && decisive(Domain::Inner, base, margin_radius, pad, observer_aabbs)
                {
                    Domain::Inner
                } else if current == Domain::Outer
                    && decisive(Domain::Margin, base, margin_radius, pad, observer_aabbs)
                {
                    Domain::Margin
                } else {
                    current
                }
            } else {
                // Demotion: exactly one step, gated on the CURRENTLY-held
                // domain's own region grown by pad + hysteresis.
                let region = region_bounds(current, base, margin_radius);
                let grown = grow(region, pad + hyst);
                if !any_intersect(&grown, observer_aabbs) {
                    step_down(current)
                } else {
                    current
                }
            };

            if new_domain != current {
                self.transitions.push(Transition { coord, from: current, to: new_domain });
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
}

fn step_down(d: Domain) -> Domain {
    match d {
        Domain::Inner => Domain::Margin,
        Domain::Margin => Domain::Outer,
        Domain::Outer => Domain::Outer,
    }
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

/// The domain's own reference region, ungrown by pad/hysteresis: `Inner` is
/// the base bounds; `Margin` is the base bounds grown by `margin_radius`.
/// Never called for `Outer` (it has no bounded region).
fn region_bounds(d: Domain, base: Aabb, margin_radius: f32) -> Aabb {
    match d {
        Domain::Inner => base,
        Domain::Margin => grow(base, margin_radius),
        Domain::Outer => base,
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

/// The plain (no pad) target domain: `Inner` if the base bounds intersect
/// any observer; else `Margin` if the base bounds grown by `margin_radius`
/// do; else `Outer`. This decides *direction* only — it never gates a
/// transition by itself (see [`decisive`]).
fn plain_target(base: Aabb, margin_radius: f32, observers: &[Aabb]) -> Domain {
    if any_intersect(&base, observers) {
        Domain::Inner
    } else if any_intersect(&grow(base, margin_radius), observers) {
        Domain::Margin
    } else {
        Domain::Outer
    }
}

/// The §5.5 promotion test: does the observer union intersect `domain`'s
/// own region grown by `pad`?
fn decisive(domain: Domain, base: Aabb, margin_radius: f32, pad: f32, observers: &[Aabb]) -> bool {
    let region = region_bounds(domain, base, margin_radius);
    any_intersect(&grow(region, pad), observers)
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

    #[test]
    fn test11_subpad_jitter_causes_zero_transitions() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        g.materialize(CellCoord { x: 0, z: 0 });
        g.materialize(CellCoord { x: 1, z: 0 });
        // Park the observer just past cell 0's edge toward cell 1, then jitter
        // within the 10-unit pad (10% of 100).
        g.classify(&[observer_at(95.0)]);
        for t in g.take_transitions() {
            g.commit_transition(t);
        }
        let settled: Vec<_> = [CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }]
            .iter()
            .map(|c| g.domain(*c).unwrap())
            .collect();
        for i in 0..200 {
            let jitter = ((i % 7) as f32 - 3.0) * 1.0; // ±3 units — sub-pad
            g.classify(&[observer_at(95.0 + jitter)]);
            assert!(g.take_transitions().is_empty(), "jitter frame {i} caused a transition");
        }
        let after: Vec<_> = [CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }]
            .iter()
            .map(|c| g.domain(*c).unwrap())
            .collect();
        assert_eq!(settled, after, "domains unchanged under sub-pad jitter");
    }

    #[test]
    fn test11_decisive_crossing_promotes_exactly_once_and_demotion_lags_by_hysteresis() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let far = CellCoord { x: 5, z: 0 }; // cell spanning x ∈ [500, 600)
        g.materialize(far);
        g.classify(&[observer_at(0.0)]);
        for t in g.take_transitions() {
            g.commit_transition(t);
        }
        assert_eq!(g.domain(far), Some(Domain::Outer));
        // Decisive move into margin range of the far cell:
        g.classify(&[observer_at(480.0)]); // 150-unit margin reach + pad covers [500,600)
        let ts = g.take_transitions();
        assert_eq!(ts.len(), 1, "exactly one transition");
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
    fn budget_violation_fails_construction() {
        let mut b = budget();
        b.vram_hlod_budget = 10; // 1024 cells × 1 KiB proxies ≫ 10 bytes
        assert_eq!(StreamingGrid::new(cfg(), b, &[]).unwrap_err(), BudgetError::HlodOverBudget);
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
        // Now heading resident: α target 1.
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
    fn far_cell_with_no_observers_stays_outer() {
        let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
        let c = CellCoord { x: 40, z: 40 };
        g.materialize(c);
        g.classify(&[]);
        assert!(g.take_transitions().is_empty());
        assert_eq!(g.domain(c), Some(Domain::Outer));
    }
}
