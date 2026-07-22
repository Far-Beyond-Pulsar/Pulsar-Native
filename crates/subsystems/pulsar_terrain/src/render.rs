//! Bounded immutable handoff from authoritative terrain state to rendering.
//!
//! The types deliberately mirror Helio's planetary protocol without depending
//! on an unmerged Helio revision:
//!
//! - [`TerrainPageUpload`] -> `helio_planet_voxel_core::PageUpload`
//! - [`TerrainPageEvict`] -> `helio_planet_voxel_core::PageEvict`
//! - [`TerrainVisiblePage`] -> `helio_planet_voxel_core::VisiblePage`
//! - [`TerrainVisiblePageSet`] -> `helio_planet_voxel_core::VisiblePageSet`
//!
//! Runtime event draining remains explicit at the caller boundary. This module
//! only translates the supplied slice and never steals events from persistence,
//! collision, replication, or tooling consumers.

use crate::{
    CellWord, PageKey, PlanetId, TerrainRequestClass, TerrainResidencySession,
    TerrainResidentPageGeneration, TerrainRuntimeEvent, TerrainRuntimeHandle, TerrainStreamingPlan,
    CELL_COUNT,
};
use std::collections::BTreeMap;
use thiserror::Error;

pub const TERRAIN_TRANSITION_FACE_MASK: u8 = 0b00_111111;
const PAGE_UPLOAD_BYTES: usize = CELL_COUNT * std::mem::size_of::<CellWord>();

/// Stable transition bit order shared with Helio and its WGSL layouts.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TerrainTransitionFace {
    #[default]
    NegativeX = 0,
    PositiveX = 1,
    NegativeY = 2,
    PositiveY = 3,
    NegativeZ = 4,
    PositiveZ = 5,
}

impl TerrainTransitionFace {
    pub const ALL: [Self; 6] = [
        Self::NegativeX,
        Self::PositiveX,
        Self::NegativeY,
        Self::PositiveY,
        Self::NegativeZ,
        Self::PositiveZ,
    ];

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn bit(self) -> u8 {
        1 << self.index()
    }

    pub const fn axis(self) -> usize {
        (self.index() / 2) as usize
    }

    pub const fn is_positive(self) -> bool {
        self.index() & 1 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainRenderDeltaConfig {
    pub max_events_per_delta: usize,
    pub max_commands_per_delta: usize,
    pub max_upload_bytes_per_delta: usize,
    pub max_tracked_pages: usize,
    pub max_visible_pages: usize,
}

impl Default for TerrainRenderDeltaConfig {
    fn default() -> Self {
        Self {
            max_events_per_delta: 64,
            max_commands_per_delta: 64,
            max_upload_bytes_per_delta: 8 * 1024 * 1024,
            max_tracked_pages: 8_192,
            max_visible_pages: 2_048,
        }
    }
}

impl TerrainRenderDeltaConfig {
    fn validate(self) -> Result<Self, TerrainRenderDeltaError> {
        if self.max_events_per_delta == 0
            || self.max_commands_per_delta == 0
            || self.max_tracked_pages == 0
            || self.max_visible_pages == 0
        {
            return Err(TerrainRenderDeltaError::InvalidConfig(
                "event, command, tracked-page, and visible-page limits must be non-zero",
            ));
        }
        if self.max_upload_bytes_per_delta < PAGE_UPLOAD_BYTES {
            return Err(TerrainRenderDeltaError::InvalidConfig(
                "upload-byte limit must hold at least one complete page",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainPageUpload {
    pub planet_id: PlanetId,
    pub page_key: PageKey,
    pub planet_generation: u64,
    pub page_generation: u64,
    pub cells: Box<[CellWord]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainPageEvict {
    pub planet_id: PlanetId,
    pub page_key: PageKey,
    pub planet_generation: u64,
    pub page_generation: u64,
}

/// One bounded planet retirement expands to the exact renderer-owned page set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainPlanetEvict {
    pub planet_id: PlanetId,
    pub retired_planet_generation: u64,
    pub pages: Vec<TerrainPageEvict>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainRenderCommand {
    Upload(TerrainPageUpload),
    EvictPage(TerrainPageEvict),
    EvictPlanet(TerrainPlanetEvict),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainVisiblePage {
    pub page_key: PageKey,
    pub planet_generation: u64,
    pub page_generation: u64,
    pub transition_mask: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainVisiblePageSet {
    pub planet_id: PlanetId,
    pub frame_index: u64,
    pub pages: Vec<TerrainVisiblePage>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainRenderDeltaCounters {
    pub input_events: usize,
    pub commands: usize,
    pub upload_pages: usize,
    pub upload_bytes: usize,
    pub page_evictions: usize,
    pub planet_evictions: usize,
    pub stale_page_ready: usize,
    pub ignored_events: usize,
    pub tracked_pages: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainRenderDelta {
    pub commands: Vec<TerrainRenderCommand>,
    pub counters: TerrainRenderDeltaCounters,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TerrainRenderDeltaError {
    #[error("invalid terrain render-delta configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("render delta contains {actual} runtime events; configured maximum is {maximum}")]
    EventBudget { actual: usize, maximum: usize },
    #[error("render delta requires {actual} commands; configured maximum is {maximum}")]
    CommandBudget { actual: usize, maximum: usize },
    #[error("render delta requires {actual} upload bytes; configured maximum is {maximum}")]
    UploadByteBudget { actual: usize, maximum: usize },
    #[error("render publisher would track {actual} pages; configured maximum is {maximum}")]
    TrackedPageBudget { actual: usize, maximum: usize },
    #[error("visible set requires {actual} pages; configured maximum is {maximum}")]
    VisiblePageBudget { actual: usize, maximum: usize },
    #[error("terrain residency has not committed the requested streaming plan")]
    PlanNotCommitted,
    #[error("committed page {page_key:?} on planet {planet_id:?} is not resident")]
    PageNotResident {
        planet_id: PlanetId,
        page_key: PageKey,
    },
    #[error("committed page {page_key:?} on planet {planet_id:?} has not been uploaded at its current generation")]
    PageNotUploaded {
        planet_id: PlanetId,
        page_key: PageKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedGeneration {
    planet_generation: u64,
    page_generation: u64,
}

type TrackedKey = (PlanetId, PageKey);

/// Stateful, bounded translator from explicit runtime events to immutable
/// renderer commands. Canonical terrain remains owned by `TerrainRuntimeHandle`.
pub struct TerrainRenderDeltaPublisher {
    config: TerrainRenderDeltaConfig,
    tracked: BTreeMap<TrackedKey, TrackedGeneration>,
}

impl TerrainRenderDeltaPublisher {
    pub fn new(config: TerrainRenderDeltaConfig) -> Result<Self, TerrainRenderDeltaError> {
        Ok(Self {
            config: config.validate()?,
            tracked: BTreeMap::new(),
        })
    }

    pub const fn config(&self) -> TerrainRenderDeltaConfig {
        self.config
    }

    pub fn tracked_page_count(&self) -> usize {
        self.tracked.len()
    }

    /// Translate one caller-owned event slice transactionally. On error, the
    /// publisher retains exactly its previous tracked state.
    pub fn translate_events(
        &mut self,
        runtime: &TerrainRuntimeHandle,
        events: &[TerrainRuntimeEvent],
    ) -> Result<TerrainRenderDelta, TerrainRenderDeltaError> {
        if events.len() > self.config.max_events_per_delta {
            return Err(TerrainRenderDeltaError::EventBudget {
                actual: events.len(),
                maximum: self.config.max_events_per_delta,
            });
        }

        let mut commands = Vec::new();
        let mut counters = TerrainRenderDeltaCounters {
            input_events: events.len(),
            ..TerrainRenderDeltaCounters::default()
        };
        // Only touched keys are copied. `None` means removal.
        let mut overlay = BTreeMap::<TrackedKey, Option<TrackedGeneration>>::new();
        let mut tracked_count = self.tracked.len();

        for event in events {
            match event {
                TerrainRuntimeEvent::PageReady {
                    planet_id,
                    page_key,
                    planet_generation,
                    page_generation,
                    ..
                } => {
                    let expected = TerrainResidentPageGeneration {
                        planet_generation: *planet_generation,
                        page_generation: *page_generation,
                    };
                    let Some(page) =
                        runtime.page_snapshot_for_generation(*planet_id, *page_key, expected)
                    else {
                        counters.stale_page_ready = counters.stale_page_ready.saturating_add(1);
                        continue;
                    };
                    let next_upload_bytes = counters
                        .upload_bytes
                        .checked_add(PAGE_UPLOAD_BYTES)
                        .unwrap_or(usize::MAX);
                    if next_upload_bytes > self.config.max_upload_bytes_per_delta {
                        return Err(TerrainRenderDeltaError::UploadByteBudget {
                            actual: next_upload_bytes,
                            maximum: self.config.max_upload_bytes_per_delta,
                        });
                    }
                    let key = (*planet_id, *page_key);
                    if effective_generation(&self.tracked, &overlay, key).is_none() {
                        tracked_count = tracked_count.saturating_add(1);
                        if tracked_count > self.config.max_tracked_pages {
                            return Err(TerrainRenderDeltaError::TrackedPageBudget {
                                actual: tracked_count,
                                maximum: self.config.max_tracked_pages,
                            });
                        }
                    }
                    let generation = TrackedGeneration {
                        planet_generation: *planet_generation,
                        page_generation: *page_generation,
                    };
                    overlay.insert(key, Some(generation));
                    commands.push(TerrainRenderCommand::Upload(TerrainPageUpload {
                        planet_id: *planet_id,
                        page_key: *page_key,
                        planet_generation: *planet_generation,
                        page_generation: *page_generation,
                        cells: page.cells().collect::<Vec<_>>().into_boxed_slice(),
                    }));
                    counters.upload_pages = counters.upload_pages.saturating_add(1);
                    counters.upload_bytes = next_upload_bytes;
                }
                TerrainRuntimeEvent::EvictPage {
                    planet_id,
                    page_key,
                    planet_generation,
                    retired_page_generation,
                } => {
                    let key = (*planet_id, *page_key);
                    if effective_generation(&self.tracked, &overlay, key).is_some_and(|current| {
                        current.planet_generation == *planet_generation
                            && current.page_generation <= *retired_page_generation
                    }) {
                        overlay.insert(key, None);
                        tracked_count = tracked_count.saturating_sub(1);
                    }
                    commands.push(TerrainRenderCommand::EvictPage(TerrainPageEvict {
                        planet_id: *planet_id,
                        page_key: *page_key,
                        planet_generation: *planet_generation,
                        page_generation: *retired_page_generation,
                    }));
                    counters.page_evictions = counters.page_evictions.saturating_add(1);
                }
                TerrainRuntimeEvent::EvictPlanet {
                    planet_id,
                    retired_generation,
                } => {
                    let pages = effective_planet_pages(
                        &self.tracked,
                        &overlay,
                        *planet_id,
                        *retired_generation,
                    );
                    for page in &pages {
                        let key = (page.planet_id, page.page_key);
                        if effective_generation(&self.tracked, &overlay, key).is_some() {
                            overlay.insert(key, None);
                            tracked_count = tracked_count.saturating_sub(1);
                        }
                    }
                    counters.page_evictions = counters.page_evictions.saturating_add(pages.len());
                    counters.planet_evictions = counters.planet_evictions.saturating_add(1);
                    commands.push(TerrainRenderCommand::EvictPlanet(TerrainPlanetEvict {
                        planet_id: *planet_id,
                        retired_planet_generation: *retired_generation,
                        pages,
                    }));
                }
                TerrainRuntimeEvent::StaleRejected { .. }
                | TerrainRuntimeEvent::Backpressure { .. }
                | TerrainRuntimeEvent::Error { .. } => {
                    counters.ignored_events = counters.ignored_events.saturating_add(1);
                }
            }
            if commands.len() > self.config.max_commands_per_delta {
                return Err(TerrainRenderDeltaError::CommandBudget {
                    actual: commands.len(),
                    maximum: self.config.max_commands_per_delta,
                });
            }
        }

        for (key, generation) in overlay {
            if let Some(generation) = generation {
                self.tracked.insert(key, generation);
            } else {
                self.tracked.remove(&key);
            }
        }
        counters.commands = commands.len();
        counters.tracked_pages = self.tracked.len();
        Ok(TerrainRenderDelta { commands, counters })
    }

    /// Publish the stable visible set only after the exact streaming plan has
    /// completed its residency handoff and every visible generation has been
    /// translated into an upload command.
    pub fn visible_set(
        &self,
        runtime: &TerrainRuntimeHandle,
        session: &TerrainResidencySession,
        plan: &TerrainStreamingPlan,
        frame_index: u64,
    ) -> Result<TerrainVisiblePageSet, TerrainRenderDeltaError> {
        if !session.has_committed_plan(plan) {
            return Err(TerrainRenderDeltaError::PlanNotCommitted);
        }
        let visible_count = plan
            .demands()
            .iter()
            .filter(|demand| demand.request_class() == TerrainRequestClass::Visible)
            .count();
        if visible_count > self.config.max_visible_pages {
            return Err(TerrainRenderDeltaError::VisiblePageBudget {
                actual: visible_count,
                maximum: self.config.max_visible_pages,
            });
        }
        let masks = plan.transition_masks();
        let mut pages = Vec::with_capacity(visible_count);
        for demand in plan
            .demands()
            .iter()
            .filter(|demand| demand.request_class() == TerrainRequestClass::Visible)
        {
            let page_key = demand.page_key();
            let generation = runtime
                .resident_page_generation(plan.planet_id(), page_key)
                .ok_or(TerrainRenderDeltaError::PageNotResident {
                    planet_id: plan.planet_id(),
                    page_key,
                })?;
            let tracked = self.tracked.get(&(plan.planet_id(), page_key));
            if tracked
                != Some(&TrackedGeneration {
                    planet_generation: generation.planet_generation,
                    page_generation: generation.page_generation,
                })
            {
                return Err(TerrainRenderDeltaError::PageNotUploaded {
                    planet_id: plan.planet_id(),
                    page_key,
                });
            }
            pages.push(TerrainVisiblePage {
                page_key,
                planet_generation: generation.planet_generation,
                page_generation: generation.page_generation,
                transition_mask: masks.get(&page_key).copied().unwrap_or(0),
            });
        }
        pages.sort_unstable_by_key(|page| page.page_key);
        Ok(TerrainVisiblePageSet {
            planet_id: plan.planet_id(),
            frame_index,
            pages,
        })
    }
}

fn effective_generation(
    tracked: &BTreeMap<TrackedKey, TrackedGeneration>,
    overlay: &BTreeMap<TrackedKey, Option<TrackedGeneration>>,
    key: TrackedKey,
) -> Option<TrackedGeneration> {
    match overlay.get(&key) {
        Some(generation) => *generation,
        None => tracked.get(&key).copied(),
    }
}

fn effective_planet_pages(
    tracked: &BTreeMap<TrackedKey, TrackedGeneration>,
    overlay: &BTreeMap<TrackedKey, Option<TrackedGeneration>>,
    planet_id: PlanetId,
    retired_planet_generation: u64,
) -> Vec<TerrainPageEvict> {
    let mut pages = BTreeMap::<PageKey, TrackedGeneration>::new();
    for ((planet, page), generation) in tracked {
        if *planet == planet_id && generation.planet_generation <= retired_planet_generation {
            pages.insert(*page, *generation);
        }
    }
    for ((planet, page), generation) in overlay {
        if *planet != planet_id {
            continue;
        }
        match generation {
            Some(generation) if generation.planet_generation <= retired_planet_generation => {
                pages.insert(*page, *generation);
            }
            Some(_) | None => {
                pages.remove(page);
            }
        }
    }
    pages
        .into_iter()
        .map(|(page_key, generation)| TerrainPageEvict {
            planet_id,
            page_key,
            planet_generation: generation.planet_generation,
            page_generation: generation.page_generation,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EditMode, EditOp, EditShape, PageDemand, PlanetDefinition, PlanetPosition, PlanetView,
        TerrainResidencyConfig, TerrainRuntimeConfig, TerrainStreamingConfig,
        TerrainStreamingPlanner, TerrainSubsystem,
    };
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::thread;
    use std::time::{Duration, Instant};

    const DENSE_PAGE_BYTES: usize = CELL_COUNT * std::mem::size_of::<CellWord>();

    fn definition(id: u8) -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId([id; 16]),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: id.max(1),
            root_lod: 6,
            max_resident_pages: 64,
        }
    }

    fn start() -> TerrainSubsystem {
        let mut subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 4,
            max_planets: 2,
            max_component_sources: 2,
            request_capacity: 64,
            critical_request_reserve: 4,
            completion_capacity: 64,
            event_capacity: 128,
            max_resident_pages: 64,
            max_resident_dense_bytes: 64 * DENSE_PAGE_BYTES,
            max_completions_per_frame: 64,
        })
        .unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        subsystem
    }

    fn wait_for_page_events(
        runtime: &TerrainRuntimeHandle,
        count: usize,
    ) -> Vec<TerrainRuntimeEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events
            .iter()
            .filter(|event| matches!(event, TerrainRuntimeEvent::PageReady { .. }))
            .count()
            < count
        {
            runtime.pump(64);
            events.extend(runtime.drain_events(128));
            assert!(Instant::now() < deadline, "timed out waiting for pages");
            thread::yield_now();
        }
        events
    }

    #[test]
    fn transition_masks_cover_all_faces_and_signed_boundaries() {
        for face in TerrainTransitionFace::ALL {
            let axis = face.axis();
            let mut fine_xyz = [-3, -3, -3];
            fine_xyz[axis] = if face.is_positive() { -3 } else { -2 };
            let fine = PageKey::new(0, fine_xyz);
            let mut coarse_xyz = fine_xyz.map(|coordinate| coordinate.div_euclid(2));
            coarse_xyz[axis] += if face.is_positive() { 1 } else { -1 };
            let coarse = PageKey::new(1, coarse_xyz);
            let plan = TerrainStreamingPlan::for_test(
                PlanetId([1; 16]),
                vec![
                    PageDemand::for_test(fine, TerrainRequestClass::Visible),
                    PageDemand::for_test(coarse, TerrainRequestClass::Visible),
                ],
            );
            assert_eq!(plan.transition_masks().get(&fine), Some(&face.bit()));
            assert_eq!(plan.transition_masks().get(&coarse), Some(&0));
        }
        assert_eq!(
            TerrainTransitionFace::ALL
                .into_iter()
                .fold(0, |mask, face| mask | face.bit()),
            TERRAIN_TRANSITION_FACE_MASK
        );
    }

    #[test]
    fn mixed_lod_handoff_publishes_uploads_then_a_visible_set() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(3);
        runtime.upsert_planet(planet.clone()).unwrap();
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 2.0,
            prediction_seconds: 0.0,
            max_pages: 64,
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

        let mut session = TerrainResidencySession::new(
            planet.planet_id,
            TerrainResidencyConfig {
                max_active_pages: 64,
                max_transition_pages: 128,
                max_requests_per_reconcile: 64,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 128,
            max_commands_per_delta: 128,
            max_upload_bytes_per_delta: 64 * DENSE_PAGE_BYTES,
            max_tracked_pages: 64,
            max_visible_pages: 64,
        })
        .unwrap();
        assert_eq!(
            publisher.visible_set(&runtime, &session, &plan, 16),
            Err(TerrainRenderDeltaError::PlanNotCommitted)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        let mut tick = 0;
        while !session.has_committed_plan(&plan) {
            session.reconcile(&runtime, &plan, tick).unwrap();
            runtime.pump(64);
            events.extend(runtime.drain_events(128));
            tick += 1;
            assert!(Instant::now() < deadline, "timed out committing plan");
            thread::yield_now();
        }

        assert!(matches!(
            publisher.visible_set(&runtime, &session, &plan, 16),
            Err(TerrainRenderDeltaError::PageNotUploaded { .. })
        ));
        let delta = publisher.translate_events(&runtime, &events).unwrap();
        assert_eq!(delta.counters.upload_pages, plan.demands().len());
        assert_eq!(publisher.tracked_page_count(), plan.demands().len());
        let visible = publisher
            .visible_set(&runtime, &session, &plan, 17)
            .unwrap();
        assert_eq!(visible.frame_index, 17);
        assert_eq!(visible.pages.len(), plan.visible_count());
        assert!(visible
            .pages
            .iter()
            .all(|page| page.transition_mask & !TERRAIN_TRANSITION_FACE_MASK == 0));

        // Worker completion order may change upload command order, but the
        // committed renderer-visible state remains canonical.
        let mut reversed_events = events.clone();
        reversed_events.reverse();
        let mut reversed_publisher = TerrainRenderDeltaPublisher::new(publisher.config()).unwrap();
        reversed_publisher
            .translate_events(&runtime, &reversed_events)
            .unwrap();
        assert_eq!(
            reversed_publisher
                .visible_set(&runtime, &session, &plan, 17)
                .unwrap(),
            visible
        );

        let mut visible_limited = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_visible_pages: 1,
            ..publisher.config()
        })
        .unwrap();
        visible_limited.translate_events(&runtime, &events).unwrap();
        assert_eq!(
            visible_limited.visible_set(&runtime, &session, &plan, 18),
            Err(TerrainRenderDeltaError::VisiblePageBudget {
                actual: plan.visible_count(),
                maximum: 1,
            })
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn teleport_keeps_the_committed_visible_set_until_atomic_handoff() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(8);
        runtime.upsert_planet(planet.clone()).unwrap();
        let origin = TerrainStreamingPlan::for_test(
            planet.planet_id,
            vec![PageDemand::for_test(
                PageKey::new(0, [0, 0, 0]),
                TerrainRequestClass::Visible,
            )],
        );
        let destination = TerrainStreamingPlan::for_test(
            planet.planet_id,
            vec![PageDemand::for_test(
                PageKey::new(0, [16, 0, 0]),
                TerrainRequestClass::Visible,
            )],
        );
        let mut session = TerrainResidencySession::new(
            planet.planet_id,
            TerrainResidencyConfig {
                max_active_pages: 1,
                max_transition_pages: 2,
                max_requests_per_reconcile: 1,
                ..TerrainResidencyConfig::default()
            },
        )
        .unwrap();
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 2,
            max_visible_pages: 1,
        })
        .unwrap();

        let mut tick = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut origin_events = Vec::new();
        while !session.has_committed_plan(&origin) {
            session.reconcile(&runtime, &origin, tick).unwrap();
            runtime.pump(8);
            origin_events.extend(runtime.drain_events(8));
            tick += 1;
            assert!(Instant::now() < deadline, "origin handoff timed out");
            thread::yield_now();
        }
        publisher
            .translate_events(&runtime, &origin_events)
            .unwrap();
        let old_visible = publisher
            .visible_set(&runtime, &session, &origin, tick)
            .unwrap();

        session.reconcile(&runtime, &destination, tick).unwrap();
        assert_eq!(
            publisher
                .visible_set(&runtime, &session, &origin, tick + 1)
                .unwrap()
                .pages,
            old_visible.pages
        );
        assert_eq!(
            publisher.visible_set(&runtime, &session, &destination, tick + 1),
            Err(TerrainRenderDeltaError::PlanNotCommitted)
        );

        let mut destination_events = Vec::new();
        while !session.has_committed_plan(&destination) {
            runtime.pump(8);
            destination_events.extend(runtime.drain_events(8));
            session.reconcile(&runtime, &destination, tick).unwrap();
            tick += 1;
            assert!(Instant::now() < deadline, "destination handoff timed out");
            thread::yield_now();
        }
        destination_events.extend(runtime.drain_events(8));
        publisher
            .translate_events(&runtime, &destination_events)
            .unwrap();
        assert_eq!(
            publisher.visible_set(&runtime, &session, &origin, tick),
            Err(TerrainRenderDeltaError::PlanNotCommitted)
        );
        assert_eq!(
            publisher
                .visible_set(&runtime, &session, &destination, tick)
                .unwrap()
                .pages[0]
                .page_key,
            PageKey::new(0, [16, 0, 0])
        );
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn delayed_page_eviction_does_not_retire_a_newer_upload() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(7);
        runtime.upsert_planet(planet.clone()).unwrap();
        let key = PageKey::new(0, [-1, 0, 0]);
        runtime
            .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 1)
            .unwrap();
        let first = wait_for_page_events(&runtime, 1);
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        publisher.translate_events(&runtime, &first).unwrap();
        assert!(runtime.evict_page(planet.planet_id, key).unwrap());
        let eviction = runtime.drain_events(8);
        runtime
            .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 2)
            .unwrap();
        let second = wait_for_page_events(&runtime, 1);

        let reordered = second.into_iter().chain(eviction).collect::<Vec<_>>();
        let delta = publisher.translate_events(&runtime, &reordered).unwrap();
        assert!(matches!(
            delta.commands.as_slice(),
            [
                TerrainRenderCommand::Upload(TerrainPageUpload {
                    page_generation: 2,
                    ..
                }),
                TerrainRenderCommand::EvictPage(TerrainPageEvict {
                    page_generation: 1,
                    ..
                })
            ]
        ));
        assert_eq!(publisher.tracked_page_count(), 1);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn delayed_page_ready_cannot_upload_a_newer_generation() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(4);
        runtime.upsert_planet(planet.clone()).unwrap();
        let key = PageKey::new(0, [0; 3]);
        runtime
            .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 1)
            .unwrap();
        let first = wait_for_page_events(&runtime, 1);
        runtime
            .append_edit(
                planet.planet_id,
                EditOp {
                    sequence: 1,
                    stable_id: [9; 16],
                    shape: EditShape::Sphere {
                        center_cell: [4; 3],
                        radius_cells: 2,
                    },
                    mode: EditMode::Subtract,
                    material: 0,
                },
            )
            .unwrap();
        runtime
            .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 2)
            .unwrap();
        let second = wait_for_page_events(&runtime, 1);
        let events = first.into_iter().chain(second).collect::<Vec<_>>();
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        let delta = publisher.translate_events(&runtime, &events).unwrap();
        assert_eq!(delta.counters.stale_page_ready, 1);
        assert_eq!(delta.counters.upload_pages, 1);
        assert!(matches!(
            delta.commands.as_slice(),
            [TerrainRenderCommand::Upload(TerrainPageUpload {
                page_generation: 2,
                ..
            })]
        ));
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn planet_eviction_expands_the_exact_tracked_generation_set() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(5);
        runtime.upsert_planet(planet.clone()).unwrap();
        for (deadline, key) in [PageKey::new(0, [0; 3]), PageKey::new(2, [-1, 0, 0])]
            .into_iter()
            .enumerate()
        {
            runtime
                .request_page(
                    planet.planet_id,
                    key,
                    TerrainRequestClass::Visible,
                    deadline as u64,
                )
                .unwrap();
        }
        let ready = wait_for_page_events(&runtime, 2);
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        publisher.translate_events(&runtime, &ready).unwrap();
        assert_eq!(publisher.tracked_page_count(), 2);
        assert!(runtime.remove_planet(planet.planet_id).unwrap());
        let eviction = runtime.drain_events(8);
        let delta = publisher.translate_events(&runtime, &eviction).unwrap();
        assert!(matches!(
            delta.commands.as_slice(),
            [TerrainRenderCommand::EvictPlanet(TerrainPlanetEvict { pages, .. })]
                if pages.len() == 2 && pages.iter().all(|page| page.page_generation == 1)
        ));
        assert_eq!(publisher.tracked_page_count(), 0);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn budget_failure_is_transactional() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(6);
        runtime.upsert_planet(planet.clone()).unwrap();
        for key in [PageKey::new(0, [0; 3]), PageKey::new(0, [1, 0, 0])] {
            runtime
                .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 1)
                .unwrap();
        }
        let ready = wait_for_page_events(&runtime, 2);
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 1,
            max_visible_pages: 8,
        })
        .unwrap();
        assert_eq!(
            publisher.translate_events(&runtime, &ready),
            Err(TerrainRenderDeltaError::TrackedPageBudget {
                actual: 2,
                maximum: 1,
            })
        );
        assert_eq!(publisher.tracked_page_count(), 0);

        let mut command_limited = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 1,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        assert_eq!(
            command_limited.translate_events(&runtime, &ready),
            Err(TerrainRenderDeltaError::CommandBudget {
                actual: 2,
                maximum: 1,
            })
        );
        assert_eq!(command_limited.tracked_page_count(), 0);

        let mut upload_limited = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        assert_eq!(
            upload_limited.translate_events(&runtime, &ready),
            Err(TerrainRenderDeltaError::UploadByteBudget {
                actual: 2 * DENSE_PAGE_BYTES,
                maximum: DENSE_PAGE_BYTES,
            })
        );
        assert_eq!(upload_limited.tracked_page_count(), 0);
        subsystem.shutdown().unwrap();
    }
}
