use crate::{
    PageDemand, PageKey, PlanetId, TerrainRequestClass, TerrainRequestOutcome, TerrainRuntimeError,
    TerrainRuntimeHandle, TerrainStreamingPlan,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainResidencyConfig {
    /// Maximum number of pages in one stable visible/prefetch set.
    pub max_active_pages: usize,
    /// Maximum union of the committed and replacement sets during handoff.
    pub max_transition_pages: usize,
    /// Maximum runtime submissions performed by one reconcile call.
    pub max_requests_per_reconcile: usize,
    pub visible_deadline_ticks: u64,
    pub prefetch_deadline_ticks: u64,
}

impl Default for TerrainResidencyConfig {
    fn default() -> Self {
        Self {
            max_active_pages: 2_048,
            max_transition_pages: 4_096,
            max_requests_per_reconcile: 64,
            visible_deadline_ticks: 1,
            prefetch_deadline_ticks: 8,
        }
    }
}

impl TerrainResidencyConfig {
    fn validate(self) -> Result<Self, TerrainResidencyError> {
        if self.max_active_pages == 0 {
            return Err(TerrainResidencyError::InvalidConfig(
                "max_active_pages must be non-zero",
            ));
        }
        if self.max_transition_pages < self.max_active_pages {
            return Err(TerrainResidencyError::InvalidConfig(
                "max_transition_pages must cover max_active_pages",
            ));
        }
        if self.max_requests_per_reconcile == 0 {
            return Err(TerrainResidencyError::InvalidConfig(
                "max_requests_per_reconcile must be non-zero",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainResidencyCounters {
    pub plans_seen: u64,
    pub plans_superseded: u64,
    pub requests_queued: u64,
    pub requests_coalesced: u64,
    pub pages_current: u64,
    pub requests_deferred: u64,
    pub pages_evicted: u64,
    pub handoffs_committed: u64,
    pub active_page_high_water: usize,
    pub transition_page_high_water: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainResidencyReport {
    pub submitted: usize,
    pub queued: usize,
    pub coalesced: usize,
    pub current: usize,
    pub deferred: usize,
    pub evicted: usize,
    pub ready_pages: usize,
    pub desired_pages: usize,
    pub committed_pages: usize,
    pub handoff_committed: bool,
}

#[derive(Debug, Error)]
pub enum TerrainResidencyError {
    #[error("invalid terrain residency configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("residency session belongs to planet {session:?}, not plan planet {plan:?}")]
    PlanetMismatch { session: PlanetId, plan: PlanetId },
    #[error("plan contains {pages} pages but the active-page budget is {capacity}")]
    ActivePageBudget { pages: usize, capacity: usize },
    #[error("handoff requires {pages} pages but the transition budget is {capacity}")]
    TransitionPageBudget { pages: usize, capacity: usize },
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
}

#[derive(Clone, Debug)]
struct PendingPlan {
    demands: Vec<PageDemand>,
    keys: BTreeSet<PageKey>,
}

/// Reconciles complete deterministic demand plans with the asynchronous page
/// runtime. The previously committed set remains resident until every page in
/// its replacement is ready, preventing a camera teleport from exposing a
/// partially materialized LOD set.
pub struct TerrainResidencySession {
    planet_id: PlanetId,
    config: TerrainResidencyConfig,
    committed: BTreeMap<PageKey, TerrainRequestClass>,
    pending: Option<PendingPlan>,
    counters: TerrainResidencyCounters,
}

impl TerrainResidencySession {
    pub fn new(
        planet_id: PlanetId,
        config: TerrainResidencyConfig,
    ) -> Result<Self, TerrainResidencyError> {
        Ok(Self {
            planet_id,
            config: config.validate()?,
            committed: BTreeMap::new(),
            pending: None,
            counters: TerrainResidencyCounters::default(),
        })
    }

    pub fn planet_id(&self) -> PlanetId {
        self.planet_id
    }

    pub fn committed_pages(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.committed.keys().copied()
    }

    pub fn pending_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.pending
            .as_ref()
            .into_iter()
            .flat_map(|pending| pending.keys.iter().copied())
    }

    pub fn counters(&self) -> TerrainResidencyCounters {
        self.counters
    }

    pub fn reconcile(
        &mut self,
        runtime: &TerrainRuntimeHandle,
        plan: &TerrainStreamingPlan,
        tick: u64,
    ) -> Result<TerrainResidencyReport, TerrainResidencyError> {
        if plan.planet_id() != self.planet_id {
            return Err(TerrainResidencyError::PlanetMismatch {
                session: self.planet_id,
                plan: plan.planet_id(),
            });
        }
        if plan.demands().len() > self.config.max_active_pages {
            return Err(TerrainResidencyError::ActivePageBudget {
                pages: plan.demands().len(),
                capacity: self.config.max_active_pages,
            });
        }
        let desired_keys = plan
            .demands()
            .iter()
            .map(|demand| demand.page_key())
            .collect::<BTreeSet<_>>();
        let transition_pages = self
            .committed
            .keys()
            .copied()
            .chain(desired_keys.iter().copied())
            .collect::<BTreeSet<_>>()
            .len();
        if transition_pages > self.config.max_transition_pages {
            return Err(TerrainResidencyError::TransitionPageBudget {
                pages: transition_pages,
                capacity: self.config.max_transition_pages,
            });
        }

        let is_new_plan = self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.demands != plan.demands());
        let mut report = TerrainResidencyReport {
            desired_pages: desired_keys.len(),
            committed_pages: self.committed.len(),
            ..TerrainResidencyReport::default()
        };
        let committed_matches = self.committed.len() == plan.demands().len()
            && plan.demands().iter().all(|demand| {
                self.committed.get(&demand.page_key()) == Some(&demand.request_class())
            });
        if self.pending.is_none() && committed_matches {
            report.ready_pages = desired_keys.len();
            return Ok(report);
        }
        if is_new_plan {
            self.counters.plans_seen = self.counters.plans_seen.saturating_add(1);
            if let Some(previous) = self.pending.take() {
                self.counters.plans_superseded = self.counters.plans_superseded.saturating_add(1);
                for key in previous.keys.difference(&desired_keys).copied() {
                    if !self.committed.contains_key(&key)
                        && runtime.evict_page(self.planet_id, key)?
                    {
                        report.evicted += 1;
                    }
                }
            }
            self.pending = Some(PendingPlan {
                demands: plan.demands().to_vec(),
                keys: desired_keys.clone(),
            });
        }

        self.counters.active_page_high_water =
            self.counters.active_page_high_water.max(desired_keys.len());
        self.counters.transition_page_high_water = self
            .counters
            .transition_page_high_water
            .max(transition_pages);

        let resident_before = runtime
            .resident_page_keys(self.planet_id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let pending_demands = self
            .pending
            .as_ref()
            .expect("a valid plan always establishes pending demand")
            .demands
            .clone();
        for demand in pending_demands
            .iter()
            .filter(|demand| !resident_before.contains(&demand.page_key()))
            .take(self.config.max_requests_per_reconcile)
        {
            let deadline_offset = match demand.request_class() {
                TerrainRequestClass::Prefetch => self.config.prefetch_deadline_ticks,
                TerrainRequestClass::Visible
                | TerrainRequestClass::Collision
                | TerrainRequestClass::EditResponse => self.config.visible_deadline_ticks,
            };
            report.submitted += 1;
            match runtime.request_page(
                self.planet_id,
                demand.page_key(),
                demand.request_class(),
                tick.saturating_add(deadline_offset),
            ) {
                Ok(TerrainRequestOutcome::Queued { .. }) => {
                    report.queued += 1;
                    self.counters.requests_queued = self.counters.requests_queued.saturating_add(1);
                }
                Ok(TerrainRequestOutcome::Coalesced { .. }) => {
                    report.coalesced += 1;
                    self.counters.requests_coalesced =
                        self.counters.requests_coalesced.saturating_add(1);
                }
                Ok(TerrainRequestOutcome::Current { .. }) => {
                    report.current += 1;
                    self.counters.pages_current = self.counters.pages_current.saturating_add(1);
                }
                Err(error) if is_backpressure(&error) => {
                    report.deferred += 1;
                    self.counters.requests_deferred =
                        self.counters.requests_deferred.saturating_add(1);
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }

        let resident_after = runtime
            .resident_page_keys(self.planet_id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        report.ready_pages = desired_keys.intersection(&resident_after).count();
        if report.ready_pages == desired_keys.len() {
            let obsolete = self
                .committed
                .keys()
                .copied()
                .filter(|key| !desired_keys.contains(key))
                .collect::<Vec<_>>();
            for key in obsolete {
                if runtime.evict_page(self.planet_id, key)? {
                    report.evicted += 1;
                }
            }
            self.committed = plan
                .demands()
                .iter()
                .map(|demand| (demand.page_key(), demand.request_class()))
                .collect();
            self.pending = None;
            report.committed_pages = self.committed.len();
            report.handoff_committed = true;
            self.counters.handoffs_committed = self.counters.handoffs_committed.saturating_add(1);
        }
        self.counters.pages_evicted = self
            .counters
            .pages_evicted
            .saturating_add(report.evicted as u64);
        Ok(report)
    }
}

fn is_backpressure(error: &TerrainRuntimeError) -> bool {
    matches!(
        error,
        TerrainRuntimeError::RequestBackpressure { .. }
            | TerrainRuntimeError::CompletionBackpressure { .. }
            | TerrainRuntimeError::EventBackpressure { .. }
            | TerrainRuntimeError::PlanetResidentPageBudget { .. }
            | TerrainRuntimeError::GlobalResidentPageBudget { .. }
            | TerrainRuntimeError::ResidentByteBudget { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PageDemand, PlanetDefinition, PlanetPosition, PlanetView, TerrainRuntimeConfig,
        TerrainStreamingConfig, TerrainStreamingPlanner, TerrainSubsystem, CELL_COUNT,
    };
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::thread;
    use std::time::{Duration, Instant};

    const DENSE_PAGE_BYTES: usize = CELL_COUNT * std::mem::size_of::<crate::CellWord>();

    fn start() -> TerrainSubsystem {
        let mut subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 1,
            max_planets: 1,
            max_component_sources: 1,
            request_capacity: 16,
            critical_request_reserve: 2,
            completion_capacity: 16,
            event_capacity: 64,
            max_resident_pages: 8,
            max_resident_dense_bytes: 8 * DENSE_PAGE_BYTES,
            max_completions_per_frame: 16,
        })
        .unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        subsystem
    }

    fn definition() -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId([42; 16]),
            center_cell: [0; 3],
            radius_cells: 100,
            material: 2,
            root_lod: 12,
            max_resident_pages: 8,
        }
    }

    fn plan(keys: &[(PageKey, TerrainRequestClass)]) -> TerrainStreamingPlan {
        TerrainStreamingPlan::for_test(
            definition().planet_id,
            keys.iter()
                .map(|(key, class)| PageDemand::for_test(*key, *class))
                .collect(),
        )
    }

    fn settle(
        session: &mut TerrainResidencySession,
        handle: &TerrainRuntimeHandle,
        plan: &TerrainStreamingPlan,
    ) -> TerrainResidencyReport {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            handle.pump(16);
            handle.drain_events(64);
            let report = session.reconcile(handle, plan, 1).unwrap();
            if report.handoff_committed {
                return report;
            }
            assert!(Instant::now() < deadline, "residency handoff timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn replacement_set_commits_only_after_every_page_is_ready() {
        let mut subsystem = start();
        let handle = subsystem.runtime_handle();
        handle.upsert_planet(definition()).unwrap();
        let mut session = TerrainResidencySession::new(
            definition().planet_id,
            TerrainResidencyConfig {
                max_active_pages: 2,
                max_transition_pages: 4,
                max_requests_per_reconcile: 2,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let first = plan(&[
            (PageKey::new(0, [0, 0, 0]), TerrainRequestClass::Visible),
            (PageKey::new(0, [1, 0, 0]), TerrainRequestClass::Visible),
        ]);
        settle(&mut session, &handle, &first);

        let second = plan(&[
            (PageKey::new(0, [4, 0, 0]), TerrainRequestClass::Visible),
            (PageKey::new(0, [5, 0, 0]), TerrainRequestClass::Visible),
        ]);
        let initial = session.reconcile(&handle, &second, 2).unwrap();
        assert!(!initial.handoff_committed);
        assert_eq!(
            session.committed_pages().collect::<Vec<_>>(),
            first
                .demands()
                .iter()
                .map(|d| d.page_key())
                .collect::<Vec<_>>()
        );
        assert!(
            handle
                .resident_page_keys(definition().planet_id)
                .unwrap()
                .len()
                <= 4
        );

        let committed = settle(&mut session, &handle, &second);
        assert!(committed.handoff_committed);
        assert_eq!(committed.evicted, 2);
        assert_eq!(
            handle
                .resident_page_keys(definition().planet_id)
                .unwrap()
                .len(),
            2
        );
        let stable = session.reconcile(&handle, &second, 3).unwrap();
        assert!(!stable.handoff_committed);
        assert_eq!(stable.submitted, 0);
        assert_eq!(session.counters().handoffs_committed, 2);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn superseded_teleport_cancels_uncommitted_work_and_stays_bounded() {
        let mut subsystem = start();
        let handle = subsystem.runtime_handle();
        handle.upsert_planet(definition()).unwrap();
        let mut session = TerrainResidencySession::new(
            definition().planet_id,
            TerrainResidencyConfig {
                max_active_pages: 2,
                max_transition_pages: 4,
                max_requests_per_reconcile: 1,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let origin = plan(&[(PageKey::new(0, [0, 0, 0]), TerrainRequestClass::Visible)]);
        settle(&mut session, &handle, &origin);
        let abandoned = plan(&[
            (PageKey::new(0, [10, 0, 0]), TerrainRequestClass::Visible),
            (PageKey::new(0, [11, 0, 0]), TerrainRequestClass::Prefetch),
        ]);
        session.reconcile(&handle, &abandoned, 2).unwrap();
        let destination = plan(&[(PageKey::new(0, [-10, 0, 0]), TerrainRequestClass::Visible)]);
        session.reconcile(&handle, &destination, 3).unwrap();
        settle(&mut session, &handle, &destination);

        assert_eq!(
            session.committed_pages().collect::<Vec<_>>(),
            vec![PageKey::new(0, [-10, 0, 0])]
        );
        assert!(session.counters().plans_superseded >= 1);
        assert!(session.counters().transition_page_high_water <= 4);
        assert!(handle.counters().cancelled + handle.counters().stale_rejected >= 1);
        assert!(
            handle
                .resident_page_keys(definition().planet_id)
                .unwrap()
                .len()
                <= 2
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn transition_budget_rejects_a_handoff_without_mutating_committed_state() {
        let mut subsystem = start();
        let handle = subsystem.runtime_handle();
        handle.upsert_planet(definition()).unwrap();
        let mut session = TerrainResidencySession::new(
            definition().planet_id,
            TerrainResidencyConfig {
                max_active_pages: 2,
                max_transition_pages: 2,
                max_requests_per_reconcile: 2,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let first = plan(&[
            (PageKey::new(0, [0, 0, 0]), TerrainRequestClass::Visible),
            (PageKey::new(0, [1, 0, 0]), TerrainRequestClass::Visible),
        ]);
        settle(&mut session, &handle, &first);
        let replacement = plan(&[
            (PageKey::new(0, [2, 0, 0]), TerrainRequestClass::Visible),
            (PageKey::new(0, [3, 0, 0]), TerrainRequestClass::Visible),
        ]);
        assert!(matches!(
            session.reconcile(&handle, &replacement, 2),
            Err(TerrainResidencyError::TransitionPageBudget {
                pages: 4,
                capacity: 2
            })
        ));
        assert_eq!(session.committed_pages().count(), 2);
        assert_eq!(
            handle
                .resident_page_keys(definition().planet_id)
                .unwrap()
                .len(),
            2
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn repeated_teleports_do_not_accumulate_resident_pages() {
        let mut subsystem = start();
        let handle = subsystem.runtime_handle();
        handle.upsert_planet(definition()).unwrap();
        let mut session = TerrainResidencySession::new(
            definition().planet_id,
            TerrainResidencyConfig {
                max_active_pages: 1,
                max_transition_pages: 2,
                max_requests_per_reconcile: 1,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();

        for x in -16..16 {
            let destination =
                plan(&[(PageKey::new(0, [x * 8, 0, 0]), TerrainRequestClass::Visible)]);
            settle(&mut session, &handle, &destination);
            assert_eq!(
                handle
                    .resident_page_keys(definition().planet_id)
                    .unwrap()
                    .len(),
                1
            );
        }

        let counters = session.counters();
        assert_eq!(counters.handoffs_committed, 32);
        assert_eq!(counters.active_page_high_water, 1);
        assert_eq!(counters.transition_page_high_water, 2);
        assert_eq!(handle.counters().resident_page_high_water, 2);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn mixed_lod_plan_materializes_and_commits_end_to_end() {
        let max_pages = 64;
        let mut subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 4,
            max_planets: 1,
            max_component_sources: 1,
            request_capacity: max_pages,
            critical_request_reserve: 4,
            completion_capacity: max_pages,
            event_capacity: max_pages * 2,
            max_resident_pages: max_pages,
            max_resident_dense_bytes: max_pages * DENSE_PAGE_BYTES,
            max_completions_per_frame: max_pages,
        })
        .unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let handle = subsystem.runtime_handle();
        let mut planet = definition();
        planet.radius_cells = 1_000;
        planet.root_lod = 6;
        planet.max_resident_pages = max_pages;
        handle.upsert_planet(planet.clone()).unwrap();
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 2.0,
            prediction_seconds: 0.0,
            max_pages,
            max_traversal_nodes: 4_096,
        })
        .unwrap();
        let view = PlanetView::new(
            PlanetPosition::from_lod0_cell([1_000, 0, 0]),
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [1280, 720],
            0.1,
            20_000_000.0,
            [0.0; 3],
        )
        .unwrap();
        let plan = planner.plan_fixed_sphere(&planet, view).unwrap();
        assert!(plan
            .demands()
            .iter()
            .any(|demand| demand.page_key().lod == 0));
        assert!(plan
            .demands()
            .iter()
            .any(|demand| demand.page_key().lod > 0));
        assert!(plan.is_face_balanced());
        assert!(plan.demands().len() <= max_pages);

        let mut session = TerrainResidencySession::new(
            planet.planet_id,
            TerrainResidencyConfig {
                max_active_pages: max_pages,
                max_transition_pages: max_pages * 2,
                max_requests_per_reconcile: max_pages,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let report = settle(&mut session, &handle, &plan);
        assert!(report.handoff_committed);
        assert_eq!(report.committed_pages, plan.demands().len());
        assert_eq!(
            handle.resident_page_keys(planet.planet_id).unwrap().len(),
            plan.demands().len()
        );
        subsystem.shutdown().unwrap();
    }
}
