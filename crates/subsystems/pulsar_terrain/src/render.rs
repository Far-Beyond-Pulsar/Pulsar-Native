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
    CellWord, PageKey, PlanetId, TerrainIncrementalResidencySession, TerrainRequestClass,
    TerrainResidentPageGeneration, TerrainRuntimeEvent, TerrainRuntimeHandle, CELL_COUNT,
};
use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TerrainRenderCommandId {
    Upload {
        planet_id: PlanetId,
        page_key: PageKey,
        planet_generation: u64,
        page_generation: u64,
    },
    EvictPage {
        planet_id: PlanetId,
        page_key: PageKey,
        planet_generation: u64,
        page_generation: u64,
    },
    EvictPlanet {
        planet_id: PlanetId,
        retired_planet_generation: u64,
    },
}

impl TerrainRenderCommand {
    pub const fn id(&self) -> TerrainRenderCommandId {
        match self {
            Self::Upload(upload) => TerrainRenderCommandId::Upload {
                planet_id: upload.planet_id,
                page_key: upload.page_key,
                planet_generation: upload.planet_generation,
                page_generation: upload.page_generation,
            },
            Self::EvictPage(eviction) => TerrainRenderCommandId::EvictPage {
                planet_id: eviction.planet_id,
                page_key: eviction.page_key,
                planet_generation: eviction.planet_generation,
                page_generation: eviction.page_generation,
            },
            Self::EvictPlanet(eviction) => TerrainRenderCommandId::EvictPlanet {
                planet_id: eviction.planet_id,
                retired_planet_generation: eviction.retired_planet_generation,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainRenderCommandDisposition {
    Applied,
    Deferred,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainRenderCommandFeedback {
    pub command: TerrainRenderCommandId,
    pub disposition: TerrainRenderCommandDisposition,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainRenderFeedback {
    pub commands: Vec<TerrainRenderCommandFeedback>,
    /// Renderer-local capacity eviction. These pages are no longer safe for a
    /// visible frontier even though their authoritative CPU pages may remain.
    pub cache_evictions: Vec<TerrainPageEvict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainVisiblePage {
    pub page_key: PageKey,
    pub planet_generation: u64,
    pub page_generation: u64,
    /// Faces owned by this coarse page that border render-relevant leaves
    /// exactly one LOD finer.
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
    pub pending_commands: usize,
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
    #[error("render publisher would track {actual} pages; configured maximum is {maximum}")]
    TrackedPageBudget { actual: usize, maximum: usize },
    #[error("visible set requires {actual} pages; configured maximum is {maximum}")]
    VisiblePageBudget { actual: usize, maximum: usize },
    #[error("renderer rejected terrain command {command:?}")]
    RendererRejected { command: TerrainRenderCommandId },
    #[error("renderer feedback references unknown terrain command {command:?}")]
    UnknownFeedback { command: TerrainRenderCommandId },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingPageCommand {
    Upload(TrackedGeneration),
    Evict(TrackedGeneration),
}

impl PendingPageCommand {
    const fn id(self, (planet_id, page_key): TrackedKey) -> TerrainRenderCommandId {
        let generation = match self {
            Self::Upload(generation) | Self::Evict(generation) => generation,
        };
        match self {
            Self::Upload(_) => TerrainRenderCommandId::Upload {
                planet_id,
                page_key,
                planet_generation: generation.planet_generation,
                page_generation: generation.page_generation,
            },
            Self::Evict(_) => TerrainRenderCommandId::EvictPage {
                planet_id,
                page_key,
                planet_generation: generation.planet_generation,
                page_generation: generation.page_generation,
            },
        }
    }
}

/// Stateful, bounded translator from explicit runtime events to immutable
/// renderer commands. Canonical terrain remains owned by `TerrainRuntimeHandle`.
pub struct TerrainRenderDeltaPublisher {
    config: TerrainRenderDeltaConfig,
    tracked: BTreeMap<TrackedKey, TrackedGeneration>,
    pending_pages: BTreeMap<TrackedKey, PendingPageCommand>,
    pending_planets: BTreeMap<PlanetId, TerrainPlanetEvict>,
}

impl TerrainRenderDeltaPublisher {
    pub fn new(config: TerrainRenderDeltaConfig) -> Result<Self, TerrainRenderDeltaError> {
        Ok(Self {
            config: config.validate()?,
            tracked: BTreeMap::new(),
            pending_pages: BTreeMap::new(),
            pending_planets: BTreeMap::new(),
        })
    }

    pub const fn config(&self) -> TerrainRenderDeltaConfig {
        self.config
    }

    pub fn tracked_page_count(&self) -> usize {
        self.tracked.len()
    }

    pub fn pending_command_count(&self) -> usize {
        self.pending_pages.len() + self.pending_planets.len()
    }

    pub(crate) fn ensure_resident_upload(
        &mut self,
        planet_id: PlanetId,
        page_key: PageKey,
        generation: TerrainResidentPageGeneration,
    ) -> Result<bool, TerrainRenderDeltaError> {
        let key = (planet_id, page_key);
        let generation = TrackedGeneration {
            planet_generation: generation.planet_generation,
            page_generation: generation.page_generation,
        };
        if self.tracked.get(&key) == Some(&generation)
            || self.pending_pages.get(&key) == Some(&PendingPageCommand::Upload(generation))
        {
            return Ok(false);
        }
        let previous = self
            .pending_pages
            .insert(key, PendingPageCommand::Upload(generation));
        if let Err(error) = validate_tracked_page_budget(
            &self.tracked,
            &self.pending_pages,
            self.config.max_tracked_pages,
        ) {
            if let Some(previous) = previous {
                self.pending_pages.insert(key, previous);
            } else {
                self.pending_pages.remove(&key);
            }
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn published_resident_pages(
        &self,
        planet_id: PlanetId,
        resident: &BTreeMap<PageKey, TerrainResidentPageGeneration>,
    ) -> BTreeSet<PageKey> {
        self.tracked
            .iter()
            .filter_map(|((tracked_planet, page_key), tracked)| {
                (*tracked_planet == planet_id
                    && resident.get(page_key)
                        == Some(&TerrainResidentPageGeneration {
                            planet_generation: tracked.planet_generation,
                            page_generation: tracked.page_generation,
                        }))
                .then_some(*page_key)
            })
            .collect()
    }

    /// Ingest one caller-owned event slice and emit a bounded immutable retry
    /// batch. Commands remain pending until explicit renderer feedback marks
    /// them applied; backpressure therefore cannot advance visibility.
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

        let mut counters = TerrainRenderDeltaCounters {
            input_events: events.len(),
            ..TerrainRenderDeltaCounters::default()
        };
        let mut pending_pages = self.pending_pages.clone();
        let mut pending_planets = self.pending_planets.clone();

        // A whole-planet retirement is valid only while that retired
        // generation is still the newest canonical identity. If the same id
        // has already advanced, retain the old page retirements but never let
        // delayed feedback remove the newer planet frame.
        let superseded_planet_retirements = pending_planets
            .iter()
            .filter_map(|(planet_id, eviction)| {
                runtime
                    .planet_generation(*planet_id)
                    .is_some_and(|generation| generation > eviction.retired_planet_generation)
                    .then_some((*planet_id, eviction.clone()))
            })
            .collect::<Vec<_>>();
        for (planet_id, eviction) in superseded_planet_retirements {
            pending_planets.remove(&planet_id);
            for page in eviction.pages {
                stage_page_eviction(&self.tracked, &mut pending_pages, page);
            }
        }

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
                    if runtime.resident_page_generation(*planet_id, *page_key) != Some(expected) {
                        counters.stale_page_ready = counters.stale_page_ready.saturating_add(1);
                        continue;
                    }
                    let key = (*planet_id, *page_key);
                    let generation = TrackedGeneration {
                        planet_generation: *planet_generation,
                        page_generation: *page_generation,
                    };
                    if self.tracked.get(&key) != Some(&generation) {
                        pending_pages.insert(key, PendingPageCommand::Upload(generation));
                    }
                }
                TerrainRuntimeEvent::EvictPage {
                    planet_id,
                    page_key,
                    planet_generation,
                    retired_page_generation,
                } => {
                    let covered_by_planet_retirement =
                        pending_planets.get(planet_id).is_some_and(|retirement| {
                            retirement.retired_planet_generation >= *planet_generation
                        });
                    if !covered_by_planet_retirement {
                        stage_page_eviction(
                            &self.tracked,
                            &mut pending_pages,
                            TerrainPageEvict {
                                planet_id: *planet_id,
                                page_key: *page_key,
                                planet_generation: *planet_generation,
                                page_generation: *retired_page_generation,
                            },
                        );
                    }
                }
                TerrainRuntimeEvent::EvictPlanet {
                    planet_id,
                    retired_generation,
                } => {
                    let pages = effective_planet_pages(
                        &self.tracked,
                        &pending_pages,
                        *planet_id,
                        *retired_generation,
                    );
                    if runtime
                        .planet_generation(*planet_id)
                        .is_some_and(|generation| generation > *retired_generation)
                    {
                        pending_planets.remove(planet_id);
                        for page in pages {
                            stage_page_eviction(&self.tracked, &mut pending_pages, page);
                        }
                    } else {
                        pending_pages.retain(|(pending_planet, _), _| pending_planet != planet_id);
                        pending_planets.insert(
                            *planet_id,
                            TerrainPlanetEvict {
                                planet_id: *planet_id,
                                retired_planet_generation: *retired_generation,
                                pages,
                            },
                        );
                    }
                }
                TerrainRuntimeEvent::StaleRejected { .. }
                | TerrainRuntimeEvent::Backpressure { .. }
                | TerrainRuntimeEvent::Error { .. } => {
                    counters.ignored_events = counters.ignored_events.saturating_add(1);
                }
            }
        }

        validate_tracked_page_budget(&self.tracked, &pending_pages, self.config.max_tracked_pages)?;
        self.pending_pages = pending_pages;
        self.pending_planets = pending_planets;

        let mut commands = Vec::new();
        for eviction in self.pending_planets.values() {
            if commands.len() == self.config.max_commands_per_delta {
                break;
            }
            counters.page_evictions = counters.page_evictions.saturating_add(eviction.pages.len());
            counters.planet_evictions = counters.planet_evictions.saturating_add(1);
            commands.push(TerrainRenderCommand::EvictPlanet(eviction.clone()));
        }
        let mut stale_pending = Vec::new();
        for (key, pending) in &self.pending_pages {
            if commands.len() == self.config.max_commands_per_delta {
                break;
            }
            match pending {
                PendingPageCommand::Upload(generation) => {
                    if counters.upload_bytes.saturating_add(PAGE_UPLOAD_BYTES)
                        > self.config.max_upload_bytes_per_delta
                    {
                        break;
                    }
                    let expected = TerrainResidentPageGeneration {
                        planet_generation: generation.planet_generation,
                        page_generation: generation.page_generation,
                    };
                    let Some(page) = runtime.page_snapshot_for_generation(key.0, key.1, expected)
                    else {
                        stale_pending.push(*key);
                        counters.stale_page_ready = counters.stale_page_ready.saturating_add(1);
                        continue;
                    };
                    commands.push(TerrainRenderCommand::Upload(TerrainPageUpload {
                        planet_id: key.0,
                        page_key: key.1,
                        planet_generation: generation.planet_generation,
                        page_generation: generation.page_generation,
                        cells: page.cells().collect::<Vec<_>>().into_boxed_slice(),
                    }));
                    counters.upload_pages = counters.upload_pages.saturating_add(1);
                    counters.upload_bytes = counters.upload_bytes.saturating_add(PAGE_UPLOAD_BYTES);
                }
                PendingPageCommand::Evict(generation) => {
                    commands.push(TerrainRenderCommand::EvictPage(TerrainPageEvict {
                        planet_id: key.0,
                        page_key: key.1,
                        planet_generation: generation.planet_generation,
                        page_generation: generation.page_generation,
                    }));
                    counters.page_evictions = counters.page_evictions.saturating_add(1);
                }
            }
        }
        for key in stale_pending {
            self.pending_pages.remove(&key);
        }
        counters.commands = commands.len();
        counters.tracked_pages = self.tracked.len();
        counters.pending_commands = self.pending_command_count();
        Ok(TerrainRenderDelta { commands, counters })
    }

    /// Advance acknowledged GPU residency. Deferred commands stay pending and
    /// are emitted again by the next [`Self::translate_events`] call.
    pub fn acknowledge_render_feedback(
        &mut self,
        feedback: &TerrainRenderFeedback,
    ) -> Result<(), TerrainRenderDeltaError> {
        for item in &feedback.commands {
            if item.disposition == TerrainRenderCommandDisposition::Rejected {
                return Err(TerrainRenderDeltaError::RendererRejected {
                    command: item.command,
                });
            }
            if !self.is_pending(item.command) {
                return Err(TerrainRenderDeltaError::UnknownFeedback {
                    command: item.command,
                });
            }
        }

        for item in &feedback.commands {
            if item.disposition == TerrainRenderCommandDisposition::Deferred {
                continue;
            }
            match item.command {
                TerrainRenderCommandId::Upload {
                    planet_id,
                    page_key,
                    planet_generation,
                    page_generation,
                } => {
                    self.pending_pages.remove(&(planet_id, page_key));
                    self.tracked.insert(
                        (planet_id, page_key),
                        TrackedGeneration {
                            planet_generation,
                            page_generation,
                        },
                    );
                }
                TerrainRenderCommandId::EvictPage {
                    planet_id,
                    page_key,
                    planet_generation,
                    page_generation,
                } => {
                    self.pending_pages.remove(&(planet_id, page_key));
                    remove_tracked_through(
                        &mut self.tracked,
                        (planet_id, page_key),
                        TrackedGeneration {
                            planet_generation,
                            page_generation,
                        },
                    );
                }
                TerrainRenderCommandId::EvictPlanet {
                    planet_id,
                    retired_planet_generation,
                } => {
                    self.pending_planets.remove(&planet_id);
                    self.tracked.retain(|(tracked_planet, _), generation| {
                        *tracked_planet != planet_id
                            || generation.planet_generation > retired_planet_generation
                    });
                }
            }
        }
        for eviction in &feedback.cache_evictions {
            remove_tracked_through(
                &mut self.tracked,
                (eviction.planet_id, eviction.page_key),
                TrackedGeneration {
                    planet_generation: eviction.planet_generation,
                    page_generation: eviction.page_generation,
                },
            );
        }
        Ok(())
    }

    fn is_pending(&self, command: TerrainRenderCommandId) -> bool {
        match command {
            TerrainRenderCommandId::Upload {
                planet_id,
                page_key,
                ..
            }
            | TerrainRenderCommandId::EvictPage {
                planet_id,
                page_key,
                ..
            } => self
                .pending_pages
                .get(&(planet_id, page_key))
                .is_some_and(|pending| pending.id((planet_id, page_key)) == command),
            TerrainRenderCommandId::EvictPlanet { planet_id, .. } => {
                self.pending_planets.get(&planet_id).is_some_and(|pending| {
                    TerrainRenderCommand::EvictPlanet(pending.clone()).id() == command
                })
            }
        }
    }

    /// Publish the currently committed parent-preserving frontier. Every
    /// intermediate set is complete, non-overlapping, and 2:1 balanced; the
    /// final target plan does not need to be resident yet.
    pub fn visible_set(
        &self,
        runtime: &TerrainRuntimeHandle,
        session: &TerrainIncrementalResidencySession,
        frame_index: u64,
    ) -> Result<TerrainVisiblePageSet, TerrainRenderDeltaError> {
        let committed = session.committed_demands().collect::<Vec<_>>();
        let masks =
            crate::refinement::transition_masks(committed.iter().map(|(page_key, _)| *page_key));
        self.visible_set_for_pages(
            runtime,
            session.planet_id(),
            committed
                .into_iter()
                .filter_map(|(page_key, request_class)| {
                    (request_class == TerrainRequestClass::Visible).then_some(page_key)
                }),
            &masks,
            frame_index,
        )
    }

    fn visible_set_for_pages(
        &self,
        runtime: &TerrainRuntimeHandle,
        planet_id: PlanetId,
        visible_pages: impl IntoIterator<Item = PageKey>,
        masks: &BTreeMap<PageKey, u8>,
        frame_index: u64,
    ) -> Result<TerrainVisiblePageSet, TerrainRenderDeltaError> {
        let visible_pages = visible_pages.into_iter().collect::<Vec<_>>();
        let visible_count = visible_pages.len();
        if visible_count > self.config.max_visible_pages {
            return Err(TerrainRenderDeltaError::VisiblePageBudget {
                actual: visible_count,
                maximum: self.config.max_visible_pages,
            });
        }
        let mut pages = Vec::with_capacity(visible_count);
        for page_key in visible_pages {
            let generation = runtime
                .resident_page_generation(planet_id, page_key)
                .ok_or(TerrainRenderDeltaError::PageNotResident {
                    planet_id,
                    page_key,
                })?;
            let tracked = self.tracked.get(&(planet_id, page_key));
            if tracked
                != Some(&TrackedGeneration {
                    planet_generation: generation.planet_generation,
                    page_generation: generation.page_generation,
                })
            {
                return Err(TerrainRenderDeltaError::PageNotUploaded {
                    planet_id,
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
            planet_id,
            frame_index,
            pages,
        })
    }
}

fn effective_planet_pages(
    tracked: &BTreeMap<TrackedKey, TrackedGeneration>,
    pending: &BTreeMap<TrackedKey, PendingPageCommand>,
    planet_id: PlanetId,
    retired_planet_generation: u64,
) -> Vec<TerrainPageEvict> {
    let mut pages = BTreeMap::<PageKey, TrackedGeneration>::new();
    for ((planet, page), generation) in tracked {
        if *planet == planet_id && generation.planet_generation <= retired_planet_generation {
            pages.insert(*page, *generation);
        }
    }
    for ((planet, page), command) in pending {
        if *planet != planet_id {
            continue;
        }
        let generation = match command {
            PendingPageCommand::Upload(generation) | PendingPageCommand::Evict(generation) => {
                *generation
            }
        };
        if generation.planet_generation <= retired_planet_generation {
            pages.insert(*page, generation);
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

fn stage_page_eviction(
    tracked: &BTreeMap<TrackedKey, TrackedGeneration>,
    pending: &mut BTreeMap<TrackedKey, PendingPageCommand>,
    eviction: TerrainPageEvict,
) {
    let key = (eviction.planet_id, eviction.page_key);
    let retired = TrackedGeneration {
        planet_generation: eviction.planet_generation,
        page_generation: eviction.page_generation,
    };
    let newer_pending = pending.get(&key).is_some_and(|command| {
        let generation = match command {
            PendingPageCommand::Upload(generation) | PendingPageCommand::Evict(generation) => {
                *generation
            }
        };
        generation_is_newer(generation, retired)
    });
    let newer_tracked = tracked
        .get(&key)
        .is_some_and(|generation| generation_is_newer(*generation, retired));
    if !newer_pending && !newer_tracked {
        pending.insert(key, PendingPageCommand::Evict(retired));
    }
}

fn validate_tracked_page_budget(
    tracked: &BTreeMap<TrackedKey, TrackedGeneration>,
    pending: &BTreeMap<TrackedKey, PendingPageCommand>,
    maximum: usize,
) -> Result<(), TerrainRenderDeltaError> {
    let mut possible_residents = tracked.keys().copied().collect::<BTreeSet<_>>();
    possible_residents.extend(pending.iter().filter_map(|(key, command)| {
        matches!(command, PendingPageCommand::Upload(_)).then_some(*key)
    }));
    if possible_residents.len() > maximum {
        return Err(TerrainRenderDeltaError::TrackedPageBudget {
            actual: possible_residents.len(),
            maximum,
        });
    }
    Ok(())
}

fn remove_tracked_through(
    tracked: &mut BTreeMap<TrackedKey, TrackedGeneration>,
    key: TrackedKey,
    retired: TrackedGeneration,
) {
    let should_remove = tracked.get(&key).is_some_and(|current| {
        current.planet_generation < retired.planet_generation
            || (current.planet_generation == retired.planet_generation
                && current.page_generation <= retired.page_generation)
    });
    if should_remove {
        tracked.remove(&key);
    }
}

const fn generation_is_newer(candidate: TrackedGeneration, reference: TrackedGeneration) -> bool {
    candidate.planet_generation > reference.planet_generation
        || (candidate.planet_generation == reference.planet_generation
            && candidate.page_generation > reference.page_generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EditMode, EditOp, EditShape, PageDemand, PlanetDefinition, TerrainRefinementConfig,
        TerrainRuntimeConfig, TerrainStreamingPlan, TerrainSubsystem,
    };
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::time::{Duration, Instant};
    use std::{collections::BTreeSet, thread};

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

    fn acknowledge_all(publisher: &mut TerrainRenderDeltaPublisher, delta: &TerrainRenderDelta) {
        publisher
            .acknowledge_render_feedback(&TerrainRenderFeedback {
                commands: delta
                    .commands
                    .iter()
                    .map(|command| TerrainRenderCommandFeedback {
                        command: command.id(),
                        disposition: TerrainRenderCommandDisposition::Applied,
                    })
                    .collect(),
                cache_evictions: Vec::new(),
            })
            .unwrap();
    }

    fn translate_and_ack(
        publisher: &mut TerrainRenderDeltaPublisher,
        runtime: &TerrainRuntimeHandle,
        events: &[TerrainRuntimeEvent],
    ) -> TerrainRenderDelta {
        let delta = publisher.translate_events(runtime, events).unwrap();
        acknowledge_all(publisher, &delta);
        delta
    }

    #[test]
    fn transition_masks_cover_all_faces_and_signed_boundaries() {
        for coarse_face in TerrainTransitionFace::ALL {
            let axis = coarse_face.axis();
            let coarse = PageKey::new(1, [-3, -3, -3]);
            let tangential = match axis {
                0 => [1, 2],
                1 => [0, 2],
                2 => [0, 1],
                _ => unreachable!(),
            };
            for quadrant in 0..4 {
                let mut fine_xyz = coarse.page_xyz.map(|coordinate| coordinate * 2);
                fine_xyz[axis] += if coarse_face.is_positive() { 2 } else { -1 };
                fine_xyz[tangential[0]] += (quadrant & 1) as i64;
                fine_xyz[tangential[1]] += ((quadrant >> 1) & 1) as i64;
                let fine = PageKey::new(0, fine_xyz);
                let plan = TerrainStreamingPlan::for_test(
                    PlanetId([1; 16]),
                    vec![
                        PageDemand::for_test(fine, TerrainRequestClass::Visible),
                        PageDemand::for_test(coarse, TerrainRequestClass::Visible),
                    ],
                );
                assert_eq!(plan.transition_masks().get(&fine), Some(&0));
                assert_eq!(
                    plan.transition_masks().get(&coarse),
                    Some(&coarse_face.bit())
                );
            }
        }
        assert_eq!(
            TerrainTransitionFace::ALL
                .into_iter()
                .fold(0, |mask, face| mask | face.bit()),
            TERRAIN_TRANSITION_FACE_MASK
        );
    }

    #[test]
    fn incremental_handoff_keeps_the_coarse_page_visible_until_children_commit() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(9);
        runtime.upsert_planet(planet.clone()).unwrap();
        let parent = PageKey::new(2, [0, 0, 0]);
        let base = parent.page_xyz.map(|coordinate| coordinate * 2);
        let children = (0..8)
            .map(|index| {
                PageKey::new(
                    parent.lod - 1,
                    [
                        base[0] + (index & 1),
                        base[1] + ((index >> 1) & 1),
                        base[2] + ((index >> 2) & 1),
                    ],
                )
            })
            .collect::<BTreeSet<_>>();
        let plan = TerrainStreamingPlan::for_test(
            planet.planet_id,
            children
                .iter()
                .copied()
                .map(|key| PageDemand::for_test(key, TerrainRequestClass::Visible))
                .collect(),
        );
        let mut session = TerrainIncrementalResidencySession::new(
            planet.planet_id,
            TerrainRefinementConfig {
                max_active_pages: 8,
                max_transition_pages: 9,
                initial_coarse_pages: 1,
                max_requests_per_reconcile: 1,
                max_commits_per_reconcile: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 16,
            max_commands_per_delta: 16,
            max_upload_bytes_per_delta: 16 * DENSE_PAGE_BYTES,
            max_tracked_pages: 16,
            max_visible_pages: 8,
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut tick = 0;
        let mut saw_parent = false;
        let mut saw_parent_while_children_staged = false;
        let mut deferred_child_once = false;
        session
            .reconcile(&runtime, &mut publisher, &plan, tick)
            .unwrap();
        loop {
            runtime.pump(8);
            let events = runtime.drain_events(16);
            let delta = publisher.translate_events(&runtime, &events).unwrap();
            let child_upload = delta.commands.iter().find_map(|command| match command {
                TerrainRenderCommand::Upload(upload) if children.contains(&upload.page_key) => {
                    Some(command.id())
                }
                _ => None,
            });
            let deferred_command = (!deferred_child_once).then_some(child_upload).flatten();
            if let Some(command) = deferred_command {
                publisher
                    .acknowledge_render_feedback(&TerrainRenderFeedback {
                        commands: vec![TerrainRenderCommandFeedback {
                            command,
                            disposition: TerrainRenderCommandDisposition::Deferred,
                        }],
                        cache_evictions: Vec::new(),
                    })
                    .unwrap();
                session
                    .reconcile(&runtime, &mut publisher, &plan, tick)
                    .unwrap();
                assert_eq!(
                    session.committed_pages().collect::<BTreeSet<_>>(),
                    BTreeSet::from([parent]),
                    "renderer backpressure must not retire the coarse parent"
                );
                let retry = publisher.translate_events(&runtime, &[]).unwrap();
                assert!(retry.commands.iter().any(|retry| retry.id() == command));
                acknowledge_all(&mut publisher, &retry);
                deferred_child_once = true;
            } else {
                acknowledge_all(&mut publisher, &delta);
            }
            session
                .reconcile(&runtime, &mut publisher, &plan, tick)
                .unwrap();

            let committed = session.committed_pages().collect::<BTreeSet<_>>();
            if !committed.is_empty() {
                let visible = publisher.visible_set(&runtime, &session, tick).unwrap();
                let visible_keys = visible
                    .pages
                    .iter()
                    .map(|page| page.page_key)
                    .collect::<BTreeSet<_>>();
                assert_eq!(visible_keys, committed);
                if committed == BTreeSet::from([parent]) {
                    saw_parent = true;
                    saw_parent_while_children_staged |= session.staged_pages().count() == 8;
                }
            }
            if session.is_converged() {
                break;
            }
            tick += 1;
            assert!(Instant::now() < deadline, "incremental handoff timed out");
            thread::yield_now();
        }

        assert!(saw_parent);
        assert!(saw_parent_while_children_staged);
        assert!(deferred_child_once);
        assert_eq!(session.committed_pages().collect::<BTreeSet<_>>(), children);
        let final_visible = publisher.visible_set(&runtime, &session, tick + 1).unwrap();
        assert_eq!(final_visible.pages.len(), 8);
        assert!(final_visible
            .pages
            .iter()
            .all(|page| page.page_key != parent));
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
        translate_and_ack(&mut publisher, &runtime, &first);
        assert!(runtime.evict_page(planet.planet_id, key).unwrap());
        let eviction = runtime.drain_events(8);
        runtime
            .request_page(planet.planet_id, key, TerrainRequestClass::Visible, 2)
            .unwrap();
        let second = wait_for_page_events(&runtime, 1);

        let reordered = second.into_iter().chain(eviction).collect::<Vec<_>>();
        let delta = translate_and_ack(&mut publisher, &runtime, &reordered);
        assert!(matches!(
            delta.commands.as_slice(),
            [TerrainRenderCommand::Upload(TerrainPageUpload {
                page_generation: 2,
                ..
            })]
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
        let delta = translate_and_ack(&mut publisher, &runtime, &events);
        assert_eq!(delta.counters.stale_page_ready, 1);
        assert_eq!(delta.counters.upload_pages, 1);
        assert_eq!(delta.counters.page_evictions, 0);
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
        translate_and_ack(&mut publisher, &runtime, &ready);
        assert_eq!(publisher.tracked_page_count(), 2);
        assert!(runtime.remove_planet(planet.planet_id).unwrap());
        let eviction = runtime.drain_events(8);
        let delta = publisher.translate_events(&runtime, &eviction).unwrap();
        assert!(matches!(
            delta.commands.as_slice(),
            [TerrainRenderCommand::EvictPlanet(TerrainPlanetEvict { pages, .. })]
                if pages.len() == 2 && pages.iter().all(|page| page.page_generation == 1)
        ));
        assert_eq!(publisher.tracked_page_count(), 2);
        acknowledge_all(&mut publisher, &delta);
        assert_eq!(publisher.tracked_page_count(), 0);
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn renderer_cache_eviction_revokes_publication_and_requeues_the_resident_page() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(8);
        runtime.upsert_planet(planet.clone()).unwrap();
        let page_key = PageKey::new(0, [1, -1, 0]);
        runtime
            .request_page(planet.planet_id, page_key, TerrainRequestClass::Visible, 1)
            .unwrap();
        let ready = wait_for_page_events(&runtime, 1);
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        let uploaded = translate_and_ack(&mut publisher, &runtime, &ready);
        let eviction = match uploaded.commands.as_slice() {
            [TerrainRenderCommand::Upload(upload)] => TerrainPageEvict {
                planet_id: upload.planet_id,
                page_key: upload.page_key,
                planet_generation: upload.planet_generation,
                page_generation: upload.page_generation,
            },
            commands => panic!("expected one upload, got {commands:?}"),
        };
        assert_eq!(publisher.tracked_page_count(), 1);

        publisher
            .acknowledge_render_feedback(&TerrainRenderFeedback {
                commands: Vec::new(),
                cache_evictions: vec![eviction],
            })
            .unwrap();
        assert_eq!(publisher.tracked_page_count(), 0);
        let generation = runtime
            .resident_page_generation(planet.planet_id, page_key)
            .unwrap();
        assert!(publisher
            .ensure_resident_upload(planet.planet_id, page_key, generation)
            .unwrap());
        let retry = publisher.translate_events(&runtime, &[]).unwrap();
        assert_eq!(retry.commands.len(), 1);
        assert_eq!(retry.commands[0].id(), uploaded.commands[0].id());
        subsystem.shutdown().unwrap();
    }

    #[test]
    fn delayed_planet_retirement_cannot_remove_a_recreated_planet_frame() {
        let mut subsystem = start();
        let runtime = subsystem.runtime_handle();
        let planet = definition(3);
        let first_planet_generation = runtime.upsert_planet(planet.clone()).unwrap();
        let page_key = PageKey::new(0, [0; 3]);
        runtime
            .request_page(planet.planet_id, page_key, TerrainRequestClass::Visible, 1)
            .unwrap();
        let ready = wait_for_page_events(&runtime, 1);
        let mut publisher = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        translate_and_ack(&mut publisher, &runtime, &ready);
        assert!(runtime.remove_planet(planet.planet_id).unwrap());
        let delayed_retirement = runtime.drain_events(8);
        let recreated_generation = runtime.upsert_planet(planet.clone()).unwrap();
        assert!(recreated_generation > first_planet_generation);

        let delta = publisher
            .translate_events(&runtime, &delayed_retirement)
            .unwrap();
        assert!(matches!(
            delta.commands.as_slice(),
            [TerrainRenderCommand::EvictPage(TerrainPageEvict {
                planet_generation,
                ..
            })] if *planet_generation == first_planet_generation
        ));
        assert!(!delta
            .commands
            .iter()
            .any(|command| matches!(command, TerrainRenderCommand::EvictPlanet(_))));
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
        assert_eq!(publisher.pending_command_count(), 0);

        let mut command_limited = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 1,
            max_upload_bytes_per_delta: 2 * DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        let first = command_limited.translate_events(&runtime, &ready).unwrap();
        assert_eq!(first.commands.len(), 1);
        assert_eq!(command_limited.tracked_page_count(), 0);
        assert_eq!(command_limited.pending_command_count(), 2);
        acknowledge_all(&mut command_limited, &first);
        let second = command_limited.translate_events(&runtime, &[]).unwrap();
        assert_eq!(second.commands.len(), 1);

        let mut upload_limited = TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig {
            max_events_per_delta: 8,
            max_commands_per_delta: 8,
            max_upload_bytes_per_delta: DENSE_PAGE_BYTES,
            max_tracked_pages: 8,
            max_visible_pages: 8,
        })
        .unwrap();
        let first = upload_limited.translate_events(&runtime, &ready).unwrap();
        assert_eq!(first.commands.len(), 1);
        assert_eq!(first.counters.upload_bytes, DENSE_PAGE_BYTES);
        assert_eq!(upload_limited.tracked_page_count(), 0);
        assert_eq!(upload_limited.pending_command_count(), 2);
        subsystem.shutdown().unwrap();
    }
}
