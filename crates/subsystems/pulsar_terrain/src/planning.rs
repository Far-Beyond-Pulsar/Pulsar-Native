use crate::{
    FixedSphereGenerator, PlanetDefinition, PlanetId, PlanetView, TerrainCore, TerrainRuntimeError,
    TerrainRuntimeHandle, TerrainSnapshot, TerrainStreamingConfig, TerrainStreamingError,
    TerrainStreamingPlan, TerrainStreamingPlanner,
};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Frame-side coalescing policy wrapped around the deterministic streaming
/// planner. The expensive authoritative traversal runs on its dedicated worker;
/// these thresholds only decide when a newer camera sample materially changes
/// the requested target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainPlanningConfig {
    pub streaming: TerrainStreamingConfig,
    pub position_hysteresis_m: f64,
    pub direction_hysteresis_radians: f64,
    pub velocity_hysteresis_mps: f64,
}

impl Default for TerrainPlanningConfig {
    fn default() -> Self {
        Self {
            streaming: TerrainStreamingConfig::default(),
            position_hysteresis_m: 0.25,
            direction_hysteresis_radians: 0.25_f64.to_radians(),
            velocity_hysteresis_mps: 0.5,
        }
    }
}

impl TerrainPlanningConfig {
    fn validate(self) -> Result<Self, TerrainPlanningError> {
        TerrainStreamingPlanner::new(self.streaming)
            .map_err(|error| TerrainPlanningError::InvalidConfig(error.to_string()))?;
        if !self.position_hysteresis_m.is_finite()
            || self.position_hysteresis_m < 0.0
            || !self.direction_hysteresis_radians.is_finite()
            || self.direction_hysteresis_radians < 0.0
            || !self.velocity_hysteresis_mps.is_finite()
            || self.velocity_hysteresis_mps < 0.0
        {
            return Err(TerrainPlanningError::InvalidConfig(
                "planning hysteresis must be finite and non-negative".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainPlanningTicket {
    pub planet_id: PlanetId,
    pub planet_generation: u64,
    pub submission: u64,
}

#[derive(Debug)]
pub struct TerrainPlanningResult {
    ticket: TerrainPlanningTicket,
    terrain_sequence: u64,
    capture_elapsed: Duration,
    planning_elapsed: Duration,
    plan: Result<TerrainStreamingPlan, TerrainStreamingError>,
}

impl TerrainPlanningResult {
    pub const fn ticket(&self) -> TerrainPlanningTicket {
        self.ticket
    }

    pub const fn terrain_sequence(&self) -> u64 {
        self.terrain_sequence
    }

    pub const fn elapsed(&self) -> Duration {
        self.capture_elapsed.saturating_add(self.planning_elapsed)
    }

    pub const fn capture_elapsed(&self) -> Duration {
        self.capture_elapsed
    }

    pub const fn planning_elapsed(&self) -> Duration {
        self.planning_elapsed
    }

    pub const fn plan(&self) -> Result<&TerrainStreamingPlan, &TerrainStreamingError> {
        self.plan.as_ref()
    }

    pub fn into_plan(self) -> Result<TerrainStreamingPlan, TerrainStreamingError> {
        self.plan
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainPlanningCounters {
    pub pending: usize,
    pub in_flight: usize,
    pub completed: usize,
    pub pending_high_water: usize,
    pub completed_high_water: usize,
    pub submitted: u64,
    pub coalesced: u64,
    pub superseded_pending: u64,
    pub stale_results: u64,
    pub cancelled: u64,
    pub published: u64,
    pub errors: u64,
    pub capture_nanoseconds: u64,
    pub longest_capture_nanoseconds: u64,
    pub planning_nanoseconds: u64,
    pub longest_plan_nanoseconds: u64,
}

#[derive(Debug, Error)]
pub enum TerrainPlanningError {
    #[error("invalid terrain planning configuration: {0}")]
    InvalidConfig(String),
    #[error("failed to spawn the terrain planning worker")]
    ThreadSpawn,
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
}

#[derive(Clone)]
struct PlanningJob {
    ticket: TerrainPlanningTicket,
    terrain_sequence: u64,
    view: PlanetView,
    config: TerrainPlanningConfig,
}

impl PlanningJob {
    fn coalesces(
        &self,
        planet_generation: u64,
        terrain_sequence: u64,
        view: PlanetView,
        config: TerrainPlanningConfig,
    ) -> bool {
        self.ticket.planet_generation == planet_generation
            && self.terrain_sequence == terrain_sequence
            && self.config == config
            && self.view.within_hysteresis(
                view,
                config.position_hysteresis_m,
                config.direction_hysteresis_radians,
                config.velocity_hysteresis_mps,
            )
    }
}

struct PlanningState {
    stopped: bool,
    next_submission: u64,
    pending: HashMap<PlanetId, PlanningJob>,
    latest: HashMap<PlanetId, PlanningJob>,
    completed: HashMap<PlanetId, TerrainPlanningResult>,
    in_flight: Option<TerrainPlanningTicket>,
    counters: TerrainPlanningCounters,
}

impl PlanningState {
    fn new(max_planets: usize) -> Self {
        Self {
            stopped: true,
            next_submission: 1,
            pending: HashMap::with_capacity(max_planets),
            latest: HashMap::with_capacity(max_planets),
            completed: HashMap::with_capacity(max_planets),
            in_flight: None,
            counters: TerrainPlanningCounters::default(),
        }
    }

    fn refresh_counts(&mut self) {
        self.counters.pending = self.pending.len();
        self.counters.in_flight = usize::from(self.in_flight.is_some());
        self.counters.completed = self.completed.len();
        self.counters.pending_high_water =
            self.counters.pending_high_water.max(self.counters.pending);
        self.counters.completed_high_water = self
            .counters
            .completed_high_water
            .max(self.counters.completed);
    }
}

struct PlanningShared {
    state: Mutex<PlanningState>,
    wake: Condvar,
}

impl PlanningShared {
    fn new(max_planets: usize) -> Self {
        Self {
            state: Mutex::new(PlanningState::new(max_planets)),
            wake: Condvar::new(),
        }
    }

    fn pop(&self) -> Option<PlanningJob> {
        let mut state = lock(&self.state);
        loop {
            if state.stopped {
                return None;
            }
            if let Some(planet_id) = state
                .pending
                .iter()
                .min_by_key(|(_, job)| job.ticket.submission)
                .map(|(planet_id, _)| *planet_id)
            {
                let job = state
                    .pending
                    .remove(&planet_id)
                    .expect("selected planning job remains pending");
                state.in_flight = Some(job.ticket);
                state.refresh_counts();
                return Some(job);
            }
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn finish_missing(&self, job: &PlanningJob) {
        let mut state = lock(&self.state);
        state.in_flight = None;
        state.counters.cancelled = state.counters.cancelled.saturating_add(1);
        if state
            .latest
            .get(&job.ticket.planet_id)
            .is_some_and(|latest| latest.ticket == job.ticket)
        {
            state.latest.remove(&job.ticket.planet_id);
        }
        state.refresh_counts();
    }

    fn finish_stale(
        &self,
        job: &PlanningJob,
        capture_elapsed: Duration,
        planning_elapsed: Duration,
    ) {
        let mut state = lock(&self.state);
        state.in_flight = None;
        record_elapsed(&mut state.counters, capture_elapsed, planning_elapsed);
        state.counters.stale_results = state.counters.stale_results.saturating_add(1);
        let latest = (!state.stopped && !state.pending.contains_key(&job.ticket.planet_id))
            .then(|| state.latest.get(&job.ticket.planet_id).cloned())
            .flatten();
        if let Some(latest) = latest {
            state.pending.insert(job.ticket.planet_id, latest);
            self.wake.notify_one();
        }
        state.refresh_counts();
    }

    fn finish_result(
        &self,
        job: PlanningJob,
        terrain_sequence: u64,
        capture_elapsed: Duration,
        planning_elapsed: Duration,
        plan: Result<TerrainStreamingPlan, TerrainStreamingError>,
    ) {
        let mut state = lock(&self.state);
        state.in_flight = None;
        record_elapsed(&mut state.counters, capture_elapsed, planning_elapsed);
        if plan.is_err() {
            state.counters.errors = state.counters.errors.saturating_add(1);
        }
        if !state.stopped {
            if let Some(latest) = state.latest.get_mut(&job.ticket.planet_id) {
                if latest.ticket == job.ticket {
                    latest.terrain_sequence = terrain_sequence;
                }
            }
            let should_publish = state
                .completed
                .get(&job.ticket.planet_id)
                .is_none_or(|current| current.ticket.submission < job.ticket.submission);
            if should_publish {
                state.completed.insert(
                    job.ticket.planet_id,
                    TerrainPlanningResult {
                        ticket: job.ticket,
                        terrain_sequence,
                        capture_elapsed,
                        planning_elapsed,
                        plan,
                    },
                );
            }
        }
        state.refresh_counts();
    }

    fn requeue_latest(&self, planet_id: PlanetId) {
        let mut state = lock(&self.state);
        state.counters.stale_results = state.counters.stale_results.saturating_add(1);
        let latest = (!state.stopped
            && !state.pending.contains_key(&planet_id)
            && state
                .in_flight
                .is_none_or(|ticket| ticket.planet_id != planet_id))
        .then(|| state.latest.get(&planet_id).cloned())
        .flatten();
        if let Some(latest) = latest {
            state.pending.insert(planet_id, latest);
            self.wake.notify_one();
        }
        state.refresh_counts();
    }
}

#[derive(Clone)]
pub struct TerrainPlanningHandle {
    shared: Arc<PlanningShared>,
    runtime: TerrainRuntimeHandle,
}

impl TerrainPlanningHandle {
    pub fn submit(
        &self,
        planet_id: PlanetId,
        view: PlanetView,
        config: TerrainPlanningConfig,
    ) -> Result<TerrainPlanningTicket, TerrainPlanningError> {
        let config = config.validate()?;
        let identity = self.runtime.planning_identity(planet_id)?;
        let mut state = lock(&self.shared.state);
        if state.stopped {
            return Err(TerrainRuntimeError::NotRunning.into());
        }
        let coalesced_ticket = state
            .latest
            .get(&planet_id)
            .filter(|latest| {
                latest.coalesces(
                    identity.planet_generation,
                    identity.terrain_sequence,
                    view,
                    config,
                )
            })
            .map(|latest| latest.ticket);
        if let Some(ticket) = coalesced_ticket {
            state.counters.coalesced = state.counters.coalesced.saturating_add(1);
            return Ok(ticket);
        }
        let submission = state.next_submission;
        state.next_submission = submission
            .checked_add(1)
            .ok_or(TerrainRuntimeError::GenerationOverflow)?;
        let ticket = TerrainPlanningTicket {
            planet_id,
            planet_generation: identity.planet_generation,
            submission,
        };
        let job = PlanningJob {
            ticket,
            terrain_sequence: identity.terrain_sequence,
            view,
            config,
        };
        state.latest.insert(planet_id, job.clone());
        if state.pending.insert(planet_id, job).is_some() {
            state.counters.superseded_pending = state.counters.superseded_pending.saturating_add(1);
        }
        state.counters.submitted = state.counters.submitted.saturating_add(1);
        state.refresh_counts();
        self.shared.wake.notify_one();
        Ok(ticket)
    }

    /// Drain at most `maximum` completed plans. Results invalidated by a
    /// canonical edit or planet replacement are never exposed; the newest view
    /// is automatically requeued against the new canonical sequence.
    pub fn drain_completed(&self, maximum: usize) -> Vec<TerrainPlanningResult> {
        if maximum == 0 {
            return Vec::new();
        }
        let candidates = {
            let mut state = lock(&self.shared.state);
            let mut keys = state
                .completed
                .iter()
                .map(|(planet_id, result)| (*planet_id, result.ticket.submission))
                .collect::<Vec<_>>();
            keys.sort_unstable_by_key(|(_, submission)| *submission);
            let results = keys
                .into_iter()
                .take(maximum)
                .filter_map(|(planet_id, _)| state.completed.remove(&planet_id))
                .collect::<Vec<_>>();
            state.refresh_counts();
            results
        };
        let mut current = Vec::with_capacity(candidates.len());
        for result in candidates {
            if self.runtime.planning_is_current(
                result.ticket.planet_id,
                result.ticket.planet_generation,
                result.terrain_sequence,
            ) {
                current.push(result);
            } else {
                self.shared.requeue_latest(result.ticket.planet_id);
            }
        }
        if !current.is_empty() {
            let mut state = lock(&self.shared.state);
            state.counters.published = state
                .counters
                .published
                .saturating_add(current.len() as u64);
        }
        current
    }

    pub fn counters(&self) -> TerrainPlanningCounters {
        lock(&self.shared.state).counters
    }
}

pub(crate) struct TerrainPlanningService {
    handle: TerrainPlanningHandle,
    worker: Option<JoinHandle<()>>,
}

impl TerrainPlanningService {
    pub(crate) fn new(runtime: TerrainRuntimeHandle, max_planets: usize) -> Self {
        Self {
            handle: TerrainPlanningHandle {
                shared: Arc::new(PlanningShared::new(max_planets)),
                runtime,
            },
            worker: None,
        }
    }

    pub(crate) fn handle(&self) -> TerrainPlanningHandle {
        self.handle.clone()
    }

    pub(crate) fn initialize(&mut self) -> Result<(), TerrainPlanningError> {
        {
            let mut state = lock(&self.handle.shared.state);
            state.stopped = false;
        }
        let shared = self.handle.shared.clone();
        let runtime = self.handle.runtime.clone();
        let worker = thread::Builder::new()
            .name("Pulsar-Terrain-Planning".to_string())
            .spawn(move || planning_worker_loop(shared, runtime));
        self.worker = Some(match worker {
            Ok(worker) => worker,
            Err(_) => {
                lock(&self.handle.shared.state).stopped = true;
                return Err(TerrainPlanningError::ThreadSpawn);
            }
        });
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) {
        {
            let mut state = lock(&self.handle.shared.state);
            state.stopped = true;
            state.pending.clear();
            state.completed.clear();
            state.refresh_counts();
            self.handle.shared.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn planning_worker_loop(shared: Arc<PlanningShared>, runtime: TerrainRuntimeHandle) {
    while let Some(job) = shared.pop() {
        let capture_started = Instant::now();
        let Some(capture) =
            runtime.planning_capture(job.ticket.planet_id, job.ticket.planet_generation)
        else {
            shared.finish_missing(&job);
            continue;
        };
        let capture_elapsed = capture_started.elapsed();
        let generator = FixedSphereGenerator {
            center_cell: capture.definition.center_cell,
            radius_cells: capture.definition.radius_cells,
            material: capture.definition.material,
        };
        let started = Instant::now();
        let plan = TerrainCore::from_snapshot(capture.snapshot, generator)
            .map_err(|error| TerrainStreamingError::TerrainSummary(error.to_string()))
            .and_then(|core| {
                TerrainStreamingPlanner::new(job.config.streaming)?.plan_with_classifier(
                    &capture.definition,
                    job.view,
                    &core,
                )
            });
        let planning_elapsed = started.elapsed();
        if !runtime.planning_is_current(
            job.ticket.planet_id,
            job.ticket.planet_generation,
            capture.terrain_sequence,
        ) {
            shared.finish_stale(&job, capture_elapsed, planning_elapsed);
            continue;
        }
        shared.finish_result(
            job,
            capture.terrain_sequence,
            capture_elapsed,
            planning_elapsed,
            plan,
        );
    }
}

fn record_elapsed(
    counters: &mut TerrainPlanningCounters,
    capture_elapsed: Duration,
    planning_elapsed: Duration,
) {
    let capture_ns = capture_elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
    let planning_ns = planning_elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
    counters.capture_nanoseconds = counters.capture_nanoseconds.saturating_add(capture_ns);
    counters.longest_capture_nanoseconds = counters.longest_capture_nanoseconds.max(capture_ns);
    counters.planning_nanoseconds = counters.planning_nanoseconds.saturating_add(planning_ns);
    counters.longest_plan_nanoseconds = counters.longest_plan_nanoseconds.max(planning_ns);
}

pub(crate) struct TerrainPlanningIdentity {
    pub(crate) planet_generation: u64,
    pub(crate) terrain_sequence: u64,
}

pub(crate) struct TerrainPlanningCapture {
    pub(crate) definition: PlanetDefinition,
    pub(crate) terrain_sequence: u64,
    pub(crate) snapshot: TerrainSnapshot,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EditMode, EditOp, EditShape, PlanetPosition, TerrainRuntimeConfig, TerrainSubsystem,
    };
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::thread;

    fn definition(id: u8) -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId([id; 16]),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: id.max(1),
            root_lod: 8,
            max_resident_pages: 512,
        }
    }

    fn start() -> TerrainSubsystem {
        let mut subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 1,
            max_planets: 4,
            max_component_sources: 4,
            request_capacity: 32,
            critical_request_reserve: 4,
            completion_capacity: 32,
            event_capacity: 64,
            max_resident_pages: 512,
            max_resident_dense_bytes: 512 * crate::CELL_COUNT * 4,
            max_completions_per_frame: 16,
        })
        .unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        subsystem
    }

    fn view(camera: [i64; 3], forward: [f64; 3], velocity_mps: [f64; 3]) -> PlanetView {
        PlanetView::new(
            PlanetPosition::from_lod0_cell(camera),
            forward,
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [1920, 1080],
            0.1,
            100_000.0,
            velocity_mps,
        )
        .unwrap()
    }

    fn wait_for_submission(
        planning: &TerrainPlanningHandle,
        submission: u64,
    ) -> TerrainPlanningResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = planning
                .drain_completed(4)
                .into_iter()
                .find(|result| result.ticket().submission == submission)
            {
                return result;
            }
            assert!(Instant::now() < deadline, "timed out waiting for plan");
            thread::yield_now();
        }
    }

    #[test]
    fn identical_and_sub_hysteresis_views_coalesce_without_new_work() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planning = subsystem.planning_handle();
        let planet = definition(1);
        runtime.upsert_planet(planet.clone()).unwrap();
        let config = TerrainPlanningConfig {
            streaming: TerrainStreamingConfig {
                max_pages: 128,
                max_traversal_nodes: 4_096,
                ..TerrainStreamingConfig::default()
            },
            ..TerrainPlanningConfig::default()
        };
        let base = view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]);
        let first = planning.submit(planet.planet_id, base, config).unwrap();
        assert_eq!(
            planning.submit(planet.planet_id, base, config).unwrap(),
            first
        );
        let nearby = view([1_001, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]);
        assert_eq!(
            planning.submit(planet.planet_id, nearby, config).unwrap(),
            first
        );
        let result = wait_for_submission(&planning, first.submission);
        assert!(result.plan().is_ok());
        let counters = planning.counters();
        assert_eq!(counters.submitted, 1);
        assert_eq!(counters.coalesced, 2);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn rapid_ground_flight_orbit_pole_and_teleport_submissions_converge_to_latest() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planning = subsystem.planning_handle();
        let planet = definition(2);
        runtime.upsert_planet(planet.clone()).unwrap();
        let config = TerrainPlanningConfig {
            streaming: TerrainStreamingConfig {
                interaction_radius_m: 8.0,
                max_pages: 256,
                max_traversal_nodes: 8_192,
                ..TerrainStreamingConfig::default()
            },
            position_hysteresis_m: 0.0,
            direction_hysteresis_radians: 0.0,
            velocity_hysteresis_mps: 0.0,
        };
        let views = [
            view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]),
            view([1_100, 0, 0], [-1.0, 0.0, 0.0], [250.0, 0.0, 0.0]),
            view([20_000, 0, 0], [-1.0, 0.0, 0.0], [2_000.0, 0.0, 0.0]),
            view([0, 1_000, 0], [0.0, -0.999, 0.0447], [0.0; 3]),
            view([-1_000, 0, 0], [1.0, 0.0, 0.0], [0.0; 3]),
        ];
        let mut latest = None;
        for submitted_view in views {
            latest = Some(
                planning
                    .submit(planet.planet_id, submitted_view, config)
                    .unwrap(),
            );
        }
        let latest = latest.unwrap();
        let result = wait_for_submission(&planning, latest.submission);
        let expected = TerrainStreamingPlanner::new(config.streaming)
            .unwrap()
            .plan_fixed_sphere(&planet, views[4])
            .unwrap();
        assert_eq!(result.into_plan().unwrap(), expected);
        let counters = planning.counters();
        assert_eq!(counters.submitted, views.len() as u64);
        assert!(counters.superseded_pending > 0);
        assert!(counters.pending <= 1);
        assert!(counters.completed <= 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn canonical_edit_invalidates_a_completed_plan_and_requeues_the_latest_view() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planning = subsystem.planning_handle();
        let planet = definition(3);
        runtime.upsert_planet(planet.clone()).unwrap();
        let config = TerrainPlanningConfig {
            streaming: TerrainStreamingConfig {
                max_pages: 128,
                max_traversal_nodes: 4_096,
                ..TerrainStreamingConfig::default()
            },
            ..TerrainPlanningConfig::default()
        };
        let ticket = planning
            .submit(
                planet.planet_id,
                view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]),
                config,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while planning.counters().completed == 0 {
            assert!(Instant::now() < deadline, "plan did not complete");
            thread::yield_now();
        }
        runtime
            .append_edit(
                planet.planet_id,
                EditOp {
                    sequence: 1,
                    stable_id: [0xA5; 16],
                    shape: EditShape::Sphere {
                        center_cell: [1_000, 0, 0],
                        radius_cells: 4,
                    },
                    mode: EditMode::Subtract,
                    material: 0,
                },
            )
            .unwrap();
        assert!(planning.drain_completed(1).is_empty());
        let result = wait_for_submission(&planning, ticket.submission);
        assert_eq!(result.terrain_sequence(), 1);
        assert!(result.plan().is_ok());
        assert_eq!(
            planning
                .submit(
                    planet.planet_id,
                    view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]),
                    config,
                )
                .unwrap(),
            ticket
        );
        assert!(planning.counters().stale_results >= 1);
        subsystem.shutdown().unwrap();
    }
}
