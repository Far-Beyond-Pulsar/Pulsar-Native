use crate::{
    CompactedPageRecord, EditOp, FixedSphereGenerator, PageBuildCommitOutcome,
    PageBuildPreparation, PageBuildRequest, PageBuildResult, PageKey, PlanetDefinition, PlanetId,
    TerrainCore, TerrainCoreError, CELL_COUNT,
};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use engine_subsystems::{Subsystem, SubsystemContext, SubsystemError, SubsystemId};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use thiserror::Error;

pub const TERRAIN_SUBSYSTEM_ID: SubsystemId = SubsystemId::new("planetary_terrain");
const DENSE_PAGE_BYTES: usize = CELL_COUNT * std::mem::size_of::<crate::CellWord>();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerrainRequestClass {
    Prefetch,
    Visible,
    Collision,
    EditResponse,
}

impl TerrainRequestClass {
    const fn priority(self) -> u8 {
        match self {
            Self::Prefetch => 0,
            Self::Visible => 1,
            Self::Collision => 2,
            Self::EditResponse => 3,
        }
    }

    const fn is_critical(self) -> bool {
        matches!(self, Self::Collision | Self::EditResponse)
    }
}

#[derive(Clone, Debug)]
pub struct TerrainRuntimeConfig {
    pub worker_count: usize,
    pub max_planets: usize,
    pub max_component_sources: usize,
    pub request_capacity: usize,
    pub critical_request_reserve: usize,
    pub completion_capacity: usize,
    pub event_capacity: usize,
    pub max_resident_pages: usize,
    pub max_resident_dense_bytes: usize,
    pub max_completions_per_frame: usize,
}

impl Default for TerrainRuntimeConfig {
    fn default() -> Self {
        Self {
            worker_count: thread::available_parallelism()
                .map_or(2, usize::from)
                .saturating_sub(2)
                .clamp(1, 8),
            max_planets: 16,
            max_component_sources: 64,
            request_capacity: 1_024,
            critical_request_reserve: 128,
            completion_capacity: 1_024,
            event_capacity: 2_048,
            max_resident_pages: 8_192,
            max_resident_dense_bytes: 1 << 30,
            max_completions_per_frame: 64,
        }
    }
}

impl TerrainRuntimeConfig {
    fn validate(&self) -> Result<(), TerrainRuntimeError> {
        if self.worker_count == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "worker_count must be non-zero",
            ));
        }
        if self.max_planets == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "max_planets must be non-zero",
            ));
        }
        if self.max_component_sources == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "max_component_sources must be non-zero",
            ));
        }
        if self.request_capacity == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "request_capacity must be non-zero",
            ));
        }
        if self.critical_request_reserve >= self.request_capacity {
            return Err(TerrainRuntimeError::InvalidConfig(
                "critical_request_reserve must be smaller than request_capacity",
            ));
        }
        if self.completion_capacity == 0 || self.event_capacity == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "completion_capacity and event_capacity must be non-zero",
            ));
        }
        if self.critical_request_reserve >= self.completion_capacity {
            return Err(TerrainRuntimeError::InvalidConfig(
                "critical_request_reserve must be smaller than completion_capacity",
            ));
        }
        if self.max_resident_pages == 0 || self.max_resident_dense_bytes < DENSE_PAGE_BYTES {
            return Err(TerrainRuntimeError::InvalidConfig(
                "resident budgets must hold at least one dense page",
            ));
        }
        if self.max_completions_per_frame == 0 {
            return Err(TerrainRuntimeError::InvalidConfig(
                "max_completions_per_frame must be non-zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainBackpressure {
    RequestQueue,
    CompletionSlots,
    EventQueue,
    PlanetCapacity,
    ComponentCapacity,
    ResidentPages,
    ResidentBytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainRuntimeEvent {
    PageReady {
        planet_id: PlanetId,
        page_key: PageKey,
        planet_generation: u64,
        page_generation: u64,
        request_class: TerrainRequestClass,
        record: CompactedPageRecord,
    },
    EvictPlanet {
        planet_id: PlanetId,
        retired_generation: u64,
    },
    EvictPage {
        planet_id: PlanetId,
        page_key: PageKey,
        planet_generation: u64,
        retired_page_generation: u64,
    },
    StaleRejected {
        planet_id: PlanetId,
        page_key: PageKey,
        planet_generation: u64,
        page_generation: u64,
    },
    Backpressure {
        planet_id: Option<PlanetId>,
        page_key: Option<PageKey>,
        kind: TerrainBackpressure,
    },
    Error {
        planet_id: PlanetId,
        page_key: PageKey,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainRuntimeCounters {
    pub planets: usize,
    pub queued: usize,
    pub in_flight: usize,
    pub completed: usize,
    pub events: usize,
    pub outstanding: usize,
    pub resident_pages: usize,
    pub resident_dense_bytes: usize,
    pub reserved_new_pages: usize,
    pub reserved_result_bytes: usize,
    pub queue_high_water: usize,
    pub in_flight_high_water: usize,
    pub completed_high_water: usize,
    pub event_high_water: usize,
    pub resident_page_high_water: usize,
    pub resident_dense_byte_high_water: usize,
    pub accepted: u64,
    pub coalesced: u64,
    pub priority_upgrades: u64,
    pub cancelled: u64,
    pub evicted: u64,
    pub stale_rejected: u64,
    pub backpressured: u64,
    pub published: u64,
    pub errors: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainRequestOutcome {
    Queued {
        planet_generation: u64,
        page_generation: u64,
    },
    Coalesced {
        planet_generation: u64,
        page_generation: u64,
        upgraded: bool,
    },
    Current {
        planet_generation: u64,
        page_generation: u64,
        record: CompactedPageRecord,
    },
}

#[derive(Debug, Error)]
pub enum TerrainRuntimeError {
    #[error("invalid terrain runtime configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("terrain runtime is not running")]
    NotRunning,
    #[error("terrain runtime has already been initialized")]
    AlreadyInitialized,
    #[error("planet {0:?} is not registered")]
    PlanetMissing(PlanetId),
    #[error("planet capacity {capacity} is exhausted")]
    PlanetCapacity { capacity: usize },
    #[error("terrain component-source capacity {capacity} is exhausted")]
    ComponentCapacity { capacity: usize },
    #[error("request queue is saturated for {class:?}")]
    RequestBackpressure { class: TerrainRequestClass },
    #[error("completion capacity {capacity} is exhausted")]
    CompletionBackpressure { capacity: usize },
    #[error("runtime event capacity {capacity} is exhausted")]
    EventBackpressure { capacity: usize },
    #[error("planet {planet_id:?} resident-page budget {capacity} is exhausted")]
    PlanetResidentPageBudget {
        planet_id: PlanetId,
        capacity: usize,
    },
    #[error("global resident-page budget {capacity} is exhausted")]
    GlobalResidentPageBudget { capacity: usize },
    #[error("global resident dense-byte budget {capacity} is exhausted")]
    ResidentByteBudget { capacity: usize },
    #[error("terrain generation counter overflowed")]
    GenerationOverflow,
    #[error(transparent)]
    Core(#[from] TerrainCoreError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RequestIdentity {
    planet_id: PlanetId,
    page_key: PageKey,
    planet_generation: u64,
    page_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestPhase {
    Queued,
    Running,
    Completed,
}

#[derive(Clone, Copy, Debug)]
struct ActiveRequest {
    phase: RequestPhase,
    class: TerrainRequestClass,
    deadline_tick: u64,
    reserved_new_page: bool,
}

struct WorkJob {
    identity: RequestIdentity,
    request: PageBuildRequest<FixedSphereGenerator>,
    class: TerrainRequestClass,
    deadline_tick: u64,
    order: u64,
}

struct WorkQueueState {
    jobs: Vec<WorkJob>,
    stopped: bool,
}

struct WorkQueue {
    capacity: usize,
    noncritical_capacity: usize,
    state: Mutex<WorkQueueState>,
    wake: Condvar,
}

impl WorkQueue {
    fn new(config: &TerrainRuntimeConfig) -> Self {
        Self {
            capacity: config.request_capacity,
            noncritical_capacity: config
                .request_capacity
                .saturating_sub(config.critical_request_reserve),
            state: Mutex::new(WorkQueueState {
                jobs: Vec::with_capacity(config.request_capacity),
                stopped: false,
            }),
            wake: Condvar::new(),
        }
    }

    fn try_push(&self, job: WorkJob) -> bool {
        let mut state = lock(&self.state);
        let limit = if job.class.is_critical() {
            self.capacity
        } else {
            self.noncritical_capacity
        };
        if state.stopped || state.jobs.len() >= limit {
            return false;
        }
        state.jobs.push(job);
        self.wake.notify_one();
        true
    }

    fn upgrade(&self, identity: RequestIdentity, class: TerrainRequestClass, deadline_tick: u64) {
        let mut state = lock(&self.state);
        if let Some(job) = state.jobs.iter_mut().find(|job| job.identity == identity) {
            if class.priority() > job.class.priority() {
                job.class = class;
            }
            job.deadline_tick = job.deadline_tick.min(deadline_tick);
        }
    }

    fn pop(&self) -> Option<WorkJob> {
        let mut state = lock(&self.state);
        loop {
            if state.stopped {
                return None;
            }
            if !state.jobs.is_empty() {
                let index = state
                    .jobs
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, job)| {
                        (Reverse(job.class.priority()), job.deadline_tick, job.order)
                    })
                    .map(|(index, _)| index)
                    .expect("non-empty queue has a selected job");
                return Some(state.jobs.swap_remove(index));
            }
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn cancel_where(&self, predicate: impl Fn(&WorkJob) -> bool) -> Vec<WorkJob> {
        let mut state = lock(&self.state);
        let mut retained = Vec::with_capacity(state.jobs.len());
        let mut cancelled = Vec::new();
        for job in state.jobs.drain(..) {
            if predicate(&job) {
                cancelled.push(job);
            } else {
                retained.push(job);
            }
        }
        state.jobs = retained;
        cancelled
    }

    fn stop(&self) -> Vec<WorkJob> {
        let mut state = lock(&self.state);
        state.stopped = true;
        let cancelled = state.jobs.drain(..).collect();
        self.wake.notify_all();
        cancelled
    }
}

struct PlanetRuntime {
    definition: PlanetDefinition,
    generation: u64,
    core: TerrainCore<FixedSphereGenerator>,
    page_generations: BTreeMap<PageKey, u64>,
}

impl PlanetRuntime {
    fn new(definition: PlanetDefinition, generation: u64) -> Result<Self, TerrainCoreError> {
        let generator = FixedSphereGenerator {
            center_cell: definition.center_cell,
            radius_cells: definition.radius_cells,
            material: definition.material,
        };
        let core = TerrainCore::new(definition.planet_id, definition.root_lod, generator)?;
        Ok(Self {
            definition,
            generation,
            core,
            page_generations: BTreeMap::new(),
        })
    }
}

struct RuntimeState {
    running: bool,
    ever_initialized: bool,
    next_planet_generation: u64,
    next_job_order: u64,
    planets: HashMap<PlanetId, PlanetRuntime>,
    component_sources: HashMap<String, PlanetId>,
    active: HashMap<RequestIdentity, ActiveRequest>,
    events: VecDeque<TerrainRuntimeEvent>,
    counters: TerrainRuntimeCounters,
}

impl RuntimeState {
    fn new(config: &TerrainRuntimeConfig) -> Self {
        Self {
            running: false,
            ever_initialized: false,
            next_planet_generation: 1,
            next_job_order: 1,
            planets: HashMap::with_capacity(config.max_planets),
            component_sources: HashMap::with_capacity(config.max_component_sources),
            active: HashMap::with_capacity(config.completion_capacity),
            events: VecDeque::with_capacity(config.event_capacity),
            counters: TerrainRuntimeCounters::default(),
        }
    }

    fn allocate_planet_generation(&mut self) -> Result<u64, TerrainRuntimeError> {
        let generation = self.next_planet_generation;
        self.next_planet_generation = generation
            .checked_add(1)
            .ok_or(TerrainRuntimeError::GenerationOverflow)?;
        Ok(generation)
    }

    fn cancel_active(&mut self, identity: RequestIdentity) {
        if let Some(active) = self.active.remove(&identity) {
            match active.phase {
                RequestPhase::Queued => {
                    self.counters.queued = self.counters.queued.saturating_sub(1)
                }
                RequestPhase::Running => {
                    self.counters.in_flight = self.counters.in_flight.saturating_sub(1)
                }
                RequestPhase::Completed => {
                    self.counters.completed = self.counters.completed.saturating_sub(1)
                }
            }
            if active.reserved_new_page {
                self.counters.reserved_new_pages =
                    self.counters.reserved_new_pages.saturating_sub(1);
            }
            self.counters.reserved_result_bytes = self
                .counters
                .reserved_result_bytes
                .saturating_sub(DENSE_PAGE_BYTES);
            self.counters.cancelled = self.counters.cancelled.saturating_add(1);
        }
    }

    fn push_event(&mut self, event: TerrainRuntimeEvent, capacity: usize) -> bool {
        if self.events.len() >= capacity {
            return false;
        }
        self.events.push_back(event);
        self.counters.events = self.events.len();
        self.counters.event_high_water = self.counters.event_high_water.max(self.events.len());
        true
    }

    fn refresh_resident_counters(&mut self) {
        let memory = self
            .planets
            .values()
            .map(|planet| planet.core.memory_counters())
            .fold((0_usize, 0_usize), |totals, memory| {
                (
                    totals.0.saturating_add(memory.resident_pages),
                    totals.1.saturating_add(memory.resident_dense_bytes),
                )
            });
        self.counters.planets = self.planets.len();
        self.counters.resident_pages = memory.0;
        self.counters.resident_dense_bytes = memory.1;
        self.counters.resident_page_high_water =
            self.counters.resident_page_high_water.max(memory.0);
        self.counters.resident_dense_byte_high_water =
            self.counters.resident_dense_byte_high_water.max(memory.1);
        self.counters.outstanding = self.active.len();
    }
}

enum WorkerResult {
    Built(Result<PageBuildResult, String>),
    Cancelled,
}

struct WorkCompletion {
    identity: RequestIdentity,
    result: WorkerResult,
}

struct RuntimeShared {
    config: TerrainRuntimeConfig,
    state: Mutex<RuntimeState>,
    queue: WorkQueue,
    completion_tx: Sender<WorkCompletion>,
    completion_rx: Receiver<WorkCompletion>,
}

impl RuntimeShared {
    fn new(config: TerrainRuntimeConfig) -> Self {
        let (completion_tx, completion_rx) = crossbeam_channel::bounded(config.completion_capacity);
        Self {
            queue: WorkQueue::new(&config),
            state: Mutex::new(RuntimeState::new(&config)),
            completion_tx,
            completion_rx,
            config,
        }
    }

    fn worker_started(&self, identity: RequestIdentity) -> bool {
        let mut state = lock(&self.state);
        let valid = state.running
            && state
                .planets
                .get(&identity.planet_id)
                .is_some_and(|planet| {
                    planet.generation == identity.planet_generation
                        && planet.page_generations.get(&identity.page_key).copied()
                            == Some(identity.page_generation)
                });
        let transitioned = state.active.get_mut(&identity).is_some_and(|active| {
            if active.phase != RequestPhase::Queued {
                return false;
            }
            active.phase = RequestPhase::Running;
            true
        });
        if transitioned {
            state.counters.queued = state.counters.queued.saturating_sub(1);
            state.counters.in_flight = state.counters.in_flight.saturating_add(1);
            state.counters.in_flight_high_water = state
                .counters
                .in_flight_high_water
                .max(state.counters.in_flight);
        }
        valid
    }

    fn publish_worker_completion(&self, completion: WorkCompletion) {
        let mut state = lock(&self.state);
        let identity = completion.identity;
        let transitioned = state.active.get_mut(&identity).is_some_and(|active| {
            if active.phase != RequestPhase::Running {
                return false;
            }
            active.phase = RequestPhase::Completed;
            true
        });
        if transitioned {
            state.counters.in_flight = state.counters.in_flight.saturating_sub(1);
            state.counters.completed = state.counters.completed.saturating_add(1);
            state.counters.completed_high_water = state
                .counters
                .completed_high_water
                .max(state.counters.completed);
            assert!(
                self.completion_tx.try_send(completion).is_ok(),
                "every active request reserves one bounded completion slot"
            );
        }
    }
}

#[derive(Clone)]
pub struct TerrainRuntimeHandle {
    shared: Arc<RuntimeShared>,
}

impl TerrainRuntimeHandle {
    pub fn is_running(&self) -> bool {
        lock(&self.shared.state).running
    }

    pub fn upsert_planet(&self, definition: PlanetDefinition) -> Result<u64, TerrainRuntimeError> {
        validate_definition(&definition, &self.shared.config)?;
        let mut state = lock(&self.shared.state);
        if !state.running {
            return Err(TerrainRuntimeError::NotRunning);
        }
        if let Some(existing) = state.planets.get(&definition.planet_id) {
            if existing.definition == definition {
                return Ok(existing.generation);
            }
            if state.events.len() >= self.shared.config.event_capacity {
                state.counters.backpressured = state.counters.backpressured.saturating_add(1);
                return Err(TerrainRuntimeError::EventBackpressure {
                    capacity: self.shared.config.event_capacity,
                });
            }
        } else if state.planets.len() >= self.shared.config.max_planets {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(definition.planet_id),
                None,
                TerrainBackpressure::PlanetCapacity,
            );
            return Err(TerrainRuntimeError::PlanetCapacity {
                capacity: self.shared.config.max_planets,
            });
        }

        let retired = state
            .planets
            .get(&definition.planet_id)
            .map(|planet| planet.generation);
        let generation = state.allocate_planet_generation()?;
        let runtime = PlanetRuntime::new(definition.clone(), generation)?;
        let cancelled = self
            .shared
            .queue
            .cancel_where(|job| job.identity.planet_id == definition.planet_id);
        for job in cancelled {
            state.cancel_active(job.identity);
        }
        state.planets.insert(definition.planet_id, runtime);
        if let Some(retired_generation) = retired {
            let pushed = state.push_event(
                TerrainRuntimeEvent::EvictPlanet {
                    planet_id: definition.planet_id,
                    retired_generation,
                },
                self.shared.config.event_capacity,
            );
            debug_assert!(pushed, "event capacity was checked before replacement");
        }
        state.refresh_resident_counters();
        Ok(generation)
    }

    pub fn remove_planet(&self, planet_id: PlanetId) -> Result<bool, TerrainRuntimeError> {
        let mut state = lock(&self.shared.state);
        if !state.running {
            return Err(TerrainRuntimeError::NotRunning);
        }
        let Some(retired_generation) = state
            .planets
            .get(&planet_id)
            .map(|planet| planet.generation)
        else {
            return Ok(false);
        };
        if state.events.len() >= self.shared.config.event_capacity {
            state.counters.backpressured = state.counters.backpressured.saturating_add(1);
            return Err(TerrainRuntimeError::EventBackpressure {
                capacity: self.shared.config.event_capacity,
            });
        }
        let cancelled = self
            .shared
            .queue
            .cancel_where(|job| job.identity.planet_id == planet_id);
        for job in cancelled {
            state.cancel_active(job.identity);
        }
        state.planets.remove(&planet_id);
        state.component_sources.retain(|_, id| *id != planet_id);
        let pushed = state.push_event(
            TerrainRuntimeEvent::EvictPlanet {
                planet_id,
                retired_generation,
            },
            self.shared.config.event_capacity,
        );
        debug_assert!(pushed, "event capacity was checked before removal");
        state.refresh_resident_counters();
        Ok(true)
    }

    pub fn upsert_component(
        &self,
        source_key: String,
        definition: PlanetDefinition,
    ) -> Result<u64, TerrainRuntimeError> {
        let previous = {
            let mut state = lock(&self.shared.state);
            if !state.running {
                return Err(TerrainRuntimeError::NotRunning);
            }
            if !state.component_sources.contains_key(&source_key)
                && state.component_sources.len() >= self.shared.config.max_component_sources
            {
                record_backpressure(
                    &mut state,
                    &self.shared.config,
                    Some(definition.planet_id),
                    None,
                    TerrainBackpressure::ComponentCapacity,
                );
                return Err(TerrainRuntimeError::ComponentCapacity {
                    capacity: self.shared.config.max_component_sources,
                });
            }
            state.component_sources.get(&source_key).copied()
        };

        let generation = self.upsert_planet(definition.clone())?;
        if let Some(previous) = previous.filter(|planet_id| *planet_id != definition.planet_id) {
            let has_other_owner = lock(&self.shared.state)
                .component_sources
                .iter()
                .any(|(source, planet_id)| source != &source_key && *planet_id == previous);
            if !has_other_owner {
                self.remove_planet(previous)?;
            }
        }
        lock(&self.shared.state)
            .component_sources
            .insert(source_key, definition.planet_id);
        Ok(generation)
    }

    pub fn remove_component(&self, source_key: &str) -> Result<(), TerrainRuntimeError> {
        let (planet_id, has_other_owner) = {
            let mut state = lock(&self.shared.state);
            let planet_id = state.component_sources.remove(source_key);
            let has_other_owner = planet_id.is_some_and(|removed| {
                state
                    .component_sources
                    .values()
                    .any(|planet_id| *planet_id == removed)
            });
            (planet_id, has_other_owner)
        };
        if let Some(planet_id) = planet_id.filter(|_| !has_other_owner) {
            self.remove_planet(planet_id)?;
        }
        Ok(())
    }

    pub fn append_edit(
        &self,
        planet_id: PlanetId,
        operation: EditOp,
    ) -> Result<(), TerrainRuntimeError> {
        let mut state = lock(&self.shared.state);
        if !state.running {
            return Err(TerrainRuntimeError::NotRunning);
        }
        let planet = state
            .planets
            .get_mut(&planet_id)
            .ok_or(TerrainRuntimeError::PlanetMissing(planet_id))?;
        planet.core.append_edit(operation)?;
        let bounds = operation.shape.bounds();
        let mut affected = Vec::new();
        for (key, generation) in &mut planet.page_generations {
            if page_intersects(*key, bounds) {
                *generation = generation
                    .checked_add(1)
                    .ok_or(TerrainRuntimeError::GenerationOverflow)?;
                affected.push(*key);
            }
        }
        if !affected.is_empty() {
            let cancelled = self.shared.queue.cancel_where(|job| {
                job.identity.planet_id == planet_id && affected.contains(&job.identity.page_key)
            });
            for job in cancelled {
                state.cancel_active(job.identity);
            }
        }
        state.refresh_resident_counters();
        Ok(())
    }

    pub fn request_page(
        &self,
        planet_id: PlanetId,
        page_key: PageKey,
        class: TerrainRequestClass,
        deadline_tick: u64,
    ) -> Result<TerrainRequestOutcome, TerrainRuntimeError> {
        let mut state = lock(&self.shared.state);
        if !state.running {
            return Err(TerrainRuntimeError::NotRunning);
        }
        let (planet_generation, page_generation) = {
            let planet = state
                .planets
                .get(&planet_id)
                .ok_or(TerrainRuntimeError::PlanetMissing(planet_id))?;
            (
                planet.generation,
                planet.page_generations.get(&page_key).copied().unwrap_or(1),
            )
        };
        let identity = RequestIdentity {
            planet_id,
            page_key,
            planet_generation,
            page_generation,
        };
        if state.active.contains_key(&identity) {
            let upgraded = {
                let active = state
                    .active
                    .get_mut(&identity)
                    .expect("active request was checked above");
                let upgraded = class.priority() > active.class.priority()
                    || deadline_tick < active.deadline_tick;
                if class.priority() > active.class.priority() {
                    active.class = class;
                }
                active.deadline_tick = active.deadline_tick.min(deadline_tick);
                upgraded
            };
            state.counters.coalesced = state.counters.coalesced.saturating_add(1);
            if upgraded {
                state.counters.priority_upgrades =
                    state.counters.priority_upgrades.saturating_add(1);
            }
            self.shared.queue.upgrade(identity, class, deadline_tick);
            return Ok(TerrainRequestOutcome::Coalesced {
                planet_generation,
                page_generation,
                upgraded,
            });
        }

        let (preparation, is_new_page, planet_resident, planet_resident_limit) = {
            let planet = state
                .planets
                .get(&planet_id)
                .expect("planet remains registered while state is locked");
            (
                planet.core.prepare_page_build(page_key)?,
                planet.core.page(page_key).is_none(),
                planet.core.memory_counters().resident_pages,
                planet.definition.max_resident_pages,
            )
        };

        let request = match preparation {
            PageBuildPreparation::Current(record) => {
                return Ok(TerrainRequestOutcome::Current {
                    planet_generation,
                    page_generation,
                    record,
                });
            }
            PageBuildPreparation::Build(request) => request,
        };

        if !class.is_critical() && state.events.len() >= self.shared.config.event_capacity {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::EventQueue,
            );
            return Err(TerrainRuntimeError::EventBackpressure {
                capacity: self.shared.config.event_capacity,
            });
        }
        if state.active.len() >= self.shared.config.completion_capacity {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::CompletionSlots,
            );
            return Err(TerrainRuntimeError::CompletionBackpressure {
                capacity: self.shared.config.completion_capacity,
            });
        }
        let noncritical_active = state
            .active
            .values()
            .filter(|active| !active.class.is_critical())
            .count();
        let noncritical_limit = self
            .shared
            .config
            .completion_capacity
            .saturating_sub(self.shared.config.critical_request_reserve);
        if !class.is_critical() && noncritical_active >= noncritical_limit {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::RequestQueue,
            );
            return Err(TerrainRuntimeError::RequestBackpressure { class });
        }

        let planet_reserved = state
            .active
            .iter()
            .filter(|(identity, active)| {
                identity.planet_id == planet_id && active.reserved_new_page
            })
            .count();
        if is_new_page && planet_resident.saturating_add(planet_reserved) >= planet_resident_limit {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::ResidentPages,
            );
            return Err(TerrainRuntimeError::PlanetResidentPageBudget {
                planet_id,
                capacity: planet_resident_limit,
            });
        }
        if is_new_page
            && state
                .counters
                .resident_pages
                .saturating_add(state.counters.reserved_new_pages)
                >= self.shared.config.max_resident_pages
        {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::ResidentPages,
            );
            return Err(TerrainRuntimeError::GlobalResidentPageBudget {
                capacity: self.shared.config.max_resident_pages,
            });
        }
        if state
            .counters
            .resident_dense_bytes
            .saturating_add(state.counters.reserved_result_bytes)
            .saturating_add(DENSE_PAGE_BYTES)
            > self.shared.config.max_resident_dense_bytes
        {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::ResidentBytes,
            );
            return Err(TerrainRuntimeError::ResidentByteBudget {
                capacity: self.shared.config.max_resident_dense_bytes,
            });
        }

        let order = state.next_job_order;
        state.next_job_order = order
            .checked_add(1)
            .ok_or(TerrainRuntimeError::GenerationOverflow)?;
        let job = WorkJob {
            identity,
            request,
            class,
            deadline_tick,
            order,
        };
        if !self.shared.queue.try_push(job) {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::RequestQueue,
            );
            return Err(TerrainRuntimeError::RequestBackpressure { class });
        }
        state.active.insert(
            identity,
            ActiveRequest {
                phase: RequestPhase::Queued,
                class,
                deadline_tick,
                reserved_new_page: is_new_page,
            },
        );
        state
            .planets
            .get_mut(&planet_id)
            .expect("planet remains registered while state is locked")
            .page_generations
            .entry(page_key)
            .or_insert(page_generation);
        state.counters.queued = state.counters.queued.saturating_add(1);
        state.counters.queue_high_water =
            state.counters.queue_high_water.max(state.counters.queued);
        state.counters.accepted = state.counters.accepted.saturating_add(1);
        if is_new_page {
            state.counters.reserved_new_pages = state.counters.reserved_new_pages.saturating_add(1);
        }
        state.counters.reserved_result_bytes = state
            .counters
            .reserved_result_bytes
            .saturating_add(DENSE_PAGE_BYTES);
        state.refresh_resident_counters();
        Ok(TerrainRequestOutcome::Queued {
            planet_generation,
            page_generation,
        })
    }

    /// Retire one disposable resident page and all work targeting its current
    /// generation. The compacted content hash remains authoritative, so a
    /// later request can deterministically rehydrate the same page.
    pub fn evict_page(
        &self,
        planet_id: PlanetId,
        page_key: PageKey,
    ) -> Result<bool, TerrainRuntimeError> {
        let mut state = lock(&self.shared.state);
        if !state.running {
            return Err(TerrainRuntimeError::NotRunning);
        }
        let Some((planet_generation, page_generation, is_resident)) =
            state.planets.get(&planet_id).map(|planet| {
                (
                    planet.generation,
                    planet.page_generations.get(&page_key).copied().unwrap_or(1),
                    planet.core.page(page_key).is_some(),
                )
            })
        else {
            return Err(TerrainRuntimeError::PlanetMissing(planet_id));
        };
        let has_active = state
            .active
            .keys()
            .any(|identity| identity.planet_id == planet_id && identity.page_key == page_key);
        if !is_resident && !has_active {
            return Ok(false);
        }
        if state.events.len() >= self.shared.config.event_capacity {
            record_backpressure(
                &mut state,
                &self.shared.config,
                Some(planet_id),
                Some(page_key),
                TerrainBackpressure::EventQueue,
            );
            return Err(TerrainRuntimeError::EventBackpressure {
                capacity: self.shared.config.event_capacity,
            });
        }

        let cancelled = self.shared.queue.cancel_where(|job| {
            job.identity.planet_id == planet_id && job.identity.page_key == page_key
        });
        for job in cancelled {
            state.cancel_active(job.identity);
        }
        let remaining = state
            .active
            .iter()
            .filter_map(|(identity, active)| {
                (identity.planet_id == planet_id
                    && identity.page_key == page_key
                    && active.phase != RequestPhase::Completed)
                    .then_some(*identity)
            })
            .collect::<Vec<_>>();
        for identity in remaining {
            state.cancel_active(identity);
        }

        let next_page_generation = page_generation
            .checked_add(1)
            .ok_or(TerrainRuntimeError::GenerationOverflow)?;
        let planet = state
            .planets
            .get_mut(&planet_id)
            .expect("planet remains registered while state is locked");
        planet
            .page_generations
            .insert(page_key, next_page_generation);
        planet.core.evict_resident_page(page_key);
        state.counters.evicted = state.counters.evicted.saturating_add(1);
        let pushed = state.push_event(
            TerrainRuntimeEvent::EvictPage {
                planet_id,
                page_key,
                planet_generation,
                retired_page_generation: page_generation,
            },
            self.shared.config.event_capacity,
        );
        debug_assert!(pushed, "event capacity was checked before page eviction");
        state.refresh_resident_counters();
        Ok(true)
    }

    pub fn pump(&self, max_completions: usize) -> usize {
        let mut processed = 0;
        while processed < max_completions {
            let mut state = lock(&self.shared.state);
            if state.events.len() >= self.shared.config.event_capacity {
                break;
            }
            let completion = match self.shared.completion_rx.try_recv() {
                Ok(completion) => completion,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            self.publish_completion(&mut state, completion);
            processed += 1;
        }
        processed
    }

    pub fn drain_events(&self, limit: usize) -> Vec<TerrainRuntimeEvent> {
        let mut state = lock(&self.shared.state);
        let count = limit.min(state.events.len());
        let events = state.events.drain(..count).collect();
        state.counters.events = state.events.len();
        events
    }

    pub fn counters(&self) -> TerrainRuntimeCounters {
        let mut state = lock(&self.shared.state);
        state.refresh_resident_counters();
        state.counters
    }

    pub fn page_snapshot(
        &self,
        planet_id: PlanetId,
        page_key: PageKey,
    ) -> Option<crate::VoxelPage> {
        lock(&self.shared.state)
            .planets
            .get(&planet_id)
            .and_then(|planet| planet.core.page(page_key))
            .cloned()
    }

    pub fn resident_page_keys(
        &self,
        planet_id: PlanetId,
    ) -> Result<Vec<PageKey>, TerrainRuntimeError> {
        let state = lock(&self.shared.state);
        let planet = state
            .planets
            .get(&planet_id)
            .ok_or(TerrainRuntimeError::PlanetMissing(planet_id))?;
        Ok(planet.core.resident_page_keys().collect())
    }

    fn publish_completion(&self, state: &mut RuntimeState, completion: WorkCompletion) {
        let Some(active) = state.active.remove(&completion.identity) else {
            return;
        };
        if active.phase == RequestPhase::Completed {
            state.counters.completed = state.counters.completed.saturating_sub(1);
        }
        if active.reserved_new_page {
            state.counters.reserved_new_pages = state.counters.reserved_new_pages.saturating_sub(1);
        }
        state.counters.reserved_result_bytes = state
            .counters
            .reserved_result_bytes
            .saturating_sub(DENSE_PAGE_BYTES);

        let identity = completion.identity;
        let current = state
            .planets
            .get(&identity.planet_id)
            .is_some_and(|planet| {
                planet.generation == identity.planet_generation
                    && planet.page_generations.get(&identity.page_key).copied()
                        == Some(identity.page_generation)
            });
        let event = if !current || matches!(&completion.result, WorkerResult::Cancelled) {
            state.counters.stale_rejected = state.counters.stale_rejected.saturating_add(1);
            TerrainRuntimeEvent::StaleRejected {
                planet_id: identity.planet_id,
                page_key: identity.page_key,
                planet_generation: identity.planet_generation,
                page_generation: identity.page_generation,
            }
        } else {
            match completion.result {
                WorkerResult::Built(Ok(result)) => {
                    let outcome = state
                        .planets
                        .get_mut(&identity.planet_id)
                        .expect("current completion has a planet")
                        .core
                        .commit_page_build(result);
                    match outcome {
                        Ok(PageBuildCommitOutcome::Committed(record))
                        | Ok(PageBuildCommitOutcome::Duplicate(record)) => {
                            state.counters.published = state.counters.published.saturating_add(1);
                            TerrainRuntimeEvent::PageReady {
                                planet_id: identity.planet_id,
                                page_key: identity.page_key,
                                planet_generation: identity.planet_generation,
                                page_generation: identity.page_generation,
                                request_class: active.class,
                                record,
                            }
                        }
                        Ok(PageBuildCommitOutcome::Stale { .. }) => {
                            state.counters.stale_rejected =
                                state.counters.stale_rejected.saturating_add(1);
                            TerrainRuntimeEvent::StaleRejected {
                                planet_id: identity.planet_id,
                                page_key: identity.page_key,
                                planet_generation: identity.planet_generation,
                                page_generation: identity.page_generation,
                            }
                        }
                        Err(error) => {
                            state.counters.errors = state.counters.errors.saturating_add(1);
                            TerrainRuntimeEvent::Error {
                                planet_id: identity.planet_id,
                                page_key: identity.page_key,
                                message: error.to_string(),
                            }
                        }
                    }
                }
                WorkerResult::Built(Err(message)) => {
                    state.counters.errors = state.counters.errors.saturating_add(1);
                    TerrainRuntimeEvent::Error {
                        planet_id: identity.planet_id,
                        page_key: identity.page_key,
                        message,
                    }
                }
                WorkerResult::Cancelled => unreachable!("cancelled result handled above"),
            }
        };
        let pushed = state.push_event(event, self.shared.config.event_capacity);
        debug_assert!(
            pushed,
            "pump checks event capacity before receiving completion"
        );
        state.refresh_resident_counters();
    }
}

pub struct TerrainSubsystem {
    handle: TerrainRuntimeHandle,
    workers: Vec<JoinHandle<()>>,
}

impl TerrainSubsystem {
    pub fn new(config: TerrainRuntimeConfig) -> Result<Self, TerrainRuntimeError> {
        config.validate()?;
        let shared = Arc::new(RuntimeShared::new(config));
        Ok(Self {
            handle: TerrainRuntimeHandle { shared },
            workers: Vec::new(),
        })
    }

    pub fn runtime_handle(&self) -> TerrainRuntimeHandle {
        self.handle.clone()
    }

    fn initialize(&mut self) -> Result<(), TerrainRuntimeError> {
        {
            let mut state = lock(&self.handle.shared.state);
            if state.ever_initialized {
                return Err(TerrainRuntimeError::AlreadyInitialized);
            }
            state.ever_initialized = true;
            state.running = true;
        }
        for worker_index in 0..self.handle.shared.config.worker_count {
            let shared = self.handle.shared.clone();
            let handle = thread::Builder::new()
                .name(format!("Pulsar-Terrain-{worker_index}"))
                .spawn(move || worker_loop(shared))
                .map_err(|_| {
                    TerrainRuntimeError::InvalidConfig("failed to spawn terrain worker")
                })?;
            self.workers.push(handle);
        }
        Ok(())
    }

    fn shutdown_internal(&mut self) {
        {
            let mut state = lock(&self.handle.shared.state);
            if !state.running {
                return;
            }
            state.running = false;
        }
        let queued = self.handle.shared.queue.stop();
        {
            let mut state = lock(&self.handle.shared.state);
            for job in queued {
                state.cancel_active(job.identity);
            }
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        while let Ok(completion) = self.handle.shared.completion_rx.try_recv() {
            let mut state = lock(&self.handle.shared.state);
            state.cancel_active(completion.identity);
        }
        let mut state = lock(&self.handle.shared.state);
        let remaining: Vec<_> = state.active.keys().copied().collect();
        for identity in remaining {
            state.cancel_active(identity);
        }
        state.refresh_resident_counters();
    }
}

impl Subsystem for TerrainSubsystem {
    fn id(&self) -> SubsystemId {
        TERRAIN_SUBSYSTEM_ID
    }

    fn dependencies(&self) -> Vec<SubsystemId> {
        Vec::new()
    }

    fn init(&mut self, _context: &SubsystemContext) -> Result<(), SubsystemError> {
        self.initialize()
            .map_err(|error| SubsystemError::InitFailed(error.to_string()))
    }

    fn shutdown(&mut self) -> Result<(), SubsystemError> {
        self.shutdown_internal();
        Ok(())
    }

    fn on_frame(&mut self, _delta_time: f32) {
        self.handle
            .pump(self.handle.shared.config.max_completions_per_frame);
    }
}

impl Drop for TerrainSubsystem {
    fn drop(&mut self) {
        self.shutdown_internal();
    }
}

fn worker_loop(shared: Arc<RuntimeShared>) {
    while let Some(job) = shared.queue.pop() {
        let valid = shared.worker_started(job.identity);
        let result = if valid {
            WorkerResult::Built(job.request.execute().map_err(|error| error.to_string()))
        } else {
            WorkerResult::Cancelled
        };
        shared.publish_worker_completion(WorkCompletion {
            identity: job.identity,
            result,
        });
    }
}

fn validate_definition(
    definition: &PlanetDefinition,
    config: &TerrainRuntimeConfig,
) -> Result<(), TerrainRuntimeError> {
    if definition.radius_cells == 0
        || definition.material == 0
        || !(1..=62).contains(&definition.root_lod)
        || !definition.fits_centered_root()
        || definition.max_resident_pages == 0
        || definition.max_resident_pages > config.max_resident_pages
    {
        return Err(TerrainRuntimeError::InvalidConfig(
            "planet definition exceeds the runtime contract",
        ));
    }
    Ok(())
}

fn page_intersects(key: PageKey, bounds: ([i64; 3], [i64; 3])) -> bool {
    let Some(min) = key.lod0_cell_min() else {
        return false;
    };
    let Some(span) = key.lod0_cell_span() else {
        return false;
    };
    (0..3).all(|axis| min[axis] < bounds.1[axis] && min[axis].saturating_add(span) > bounds.0[axis])
}

fn record_backpressure(
    state: &mut RuntimeState,
    config: &TerrainRuntimeConfig,
    planet_id: Option<PlanetId>,
    page_key: Option<PageKey>,
    kind: TerrainBackpressure,
) {
    state.counters.backpressured = state.counters.backpressured.saturating_add(1);
    let _ = state.push_event(
        TerrainRuntimeEvent::Backpressure {
            planet_id,
            page_key,
            kind,
        },
        config.event_capacity,
    );
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditMode, EditShape};
    use std::time::{Duration, Instant};

    fn config(worker_count: usize) -> TerrainRuntimeConfig {
        TerrainRuntimeConfig {
            worker_count,
            max_planets: 4,
            max_component_sources: 8,
            request_capacity: 8,
            critical_request_reserve: 2,
            completion_capacity: 8,
            event_capacity: 16,
            max_resident_pages: 8,
            max_resident_dense_bytes: 8 * DENSE_PAGE_BYTES,
            max_completions_per_frame: 8,
        }
    }

    fn planet(id: u8) -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId([id; 16]),
            center_cell: [0; 3],
            radius_cells: 100,
            material: id.max(1),
            root_lod: 12,
            max_resident_pages: 8,
        }
    }

    fn start(worker_count: usize) -> TerrainSubsystem {
        let mut subsystem = TerrainSubsystem::new(config(worker_count)).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        subsystem
    }

    #[test]
    fn runtime_rejects_planets_outside_their_centered_root() {
        let mut subsystem = start(1);
        let handle = subsystem.runtime_handle();
        let mut invalid = planet(1);
        invalid.root_lod = 1;
        assert!(matches!(
            handle.upsert_planet(invalid),
            Err(TerrainRuntimeError::InvalidConfig(_))
        ));
        subsystem.shutdown().unwrap();
    }

    fn wait_for_events(handle: &TerrainRuntimeHandle, count: usize) -> Vec<TerrainRuntimeEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.len() < count && Instant::now() < deadline {
            handle.pump(64);
            events.extend(handle.drain_events(64));
            if events.len() < count {
                thread::yield_now();
            }
        }
        assert!(
            events.len() >= count,
            "timed out waiting for terrain events"
        );
        events
    }

    #[test]
    fn subsystem_lifecycle_joins_workers_and_accounts_for_every_job() {
        let mut subsystem = start(2);
        assert_eq!(subsystem.id(), TERRAIN_SUBSYSTEM_ID);
        assert!(subsystem.dependencies().is_empty());
        let handle = subsystem.runtime_handle();
        let id = planet(1).planet_id;
        handle.upsert_planet(planet(1)).unwrap();
        for x in 0..4 {
            handle
                .request_page(
                    id,
                    PageKey::new(0, [x, 0, 0]),
                    TerrainRequestClass::Visible,
                    10,
                )
                .unwrap();
        }
        subsystem.shutdown().unwrap();
        let counters = handle.counters();
        assert!(!handle.is_running());
        assert_eq!(counters.outstanding, 0);
        assert_eq!(
            counters.accepted,
            counters.published + counters.stale_rejected + counters.cancelled + counters.errors
        );
    }

    #[test]
    fn saturated_prefetch_preserves_edit_and_collision_admission() {
        let mut cfg = config(1);
        cfg.request_capacity = 4;
        cfg.completion_capacity = 4;
        cfg.critical_request_reserve = 1;
        let mut subsystem = TerrainSubsystem::new(cfg).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let handle = subsystem.runtime_handle();
        let id = planet(2).planet_id;
        handle.upsert_planet(planet(2)).unwrap();
        for x in 0..3 {
            handle
                .request_page(
                    id,
                    PageKey::new(0, [x, 0, 0]),
                    TerrainRequestClass::Prefetch,
                    100,
                )
                .unwrap();
        }
        assert!(matches!(
            handle.request_page(
                id,
                PageKey::new(0, [3, 0, 0]),
                TerrainRequestClass::Prefetch,
                100
            ),
            Err(TerrainRuntimeError::RequestBackpressure { .. })
        ));
        assert!(matches!(
            handle.request_page(
                id,
                PageKey::new(0, [4, 0, 0]),
                TerrainRequestClass::EditResponse,
                1
            ),
            Ok(TerrainRequestOutcome::Queued { .. })
        ));
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn duplicate_requests_coalesce_and_upgrade_without_duplicate_work() {
        let mut subsystem = start(1);
        let handle = subsystem.runtime_handle();
        let id = planet(3).planet_id;
        handle.upsert_planet(planet(3)).unwrap();
        let key = PageKey::new(0, [-1, 2, -3]);
        handle
            .request_page(id, key, TerrainRequestClass::Prefetch, 100)
            .unwrap();
        assert!(matches!(
            handle.request_page(id, key, TerrainRequestClass::Collision, 10),
            Ok(TerrainRequestOutcome::Coalesced { upgraded: true, .. })
        ));
        let events = wait_for_events(&handle, 1);
        assert!(matches!(
            events[0],
            TerrainRuntimeEvent::PageReady {
                request_class: TerrainRequestClass::Collision,
                ..
            }
        ));
        assert_eq!(handle.counters().accepted, 1);
        assert_eq!(handle.counters().coalesced, 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn newer_edits_make_late_page_results_harmless() {
        let mut subsystem = start(1);
        let handle = subsystem.runtime_handle();
        let id = planet(4).planet_id;
        handle.upsert_planet(planet(4)).unwrap();
        let key = PageKey::new(0, [0; 3]);
        handle
            .request_page(id, key, TerrainRequestClass::Visible, 10)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.counters().in_flight + handle.counters().completed == 0
            && Instant::now() < deadline
        {
            thread::yield_now();
        }
        assert!(
            handle.counters().in_flight + handle.counters().completed > 0,
            "worker did not start the stale-result fixture"
        );
        handle
            .append_edit(
                id,
                EditOp {
                    sequence: 1,
                    stable_id: [1; 16],
                    shape: EditShape::Sphere {
                        center_cell: [8; 3],
                        radius_cells: 2,
                    },
                    mode: EditMode::Subtract,
                    material: 0,
                },
            )
            .unwrap();
        let events = wait_for_events(&handle, 1);
        assert!(events.iter().any(|event| matches!(
            event,
            TerrainRuntimeEvent::StaleRejected { page_key, .. } if *page_key == key
        )));
        assert!(handle.page_snapshot(id, key).is_none());
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn eviction_retires_active_generation_and_allows_exact_rehydration() {
        let mut subsystem = start(1);
        let handle = subsystem.runtime_handle();
        let id = planet(12).planet_id;
        handle.upsert_planet(planet(12)).unwrap();
        let key = PageKey::new(0, [0; 3]);
        handle
            .request_page(id, key, TerrainRequestClass::Visible, 1)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.counters().in_flight + handle.counters().completed == 0
            && Instant::now() < deadline
        {
            thread::yield_now();
        }
        assert!(handle.evict_page(id, key).unwrap());
        assert!(matches!(
            handle.drain_events(1).as_slice(),
            [TerrainRuntimeEvent::EvictPage { page_key, .. }] if *page_key == key
        ));
        handle.pump(8);
        assert!(handle.page_snapshot(id, key).is_none());
        assert_eq!(handle.counters().outstanding, 0);

        handle
            .request_page(id, key, TerrainRequestClass::Visible, 2)
            .unwrap();
        let events = wait_for_events(&handle, 1);
        let page_id = events.iter().find_map(|event| match event {
            TerrainRuntimeEvent::PageReady { record, .. } => Some(record.page_id),
            _ => None,
        });
        assert_eq!(
            page_id,
            handle.page_snapshot(id, key).map(|page| page.page_id())
        );
        assert_eq!(handle.counters().evicted, 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn signed_coordinates_and_planets_remain_distinct_across_worker_counts() {
        fn run(worker_count: usize) -> BTreeMap<(PlanetId, PageKey), crate::PageId> {
            let mut subsystem = start(worker_count);
            let handle = subsystem.runtime_handle();
            for id in [5_u8, 6_u8] {
                handle.upsert_planet(planet(id)).unwrap();
                for key in [PageKey::new(0, [-2, -1, 0]), PageKey::new(0, [2, 1, 0])] {
                    handle
                        .request_page(PlanetId([id; 16]), key, TerrainRequestClass::Visible, 10)
                        .unwrap();
                }
            }
            let events = wait_for_events(&handle, 4);
            let records = events
                .into_iter()
                .filter_map(|event| match event {
                    TerrainRuntimeEvent::PageReady {
                        planet_id,
                        page_key,
                        record,
                        ..
                    } => Some(((planet_id, page_key), record.page_id)),
                    _ => None,
                })
                .collect();
            subsystem.shutdown().unwrap();
            records
        }

        let single = run(1);
        let parallel = run(4);
        assert_eq!(single, parallel);
        assert_eq!(single.len(), 4);
    }

    #[test]
    fn a_full_event_queue_defers_completion_without_dropping_work() {
        let mut cfg = config(1);
        cfg.event_capacity = 1;
        cfg.completion_capacity = 2;
        cfg.request_capacity = 2;
        cfg.critical_request_reserve = 1;
        let mut subsystem = TerrainSubsystem::new(cfg).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let handle = subsystem.runtime_handle();
        let mut definition = planet(8);
        let id = definition.planet_id;
        handle.upsert_planet(definition.clone()).unwrap();
        definition.center_cell[0] = 1;
        handle.upsert_planet(definition).unwrap();

        handle
            .request_page(
                id,
                PageKey::new(0, [0; 3]),
                TerrainRequestClass::EditResponse,
                1,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.counters().completed == 0 && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(handle.counters().completed, 1);
        assert_eq!(handle.pump(1), 0);
        assert_eq!(handle.counters().outstanding, 1);
        assert!(matches!(
            handle.drain_events(1).as_slice(),
            [TerrainRuntimeEvent::EvictPlanet { .. }]
        ));

        assert_eq!(handle.pump(1), 1);
        assert!(matches!(
            handle.drain_events(1).as_slice(),
            [TerrainRuntimeEvent::PageReady { .. }]
        ));
        let counters = handle.counters();
        assert_eq!(counters.outstanding, 0);
        assert_eq!(counters.accepted, 1);
        assert_eq!(counters.published, 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn component_sources_are_bounded_and_shared_planets_are_reference_counted() {
        let mut cfg = config(1);
        cfg.max_component_sources = 2;
        let mut subsystem = TerrainSubsystem::new(cfg).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let handle = subsystem.runtime_handle();
        let first = planet(9);
        let first_id = first.planet_id;
        handle
            .upsert_component("first-a".to_owned(), first.clone())
            .unwrap();
        handle
            .upsert_component("first-b".to_owned(), first)
            .unwrap();
        assert_eq!(handle.counters().planets, 1);

        handle.remove_component("first-a").unwrap();
        assert_eq!(handle.counters().planets, 1);
        handle
            .upsert_component("second".to_owned(), planet(10))
            .unwrap();
        assert!(matches!(
            handle.upsert_component("overflow".to_owned(), planet(11)),
            Err(TerrainRuntimeError::ComponentCapacity { capacity: 2 })
        ));

        handle.remove_component("first-b").unwrap();
        assert_eq!(handle.counters().planets, 1);
        assert!(matches!(
            handle.request_page(
                first_id,
                PageKey::new(0, [0; 3]),
                TerrainRequestClass::Visible,
                1,
            ),
            Err(TerrainRuntimeError::PlanetMissing(id)) if id == first_id
        ));
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn configured_resident_budgets_return_typed_backpressure() {
        let mut cfg = config(1);
        cfg.max_resident_pages = 1;
        cfg.max_resident_dense_bytes = DENSE_PAGE_BYTES;
        let mut definition = planet(7);
        definition.max_resident_pages = 1;
        let mut subsystem = TerrainSubsystem::new(cfg).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let handle = subsystem.runtime_handle();
        let id = definition.planet_id;
        handle.upsert_planet(definition).unwrap();
        handle
            .request_page(id, PageKey::new(0, [0; 3]), TerrainRequestClass::Visible, 1)
            .unwrap();
        assert!(matches!(
            handle.request_page(
                id,
                PageKey::new(0, [1, 0, 0]),
                TerrainRequestClass::Visible,
                1
            ),
            Err(TerrainRuntimeError::PlanetResidentPageBudget { .. })
                | Err(TerrainRuntimeError::ResidentByteBudget { .. })
        ));
        let counters = handle.counters();
        assert!(counters.resident_pages + counters.reserved_new_pages <= 1);
        assert!(counters.resident_dense_bytes + counters.reserved_result_bytes <= DENSE_PAGE_BYTES);
        subsystem.shutdown().unwrap();
    }
}
