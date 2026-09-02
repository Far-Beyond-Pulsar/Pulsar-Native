use crate::{
    ContentHash, DeterministicGenerator, FixedSphereGenerator, PlanetDefinition, PlanetId,
    TerrainCore, TerrainPlanningHandle, TerrainRuntimeError, TerrainRuntimeHandle, TerrainSnapshot,
    TerrainStore,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainPersistenceConfig {
    pub request_capacity: usize,
    pub event_capacity: usize,
    pub max_snapshot_bytes: usize,
    pub max_retained_snapshot_bytes: usize,
    pub max_completions_per_frame: usize,
}

impl Default for TerrainPersistenceConfig {
    fn default() -> Self {
        Self {
            request_capacity: 16,
            event_capacity: 64,
            max_snapshot_bytes: 256 * 1024 * 1024,
            max_retained_snapshot_bytes: 512 * 1024 * 1024,
            max_completions_per_frame: 4,
        }
    }
}

impl TerrainPersistenceConfig {
    fn validate(self) -> Result<Self, TerrainPersistenceError> {
        if self.request_capacity == 0 {
            return Err(TerrainPersistenceError::InvalidConfig(
                "request_capacity must be non-zero",
            ));
        }
        if self.event_capacity == 0 {
            return Err(TerrainPersistenceError::InvalidConfig(
                "event_capacity must be non-zero",
            ));
        }
        if self.event_capacity < self.request_capacity {
            return Err(TerrainPersistenceError::InvalidConfig(
                "event_capacity must cover every accepted persistence request",
            ));
        }
        if self.max_snapshot_bytes == 0
            || self.max_retained_snapshot_bytes < self.max_snapshot_bytes
        {
            return Err(TerrainPersistenceError::InvalidConfig(
                "snapshot byte budgets are invalid",
            ));
        }
        if self.max_completions_per_frame == 0 {
            return Err(TerrainPersistenceError::InvalidConfig(
                "max_completions_per_frame must be non-zero",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainPersistenceRequestKind {
    Save,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainPersistenceTicket {
    pub planet_id: PlanetId,
    pub request: u64,
    pub kind: TerrainPersistenceRequestKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainPersistenceRequestOutcome {
    Queued(TerrainPersistenceTicket),
    Coalesced {
        ticket: TerrainPersistenceTicket,
        superseded: TerrainPersistenceTicket,
    },
}

impl TerrainPersistenceRequestOutcome {
    pub const fn ticket(self) -> TerrainPersistenceTicket {
        match self {
            Self::Queued(ticket) | Self::Coalesced { ticket, .. } => ticket,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainPersistenceFailureKind {
    Store,
    Snapshot,
    SnapshotTooLarge,
    PlanetIdentity,
    GeneratorDefinition,
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainPersistenceEvent {
    Saved {
        ticket: TerrainPersistenceTicket,
        store_generation: u64,
        snapshot_hash: ContentHash,
        terrain_sequence: u64,
    },
    Restored {
        ticket: TerrainPersistenceTicket,
        store_generation: u64,
        snapshot_hash: ContentHash,
        retired_planet_generation: u64,
        planet_generation: u64,
    },
    RestoreMissing {
        ticket: TerrainPersistenceTicket,
    },
    StaleRejected {
        ticket: TerrainPersistenceTicket,
    },
    Failed {
        ticket: TerrainPersistenceTicket,
        kind: TerrainPersistenceFailureKind,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainPersistenceCounters {
    pub pending: usize,
    pub in_flight: usize,
    pub completed: usize,
    pub events: usize,
    pub outstanding: usize,
    pub retained_snapshot_bytes: usize,
    pub pending_high_water: usize,
    pub completed_high_water: usize,
    pub event_high_water: usize,
    pub retained_snapshot_byte_high_water: usize,
    pub submitted: u64,
    pub coalesced: u64,
    pub saved: u64,
    pub restored: u64,
    pub missing: u64,
    pub stale_rejected: u64,
    pub backpressured: u64,
    pub errors: u64,
}

#[derive(Debug, Error)]
pub enum TerrainPersistenceError {
    #[error("invalid terrain persistence configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("terrain persistence worker is not running")]
    NotRunning,
    #[error("failed to spawn the terrain persistence worker")]
    ThreadSpawn,
    #[error("terrain persistence request queue capacity {capacity} is exhausted")]
    RequestBackpressure { capacity: usize },
    #[error(
        "terrain persistence retained-byte budget {capacity} cannot reserve {requested} bytes"
    )]
    SnapshotByteBackpressure { requested: usize, capacity: usize },
    #[error("planet {planet_id:?} already has an incompatible persistence operation in flight")]
    PlanetBusy { planet_id: PlanetId },
    #[error("terrain persistence request counter overflowed")]
    RequestOverflow,
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
}

#[derive(Clone)]
pub(crate) struct TerrainPersistenceCapture {
    pub(crate) definition: PlanetDefinition,
    pub(crate) planet_generation: u64,
    pub(crate) terrain_sequence: u64,
    pub(crate) snapshot: TerrainSnapshot,
}

#[derive(Clone)]
pub(crate) struct TerrainPersistenceIdentity {
    pub(crate) definition: PlanetDefinition,
    pub(crate) planet_generation: u64,
    pub(crate) terrain_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerrainPersistenceRestoreCommit {
    pub(crate) retired_planet_generation: u64,
    pub(crate) planet_generation: u64,
}

#[derive(Clone)]
struct PersistenceIdentity {
    definition: PlanetDefinition,
    planet_generation: u64,
    terrain_sequence: u64,
}

enum PersistencePayload {
    Save(Box<TerrainSnapshot>),
    Restore,
}

struct PersistenceJob {
    ticket: TerrainPersistenceTicket,
    identity: PersistenceIdentity,
    store: TerrainStore,
    payload: PersistencePayload,
    reserved_bytes: usize,
}

impl PersistenceJob {
    fn store_matches(&self, store: &TerrainStore) -> bool {
        self.store.root() == store.root()
    }
}

struct RestoreReady {
    record_generation: u64,
    snapshot_hash: ContentHash,
    retained_bytes: usize,
    core: Option<TerrainCore<FixedSphereGenerator>>,
}

enum PersistenceWorkerResult {
    Saved {
        store_generation: u64,
        snapshot_hash: ContentHash,
    },
    RestoreReady(Box<RestoreReady>),
    RestoreMissing,
    Failed {
        kind: TerrainPersistenceFailureKind,
        message: String,
    },
}

struct PersistenceCompletion {
    ticket: TerrainPersistenceTicket,
    identity: PersistenceIdentity,
    result: PersistenceWorkerResult,
    retained_bytes: usize,
}

struct PersistenceState {
    accepting: bool,
    stopping: bool,
    next_request: u64,
    pending: HashMap<PlanetId, PersistenceJob>,
    in_flight: Option<TerrainPersistenceTicket>,
    completed: VecDeque<PersistenceCompletion>,
    events: VecDeque<TerrainPersistenceEvent>,
    retained_snapshot_bytes: usize,
    counters: TerrainPersistenceCounters,
}

impl PersistenceState {
    fn new(config: TerrainPersistenceConfig) -> Self {
        Self {
            accepting: false,
            stopping: false,
            next_request: 1,
            pending: HashMap::with_capacity(config.request_capacity),
            in_flight: None,
            completed: VecDeque::with_capacity(config.request_capacity),
            events: VecDeque::with_capacity(config.event_capacity),
            retained_snapshot_bytes: 0,
            counters: TerrainPersistenceCounters::default(),
        }
    }

    fn allocate_ticket(
        &mut self,
        planet_id: PlanetId,
        kind: TerrainPersistenceRequestKind,
    ) -> Result<TerrainPersistenceTicket, TerrainPersistenceError> {
        let request = self.next_request;
        self.next_request = request
            .checked_add(1)
            .ok_or(TerrainPersistenceError::RequestOverflow)?;
        Ok(TerrainPersistenceTicket {
            planet_id,
            request,
            kind,
        })
    }

    fn outstanding(&self) -> usize {
        self.pending
            .len()
            .saturating_add(usize::from(self.in_flight.is_some()))
            .saturating_add(self.completed.len())
    }

    fn refresh_counts(&mut self) {
        self.counters.pending = self.pending.len();
        self.counters.in_flight = usize::from(self.in_flight.is_some());
        self.counters.completed = self.completed.len();
        self.counters.events = self.events.len();
        self.counters.outstanding = self.outstanding();
        self.counters.retained_snapshot_bytes = self.retained_snapshot_bytes;
        self.counters.pending_high_water =
            self.counters.pending_high_water.max(self.counters.pending);
        self.counters.completed_high_water = self
            .counters
            .completed_high_water
            .max(self.counters.completed);
        self.counters.event_high_water = self.counters.event_high_water.max(self.counters.events);
        self.counters.retained_snapshot_byte_high_water = self
            .counters
            .retained_snapshot_byte_high_water
            .max(self.retained_snapshot_bytes);
    }
}

struct PersistenceShared {
    config: TerrainPersistenceConfig,
    state: Mutex<PersistenceState>,
    wake: Condvar,
}

impl PersistenceShared {
    fn new(config: TerrainPersistenceConfig) -> Self {
        Self {
            config,
            state: Mutex::new(PersistenceState::new(config)),
            wake: Condvar::new(),
        }
    }

    fn pop(&self) -> Option<PersistenceJob> {
        let mut state = lock(&self.state);
        loop {
            if let Some(planet_id) = state
                .pending
                .iter()
                .min_by_key(|(_, job)| job.ticket.request)
                .map(|(planet_id, _)| *planet_id)
            {
                let job = state
                    .pending
                    .remove(&planet_id)
                    .expect("selected persistence job remains pending");
                state.in_flight = Some(job.ticket);
                state.refresh_counts();
                return Some(job);
            }
            if state.stopping {
                return None;
            }
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn finish(&self, job: PersistenceJob, mut result: PersistenceWorkerResult) {
        let mut state = lock(&self.state);
        state.in_flight = None;
        state.retained_snapshot_bytes = state
            .retained_snapshot_bytes
            .saturating_sub(job.reserved_bytes);

        let retained_bytes = match &result {
            PersistenceWorkerResult::RestoreReady(ready) => ready.retained_bytes,
            _ => 0,
        };
        if retained_bytes > 0
            && state
                .retained_snapshot_bytes
                .checked_add(retained_bytes)
                .is_none_or(|total| total > self.config.max_retained_snapshot_bytes)
        {
            result = PersistenceWorkerResult::Failed {
                kind: TerrainPersistenceFailureKind::SnapshotTooLarge,
                message: format!(
                    "restored snapshot requires {retained_bytes} retained bytes, budget is {}",
                    self.config.max_retained_snapshot_bytes
                ),
            };
        }
        let retained_bytes = if matches!(result, PersistenceWorkerResult::RestoreReady(_)) {
            retained_bytes
        } else {
            0
        };
        state.retained_snapshot_bytes =
            state.retained_snapshot_bytes.saturating_add(retained_bytes);
        state.completed.push_back(PersistenceCompletion {
            ticket: job.ticket,
            identity: job.identity,
            result,
            retained_bytes,
        });
        debug_assert!(state.outstanding() <= self.config.request_capacity);
        state.refresh_counts();
        self.wake.notify_all();
    }
}

#[derive(Clone)]
pub struct TerrainPersistenceHandle {
    shared: Arc<PersistenceShared>,
    runtime: TerrainRuntimeHandle,
    planning: TerrainPlanningHandle,
}

impl TerrainPersistenceHandle {
    pub fn config(&self) -> TerrainPersistenceConfig {
        self.shared.config
    }

    pub fn request_save(
        &self,
        planet_id: PlanetId,
        store: TerrainStore,
    ) -> Result<TerrainPersistenceRequestOutcome, TerrainPersistenceError> {
        self.preflight(planet_id, &store, TerrainPersistenceRequestKind::Save)?;
        let capture = self.runtime.persistence_capture(planet_id)?;
        let reserved_bytes = capture.snapshot.encoded_len_upper_bound().ok_or(
            TerrainPersistenceError::SnapshotByteBackpressure {
                requested: usize::MAX,
                capacity: self.shared.config.max_snapshot_bytes,
            },
        )?;
        if reserved_bytes > self.shared.config.max_snapshot_bytes {
            return Err(TerrainPersistenceError::SnapshotByteBackpressure {
                requested: reserved_bytes,
                capacity: self.shared.config.max_snapshot_bytes,
            });
        }
        self.submit(PersistenceJob {
            ticket: TerrainPersistenceTicket {
                planet_id,
                request: 0,
                kind: TerrainPersistenceRequestKind::Save,
            },
            identity: PersistenceIdentity {
                definition: capture.definition,
                planet_generation: capture.planet_generation,
                terrain_sequence: capture.terrain_sequence,
            },
            store,
            payload: PersistencePayload::Save(Box::new(capture.snapshot)),
            reserved_bytes,
        })
    }

    pub fn request_restore(
        &self,
        planet_id: PlanetId,
        store: TerrainStore,
    ) -> Result<TerrainPersistenceRequestOutcome, TerrainPersistenceError> {
        self.preflight(planet_id, &store, TerrainPersistenceRequestKind::Restore)?;
        let capture = self.runtime.persistence_identity(planet_id)?;
        self.submit(PersistenceJob {
            ticket: TerrainPersistenceTicket {
                planet_id,
                request: 0,
                kind: TerrainPersistenceRequestKind::Restore,
            },
            identity: PersistenceIdentity {
                definition: capture.definition,
                planet_generation: capture.planet_generation,
                terrain_sequence: capture.terrain_sequence,
            },
            store,
            payload: PersistencePayload::Restore,
            reserved_bytes: self.shared.config.max_snapshot_bytes,
        })
    }

    fn preflight(
        &self,
        planet_id: PlanetId,
        store: &TerrainStore,
        kind: TerrainPersistenceRequestKind,
    ) -> Result<(), TerrainPersistenceError> {
        let mut state = lock(&self.shared.state);
        if !state.accepting {
            return Err(TerrainPersistenceError::NotRunning);
        }
        if state.completed.iter().any(|completion| {
            completion.ticket.planet_id == planet_id
                && completion.ticket.kind == TerrainPersistenceRequestKind::Restore
        }) || state.in_flight.is_some_and(|ticket| {
            ticket.planet_id == planet_id
                && (ticket.kind != TerrainPersistenceRequestKind::Save
                    || kind != TerrainPersistenceRequestKind::Save)
        }) {
            return Err(TerrainPersistenceError::PlanetBusy { planet_id });
        }
        if let Some(existing) = state.pending.get(&planet_id) {
            let coalesces = existing.ticket.kind == TerrainPersistenceRequestKind::Save
                && kind == TerrainPersistenceRequestKind::Save
                && existing.store_matches(store);
            if !coalesces {
                return Err(TerrainPersistenceError::PlanetBusy { planet_id });
            }
        } else if state.outstanding() >= self.shared.config.request_capacity {
            state.counters.backpressured = state.counters.backpressured.saturating_add(1);
            return Err(TerrainPersistenceError::RequestBackpressure {
                capacity: self.shared.config.request_capacity,
            });
        }
        Ok(())
    }

    fn submit(
        &self,
        mut job: PersistenceJob,
    ) -> Result<TerrainPersistenceRequestOutcome, TerrainPersistenceError> {
        let mut state = lock(&self.shared.state);
        if !state.accepting {
            return Err(TerrainPersistenceError::NotRunning);
        }
        let planet_id = job.ticket.planet_id;
        if state.completed.iter().any(|completion| {
            completion.ticket.planet_id == planet_id
                && completion.ticket.kind == TerrainPersistenceRequestKind::Restore
        }) {
            return Err(TerrainPersistenceError::PlanetBusy { planet_id });
        }
        if state.in_flight.is_some_and(|ticket| {
            ticket.planet_id == planet_id
                && (ticket.kind != TerrainPersistenceRequestKind::Save
                    || job.ticket.kind != TerrainPersistenceRequestKind::Save)
        }) {
            return Err(TerrainPersistenceError::PlanetBusy { planet_id });
        }

        if let Some(existing) = state.pending.get(&planet_id) {
            let coalesces = existing.ticket.kind == TerrainPersistenceRequestKind::Save
                && job.ticket.kind == TerrainPersistenceRequestKind::Save
                && existing.store_matches(&job.store);
            if !coalesces {
                return Err(TerrainPersistenceError::PlanetBusy { planet_id });
            }
            let old_bytes = existing.reserved_bytes;
            let adjusted = state
                .retained_snapshot_bytes
                .saturating_sub(old_bytes)
                .checked_add(job.reserved_bytes)
                .unwrap_or(usize::MAX);
            if adjusted > self.shared.config.max_retained_snapshot_bytes {
                state.counters.backpressured = state.counters.backpressured.saturating_add(1);
                return Err(TerrainPersistenceError::SnapshotByteBackpressure {
                    requested: job.reserved_bytes,
                    capacity: self.shared.config.max_retained_snapshot_bytes,
                });
            }
            let superseded = existing.ticket;
            job.ticket = state.allocate_ticket(planet_id, TerrainPersistenceRequestKind::Save)?;
            state.pending.insert(planet_id, job);
            state.retained_snapshot_bytes = adjusted;
            state.counters.submitted = state.counters.submitted.saturating_add(1);
            state.counters.coalesced = state.counters.coalesced.saturating_add(1);
            state.refresh_counts();
            self.shared.wake.notify_one();
            return Ok(TerrainPersistenceRequestOutcome::Coalesced {
                ticket: state
                    .pending
                    .get(&planet_id)
                    .expect("coalesced job was inserted")
                    .ticket,
                superseded,
            });
        }

        if state.outstanding() >= self.shared.config.request_capacity {
            state.counters.backpressured = state.counters.backpressured.saturating_add(1);
            return Err(TerrainPersistenceError::RequestBackpressure {
                capacity: self.shared.config.request_capacity,
            });
        }
        let total_bytes = state
            .retained_snapshot_bytes
            .checked_add(job.reserved_bytes)
            .unwrap_or(usize::MAX);
        if total_bytes > self.shared.config.max_retained_snapshot_bytes {
            state.counters.backpressured = state.counters.backpressured.saturating_add(1);
            return Err(TerrainPersistenceError::SnapshotByteBackpressure {
                requested: job.reserved_bytes,
                capacity: self.shared.config.max_retained_snapshot_bytes,
            });
        }
        job.ticket = state.allocate_ticket(planet_id, job.ticket.kind)?;
        let ticket = job.ticket;
        state.pending.insert(planet_id, job);
        state.retained_snapshot_bytes = total_bytes;
        state.counters.submitted = state.counters.submitted.saturating_add(1);
        state.refresh_counts();
        self.shared.wake.notify_one();
        Ok(TerrainPersistenceRequestOutcome::Queued(ticket))
    }

    pub fn pump(&self, maximum: usize) -> usize {
        let mut processed = 0;
        while processed < maximum {
            let completion = {
                let mut state = lock(&self.shared.state);
                if state.events.len() >= self.shared.config.event_capacity {
                    break;
                }
                state.completed.pop_front()
            };
            let Some(completion) = completion else {
                break;
            };
            let retained_bytes = completion.retained_bytes;
            let event = match completion.result {
                PersistenceWorkerResult::Saved {
                    store_generation,
                    snapshot_hash,
                } => TerrainPersistenceEvent::Saved {
                    ticket: completion.ticket,
                    store_generation,
                    snapshot_hash,
                    terrain_sequence: completion.identity.terrain_sequence,
                },
                PersistenceWorkerResult::RestoreMissing => {
                    TerrainPersistenceEvent::RestoreMissing {
                        ticket: completion.ticket,
                    }
                }
                PersistenceWorkerResult::Failed { kind, message } => {
                    TerrainPersistenceEvent::Failed {
                        ticket: completion.ticket,
                        kind,
                        message,
                    }
                }
                PersistenceWorkerResult::RestoreReady(mut ready) => {
                    match self.runtime.commit_persistence_restore(
                        completion.identity.definition.clone(),
                        completion.identity.planet_generation,
                        completion.identity.terrain_sequence,
                        &mut ready.core,
                    ) {
                        Ok(commit) => {
                            self.planning.retire_planet_generation(
                                completion.ticket.planet_id,
                                commit.retired_planet_generation,
                            );
                            TerrainPersistenceEvent::Restored {
                                ticket: completion.ticket,
                                store_generation: ready.record_generation,
                                snapshot_hash: ready.snapshot_hash,
                                retired_planet_generation: commit.retired_planet_generation,
                                planet_generation: commit.planet_generation,
                            }
                        }
                        Err(TerrainRuntimeError::EventBackpressure { .. }) => {
                            let mut state = lock(&self.shared.state);
                            state.completed.push_front(PersistenceCompletion {
                                ticket: completion.ticket,
                                identity: completion.identity,
                                result: PersistenceWorkerResult::RestoreReady(ready),
                                retained_bytes,
                            });
                            state.refresh_counts();
                            break;
                        }
                        Err(TerrainRuntimeError::StalePersistenceRestore { .. })
                        | Err(TerrainRuntimeError::PlanetMissing(_)) => {
                            TerrainPersistenceEvent::StaleRejected {
                                ticket: completion.ticket,
                            }
                        }
                        Err(error) => TerrainPersistenceEvent::Failed {
                            ticket: completion.ticket,
                            kind: TerrainPersistenceFailureKind::Runtime,
                            message: error.to_string(),
                        },
                    }
                }
            };
            let mut state = lock(&self.shared.state);
            state.retained_snapshot_bytes =
                state.retained_snapshot_bytes.saturating_sub(retained_bytes);
            match &event {
                TerrainPersistenceEvent::Saved { .. } => {
                    state.counters.saved = state.counters.saved.saturating_add(1)
                }
                TerrainPersistenceEvent::Restored { .. } => {
                    state.counters.restored = state.counters.restored.saturating_add(1)
                }
                TerrainPersistenceEvent::RestoreMissing { .. } => {
                    state.counters.missing = state.counters.missing.saturating_add(1)
                }
                TerrainPersistenceEvent::StaleRejected { .. } => {
                    state.counters.stale_rejected = state.counters.stale_rejected.saturating_add(1)
                }
                TerrainPersistenceEvent::Failed { .. } => {
                    state.counters.errors = state.counters.errors.saturating_add(1)
                }
            }
            state.events.push_back(event);
            state.refresh_counts();
            processed += 1;
        }
        processed
    }

    pub fn drain_events(&self, maximum: usize) -> Vec<TerrainPersistenceEvent> {
        let mut state = lock(&self.shared.state);
        let count = maximum.min(state.events.len());
        let events = state.events.drain(..count).collect();
        state.refresh_counts();
        events
    }

    pub fn counters(&self) -> TerrainPersistenceCounters {
        let mut state = lock(&self.shared.state);
        state.refresh_counts();
        state.counters
    }
}

pub(crate) struct TerrainPersistenceService {
    handle: TerrainPersistenceHandle,
    worker: Option<JoinHandle<()>>,
}

impl TerrainPersistenceService {
    pub(crate) fn new(
        runtime: TerrainRuntimeHandle,
        planning: TerrainPlanningHandle,
        config: TerrainPersistenceConfig,
    ) -> Result<Self, TerrainPersistenceError> {
        let config = config.validate()?;
        Ok(Self {
            handle: TerrainPersistenceHandle {
                shared: Arc::new(PersistenceShared::new(config)),
                runtime,
                planning,
            },
            worker: None,
        })
    }

    pub(crate) fn handle(&self) -> TerrainPersistenceHandle {
        self.handle.clone()
    }

    pub(crate) fn initialize(&mut self) -> Result<(), TerrainPersistenceError> {
        {
            let mut state = lock(&self.handle.shared.state);
            state.accepting = true;
            state.stopping = false;
        }
        let shared = self.handle.shared.clone();
        self.worker = Some(
            thread::Builder::new()
                .name("Pulsar-Terrain-Persistence".to_string())
                .spawn(move || persistence_worker_loop(shared))
                .map_err(|_| {
                    let mut state = lock(&self.handle.shared.state);
                    state.accepting = false;
                    state.stopping = true;
                    TerrainPersistenceError::ThreadSpawn
                })?,
        );
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) {
        {
            let mut state = lock(&self.handle.shared.state);
            state.accepting = false;
            state.stopping = true;
            self.handle.shared.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        while self.handle.pump(usize::MAX) > 0 {}
    }
}

fn persistence_worker_loop(shared: Arc<PersistenceShared>) {
    while let Some(job) = shared.pop() {
        let result = execute_job(&job, shared.config.max_snapshot_bytes);
        shared.finish(job, result);
    }
}

fn execute_job(job: &PersistenceJob, max_snapshot_bytes: usize) -> PersistenceWorkerResult {
    match &job.payload {
        PersistencePayload::Save(snapshot) => {
            let bytes = match snapshot.encode() {
                Ok(bytes) => bytes,
                Err(error) => {
                    return PersistenceWorkerResult::Failed {
                        kind: TerrainPersistenceFailureKind::Snapshot,
                        message: error.to_string(),
                    };
                }
            };
            if bytes.len() > max_snapshot_bytes {
                return PersistenceWorkerResult::Failed {
                    kind: TerrainPersistenceFailureKind::SnapshotTooLarge,
                    message: format!(
                        "encoded snapshot is {} bytes, budget is {max_snapshot_bytes}",
                        bytes.len()
                    ),
                };
            }
            match job.store.save(&bytes) {
                Ok(record) => PersistenceWorkerResult::Saved {
                    store_generation: record.generation,
                    snapshot_hash: record.hash,
                },
                Err(error) => PersistenceWorkerResult::Failed {
                    kind: TerrainPersistenceFailureKind::Store,
                    message: error.to_string(),
                },
            }
        }
        PersistencePayload::Restore => match job.store.load_latest_snapshot() {
            Ok(None) => PersistenceWorkerResult::RestoreMissing,
            Err(error) => PersistenceWorkerResult::Failed {
                kind: TerrainPersistenceFailureKind::Store,
                message: error.to_string(),
            },
            Ok(Some((record, snapshot))) => {
                if record.bytes.len() > max_snapshot_bytes {
                    return PersistenceWorkerResult::Failed {
                        kind: TerrainPersistenceFailureKind::SnapshotTooLarge,
                        message: format!(
                            "stored snapshot is {} bytes, budget is {max_snapshot_bytes}",
                            record.bytes.len()
                        ),
                    };
                }
                if snapshot.planet_id != job.identity.definition.planet_id {
                    return PersistenceWorkerResult::Failed {
                        kind: TerrainPersistenceFailureKind::PlanetIdentity,
                        message: "stored snapshot belongs to a different planet".to_string(),
                    };
                }
                let generator = FixedSphereGenerator {
                    center_cell: job.identity.definition.center_cell,
                    radius_cells: job.identity.definition.radius_cells,
                    material: job.identity.definition.material,
                };
                if snapshot.generator_hash != generator.hash()
                    || snapshot.hierarchy.root_lod() != job.identity.definition.root_lod
                {
                    return PersistenceWorkerResult::Failed {
                        kind: TerrainPersistenceFailureKind::GeneratorDefinition,
                        message: "stored snapshot generator does not match the live definition"
                            .to_string(),
                    };
                }
                let retained_bytes = record.bytes.len();
                match TerrainCore::from_snapshot(snapshot, generator) {
                    Ok(core) => PersistenceWorkerResult::RestoreReady(Box::new(RestoreReady {
                        record_generation: record.generation,
                        snapshot_hash: record.hash,
                        retained_bytes,
                        core: Some(core),
                    })),
                    Err(error) => PersistenceWorkerResult::Failed {
                        kind: TerrainPersistenceFailureKind::Snapshot,
                        message: error.to_string(),
                    },
                }
            }
        },
    }
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
        EditMode, EditOp, EditShape, NodeState, PlanetPosition, PlanetView, TerrainPlanningConfig,
        TerrainRequestClass, TerrainRuntimeConfig, TerrainRuntimeEvent, TerrainSubsystem,
    };
    use engine_fs::virtual_fs;
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::time::{Duration, Instant};

    fn runtime_config() -> TerrainRuntimeConfig {
        TerrainRuntimeConfig {
            worker_count: 1,
            max_planets: 4,
            max_component_sources: 4,
            request_capacity: 8,
            critical_request_reserve: 2,
            completion_capacity: 8,
            event_capacity: 32,
            max_resident_pages: 16,
            max_resident_dense_bytes: 16 * crate::CELL_COUNT * size_of::<crate::CellWord>(),
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
            max_resident_pages: 16,
        }
    }

    fn start(
        config: TerrainPersistenceConfig,
    ) -> (
        TerrainSubsystem,
        TerrainRuntimeHandle,
        TerrainPersistenceHandle,
    ) {
        let mut subsystem =
            TerrainSubsystem::new_with_persistence(runtime_config(), config).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let runtime = subsystem.runtime_handle();
        let persistence = subsystem.persistence_handle();
        (subsystem, runtime, persistence)
    }

    fn start_with_runtime(
        runtime_config: TerrainRuntimeConfig,
        persistence_config: TerrainPersistenceConfig,
    ) -> (
        TerrainSubsystem,
        TerrainRuntimeHandle,
        TerrainPersistenceHandle,
    ) {
        let mut subsystem =
            TerrainSubsystem::new_with_persistence(runtime_config, persistence_config).unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let runtime = subsystem.runtime_handle();
        let persistence = subsystem.persistence_handle();
        (subsystem, runtime, persistence)
    }

    fn wait_for_completed(handle: &TerrainPersistenceHandle, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.counters().completed < count && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            handle.counters().completed >= count,
            "timed out waiting for persistence completion: {:?}",
            handle.counters()
        );
    }

    fn wait_for_planning_completed(handle: &TerrainPlanningHandle, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.counters().completed < count && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            handle.counters().completed >= count,
            "timed out waiting for planning completion: {:?}",
            handle.counters()
        );
    }

    fn wait_for_page(
        runtime: &TerrainRuntimeHandle,
        planet_id: PlanetId,
        page_key: crate::PageKey,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.pump(8);
            if runtime.drain_events(8).into_iter().any(|event| {
                matches!(
                    event,
                    TerrainRuntimeEvent::PageReady {
                        planet_id: ready_planet,
                        page_key: ready_key,
                        ..
                    } if ready_planet == planet_id && ready_key == page_key
                )
            }) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for page");
            thread::yield_now();
        }
    }

    fn wait_for_persistence_events(
        handle: &TerrainPersistenceHandle,
        count: usize,
    ) -> Vec<TerrainPersistenceEvent> {
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
            "timed out waiting for persistence events: {:?}",
            handle.counters()
        );
        events
    }

    #[test]
    fn save_persists_the_exact_captured_sequence_while_later_edits_continue() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(1);
        runtime.upsert_planet(definition.clone()).unwrap();
        let captured = runtime.persistence_capture(definition.planet_id).unwrap();
        let captured_hash = captured.snapshot.content_hash().unwrap();

        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        runtime
            .set_region(
                definition.planet_id,
                crate::PageKey::new(4, [0; 3]),
                NodeState::Air,
            )
            .unwrap();

        let events = wait_for_persistence_events(&persistence, 1);
        assert!(matches!(
            events.as_slice(),
            [TerrainPersistenceEvent::Saved {
                snapshot_hash,
                terrain_sequence: 0,
                ..
            }] if *snapshot_hash == captured_hash
        ));
        let (_, stored) = store.load_latest_snapshot().unwrap().unwrap();
        assert_eq!(stored.content_hash().unwrap(), captured_hash);
        assert_ne!(
            runtime
                .persistence_capture(definition.planet_id)
                .unwrap()
                .snapshot
                .content_hash()
                .unwrap(),
            captured_hash
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn restore_atomically_advances_generation_and_retires_visibility_once() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(2);
        runtime.upsert_planet(definition.clone()).unwrap();
        runtime
            .set_root(definition.planet_id, NodeState::Air)
            .unwrap();
        runtime.drain_events(64);
        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        let saved = wait_for_persistence_events(&persistence, 1);
        let saved_hash = match saved[0] {
            TerrainPersistenceEvent::Saved { snapshot_hash, .. } => snapshot_hash,
            ref other => panic!("unexpected persistence event: {other:?}"),
        };

        runtime
            .set_root(definition.planet_id, NodeState::Solid(9))
            .unwrap();
        runtime.drain_events(64);
        let planning = subsystem.planning_handle();
        planning
            .submit(
                definition.planet_id,
                PlanetView::new(
                    PlanetPosition::from_lod0_cell([200, 0, 0]),
                    [-1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    60_f64.to_radians(),
                    [1280, 720],
                    0.1,
                    10_000.0,
                    [0.0; 3],
                )
                .unwrap(),
                TerrainPlanningConfig::default(),
            )
            .unwrap();
        wait_for_planning_completed(&planning, 1);
        let before_restore = runtime.planet_generation(definition.planet_id).unwrap();
        persistence
            .request_restore(definition.planet_id, store)
            .unwrap();
        let restored = wait_for_persistence_events(&persistence, 1);
        let after_restore = runtime.planet_generation(definition.planet_id).unwrap();
        assert!(after_restore > before_restore);
        assert!(matches!(
            restored.as_slice(),
            [TerrainPersistenceEvent::Restored {
                snapshot_hash,
                retired_planet_generation,
                planet_generation,
                ..
            }] if *snapshot_hash == saved_hash
                && *retired_planet_generation == before_restore
                && *planet_generation == after_restore
        ));
        assert_eq!(
            runtime
                .persistence_capture(definition.planet_id)
                .unwrap()
                .snapshot
                .content_hash()
                .unwrap(),
            saved_hash
        );
        assert_eq!(
            runtime.drain_events(64),
            vec![TerrainRuntimeEvent::EvictPlanet {
                planet_id: definition.planet_id,
                retired_generation: before_restore,
            }]
        );
        assert_eq!(planning.counters().completed, 0);
        assert!(planning.counters().cancelled >= 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn live_restore_reproduces_edits_overrides_summaries_and_compacted_pages() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(12);
        runtime.upsert_planet(definition.clone()).unwrap();
        runtime
            .append_edit(
                definition.planet_id,
                EditOp {
                    sequence: 1,
                    stable_id: [0xA5; 16],
                    shape: EditShape::Sphere {
                        center_cell: [96, 0, 0],
                        radius_cells: 8,
                    },
                    mode: EditMode::Subtract,
                    material: 0,
                },
            )
            .unwrap();
        runtime
            .set_region(
                definition.planet_id,
                crate::PageKey::new(2, [-1, 0, 0]),
                NodeState::Solid(12),
            )
            .unwrap();
        let page_key = crate::PageKey::new(0, [6, 0, 0]);
        runtime
            .request_page(
                definition.planet_id,
                page_key,
                TerrainRequestClass::Collision,
                0,
            )
            .unwrap();
        wait_for_page(&runtime, definition.planet_id, page_key);
        let expected = runtime.persistence_capture(definition.planet_id).unwrap();
        let expected_hash = expected.snapshot.content_hash().unwrap();
        assert!(!expected.snapshot.compacted_pages.is_empty());

        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        wait_for_persistence_events(&persistence, 1);
        runtime
            .set_root(definition.planet_id, NodeState::Air)
            .unwrap();
        runtime.drain_events(64);
        persistence
            .request_restore(definition.planet_id, store)
            .unwrap();
        assert!(matches!(
            wait_for_persistence_events(&persistence, 1).as_slice(),
            [TerrainPersistenceEvent::Restored { snapshot_hash, .. }]
                if *snapshot_hash == expected_hash
        ));
        let restored = runtime.persistence_capture(definition.planet_id).unwrap();
        assert_eq!(restored.snapshot.content_hash().unwrap(), expected_hash);
        assert_eq!(restored.snapshot, expected.snapshot);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn restore_rejects_a_completion_after_live_state_changes() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(3);
        runtime.upsert_planet(definition.clone()).unwrap();
        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        wait_for_persistence_events(&persistence, 1);

        let generation = runtime.planet_generation(definition.planet_id).unwrap();
        persistence
            .request_restore(definition.planet_id, store)
            .unwrap();
        runtime
            .set_region(
                definition.planet_id,
                crate::PageKey::new(4, [0; 3]),
                NodeState::Air,
            )
            .unwrap();
        let events = wait_for_persistence_events(&persistence, 1);
        assert!(matches!(
            events.as_slice(),
            [TerrainPersistenceEvent::StaleRejected { .. }]
        ));
        assert_eq!(
            runtime.planet_generation(definition.planet_id),
            Some(generation)
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn missing_and_wrong_planet_stores_return_typed_events() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let missing = TerrainStore::new(temporary.path().join("missing"));
        let stored = TerrainStore::new(temporary.path().join("stored"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let first = planet(4);
        let second = planet(5);
        runtime.upsert_planet(first.clone()).unwrap();
        runtime.upsert_planet(second.clone()).unwrap();

        persistence
            .request_restore(first.planet_id, missing)
            .unwrap();
        assert!(matches!(
            wait_for_persistence_events(&persistence, 1).as_slice(),
            [TerrainPersistenceEvent::RestoreMissing { .. }]
        ));
        persistence
            .request_save(first.planet_id, stored.clone())
            .unwrap();
        wait_for_persistence_events(&persistence, 1);
        persistence
            .request_restore(second.planet_id, stored)
            .unwrap();
        assert!(matches!(
            wait_for_persistence_events(&persistence, 1).as_slice(),
            [TerrainPersistenceEvent::Failed {
                kind: TerrainPersistenceFailureKind::PlanetIdentity,
                ..
            }]
        ));
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn pending_saves_coalesce_and_global_outstanding_work_is_bounded() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, _) = start(TerrainPersistenceConfig::default());
        for id in 6..=8 {
            runtime.upsert_planet(planet(id)).unwrap();
        }
        let config = TerrainPersistenceConfig {
            request_capacity: 2,
            ..TerrainPersistenceConfig::default()
        };
        let shared = Arc::new(PersistenceShared::new(config));
        lock(&shared.state).accepting = true;
        let handle = TerrainPersistenceHandle {
            shared,
            runtime,
            planning: subsystem.planning_handle(),
        };

        let first = handle
            .request_save(PlanetId([6; 16]), store.clone())
            .unwrap()
            .ticket();
        let second = handle
            .request_save(PlanetId([6; 16]), store.clone())
            .unwrap();
        assert!(matches!(
            second,
            TerrainPersistenceRequestOutcome::Coalesced { superseded, .. }
                if superseded == first
        ));
        handle
            .request_save(PlanetId([7; 16]), store.clone())
            .unwrap();
        assert!(matches!(
            handle.request_save(PlanetId([8; 16]), store),
            Err(TerrainPersistenceError::RequestBackpressure { capacity: 2 })
        ));
        let counters = handle.counters();
        assert_eq!(counters.pending, 2);
        assert_eq!(counters.outstanding, 2);
        assert_eq!(counters.coalesced, 1);
        assert_eq!(counters.backpressured, 1);
        assert!(counters.retained_snapshot_bytes <= config.max_retained_snapshot_bytes);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn completed_restore_keeps_the_planet_busy_until_atomic_commit() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(9);
        runtime.upsert_planet(definition.clone()).unwrap();
        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        wait_for_persistence_events(&persistence, 1);

        persistence
            .request_restore(definition.planet_id, store.clone())
            .unwrap();
        wait_for_completed(&persistence, 1);
        assert!(matches!(
            persistence.request_save(definition.planet_id, store.clone()),
            Err(TerrainPersistenceError::PlanetBusy { planet_id })
                if planet_id == definition.planet_id
        ));

        assert_eq!(persistence.pump(1), 1);
        assert!(matches!(
            persistence.drain_events(1).as_slice(),
            [TerrainPersistenceEvent::Restored { .. }]
        ));
        persistence
            .request_save(definition.planet_id, store)
            .unwrap();
        wait_for_persistence_events(&persistence, 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn restore_retries_without_data_loss_when_runtime_events_are_backpressured() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let mut constrained_runtime = runtime_config();
        constrained_runtime.event_capacity = 1;
        let (mut subsystem, runtime, persistence) =
            start_with_runtime(constrained_runtime, TerrainPersistenceConfig::default());
        let definition = planet(10);
        runtime.upsert_planet(definition.clone()).unwrap();
        runtime
            .set_root(definition.planet_id, NodeState::Air)
            .unwrap();
        runtime.drain_events(1);
        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();
        wait_for_persistence_events(&persistence, 1);

        runtime
            .set_root(definition.planet_id, NodeState::Solid(10))
            .unwrap();
        let retired_generation = runtime.planet_generation(definition.planet_id).unwrap();
        persistence
            .request_restore(definition.planet_id, store)
            .unwrap();
        wait_for_completed(&persistence, 1);
        assert_eq!(persistence.pump(1), 0);
        assert_eq!(persistence.counters().completed, 1);

        assert_eq!(runtime.drain_events(1).len(), 1);
        assert_eq!(persistence.pump(1), 1);
        assert!(matches!(
            persistence.drain_events(1).as_slice(),
            [TerrainPersistenceEvent::Restored {
                retired_planet_generation,
                ..
            }] if *retired_planet_generation == retired_generation
        ));
        assert_eq!(
            runtime.drain_events(1),
            vec![TerrainRuntimeEvent::EvictPlanet {
                planet_id: definition.planet_id,
                retired_generation,
            }]
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn shutdown_drains_every_accepted_save_before_the_worker_stops() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let (mut subsystem, runtime, persistence) = start(TerrainPersistenceConfig::default());
        let definition = planet(11);
        runtime.upsert_planet(definition.clone()).unwrap();
        let expected_hash = runtime
            .persistence_capture(definition.planet_id)
            .unwrap()
            .snapshot
            .content_hash()
            .unwrap();
        persistence
            .request_save(definition.planet_id, store.clone())
            .unwrap();

        subsystem.shutdown().unwrap();

        let (_, stored) = store.load_latest_snapshot().unwrap().unwrap();
        assert_eq!(stored.content_hash().unwrap(), expected_hash);
        assert!(matches!(
            persistence.drain_events(1).as_slice(),
            [TerrainPersistenceEvent::Saved { snapshot_hash, .. }]
                if *snapshot_hash == expected_hash
        ));
        assert_eq!(persistence.counters().outstanding, 0);
    }
}
