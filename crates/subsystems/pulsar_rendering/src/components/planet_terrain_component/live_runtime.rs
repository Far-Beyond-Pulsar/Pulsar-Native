use std::collections::{BTreeMap, BTreeSet};

use engine_subsystems::{Subsystem, SubsystemContext, SubsystemError};
use helio_pass_planetary_voxel::PlanetaryVoxelRenderConfig;
use helio_planet_voxel_core::VisibilityOutcome;
use pulsar_reflection::LiveKeySet;
use pulsar_terrain::{
    CELL_COUNT, PlanetId, PlanetPosition, PlanetView, PositionError, TerrainControllerConfig,
    TerrainControllerError, TerrainPlanningConfig, TerrainRefinementConfig,
    TerrainRenderDeltaConfig, TerrainRuntimeConfig, TerrainRuntimeError, TerrainRuntimeHandle,
    TerrainStreamingConfig, TerrainStreamingController, TerrainStreamingError, TerrainSubsystem,
};
use thiserror::Error;

use super::{
    PlanetTerrainComponentRenderAdapter, PlanetaryTerrainRenderError, TerrainRenderApplyReport,
};

const LIVE_MAX_PLANETS: usize = 4;
const LIVE_ACTIVE_PAGES_PER_PLANET: usize = 96;
const LIVE_TRANSITION_PAGES_PER_PLANET: usize = 96;
const LIVE_GPU_VISIBLE_PAGES: usize = 384;

/// Component identities retained between SceneDB revisions. The canonical
/// terrain runtime remains the source of truth; this cache only lets the
/// component sync remove sources that disappeared from the scene.
#[derive(Debug, Default)]
pub struct PlanetTerrainComponentCache {
    sources: BTreeMap<String, PlanetId>,
}

impl PlanetTerrainComponentCache {
    pub fn record(&mut self, source_key: String, planet_id: PlanetId) {
        self.sources.insert(source_key, planet_id);
    }

    pub fn remove(&mut self, source_key: &str) {
        self.sources.remove(source_key);
    }

    pub fn active_planets(&self) -> BTreeSet<PlanetId> {
        self.sources.values().copied().collect()
    }

    pub fn remove_stale(
        &mut self,
        runtime: &TerrainRuntimeHandle,
        live_keys: &LiveKeySet,
    ) -> Result<usize, TerrainRuntimeError> {
        let stale = self
            .sources
            .keys()
            .filter(|key| !live_keys.contains(key))
            .cloned()
            .collect::<Vec<_>>();
        for source_key in &stale {
            runtime.remove_component(source_key)?;
            self.sources.remove(source_key);
        }
        Ok(stale.len())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetTerrainFrameInput {
    pub camera_m: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    pub vertical_fov_radians: f64,
    pub viewport_px: [u32; 2],
    pub near_m: f64,
    pub far_m: f64,
    pub velocity_mps: [f64; 3],
    pub delta_time_s: f32,
    pub tick: u64,
    pub frame_index: u64,
    pub graph_rebuilt: bool,
}

#[derive(Debug)]
pub struct PlanetTerrainAdvanceReport {
    pub plans_applied: usize,
    pub planning_failures: Vec<String>,
    pub render: TerrainRenderApplyReport,
    pub visibility: VisibilityOutcome,
}

#[derive(Debug, Error)]
pub enum PlanetTerrainLiveError {
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
    #[error(transparent)]
    Controller(#[from] TerrainControllerError),
    #[error(transparent)]
    Position(#[from] PositionError),
    #[error(transparent)]
    Streaming(#[from] TerrainStreamingError),
    #[error(transparent)]
    Renderer(#[from] PlanetaryTerrainRenderError),
    #[error("planet terrain subsystem failed: {0}")]
    Subsystem(String),
}

/// Live owner joining scene components, Pulsar's canonical terrain runtime,
/// the incremental controller, and Helio's graph-owned disposable cache.
pub struct PlanetTerrainRuntime {
    subsystem: TerrainSubsystem,
    runtime: TerrainRuntimeHandle,
    controller: TerrainStreamingController,
    adapter: PlanetTerrainComponentRenderAdapter,
    component_cache: PlanetTerrainComponentCache,
}

impl PlanetTerrainRuntime {
    pub fn new() -> Result<Self, PlanetTerrainLiveError> {
        let mut subsystem = TerrainSubsystem::new(live_runtime_config())?;
        subsystem
            .init(&SubsystemContext::new())
            .map_err(|error| PlanetTerrainLiveError::Subsystem(error.to_string()))?;
        let runtime = subsystem.runtime_handle();
        let controller = TerrainStreamingController::new(
            runtime.clone(),
            subsystem.planning_handle(),
            live_controller_config(),
        )?;
        Ok(Self {
            subsystem,
            runtime,
            controller,
            adapter: PlanetTerrainComponentRenderAdapter::new(),
            component_cache: PlanetTerrainComponentCache::default(),
        })
    }

    pub fn renderer_config() -> PlanetaryVoxelRenderConfig {
        PlanetaryVoxelRenderConfig::horizon_demo()
    }

    pub fn component_context_mut(
        &mut self,
    ) -> (&mut TerrainRuntimeHandle, &mut PlanetTerrainComponentCache) {
        (&mut self.runtime, &mut self.component_cache)
    }

    pub fn has_active_components(&self) -> bool {
        !self.component_cache.sources.is_empty()
    }

    pub fn renderer_ready(&self, renderer: &helio::Renderer) -> bool {
        self.adapter.residency(renderer).is_ok()
    }

    pub fn remove_stale_components(
        &mut self,
        live_keys: &LiveKeySet,
    ) -> Result<usize, TerrainRuntimeError> {
        self.component_cache.remove_stale(&self.runtime, live_keys)
    }

    pub fn advance(
        &mut self,
        renderer: &mut helio::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input: PlanetTerrainFrameInput,
    ) -> Result<PlanetTerrainAdvanceReport, PlanetTerrainLiveError> {
        let renderer_lost_published_cache = self.controller.published_page_count() > 0
            && self
                .adapter
                .residency(renderer)?
                .cache()
                .counters()
                .resident_pages
                == 0;
        if input.graph_rebuilt || renderer_lost_published_cache {
            self.controller.invalidate_renderer_cache()?;
        }

        self.subsystem.on_frame(input.delta_time_s);
        let camera = PlanetPosition::from_meters(input.camera_m)?;
        let view = PlanetView::new(
            camera,
            input.forward,
            input.up,
            input.vertical_fov_radians,
            input.viewport_px,
            input.near_m,
            input.far_m,
            input.velocity_mps,
        )?;
        for planet_id in self.component_cache.active_planets() {
            self.controller.submit_view(planet_id, view)?;
        }

        let events = self
            .runtime
            .drain_events(self.controller.config().rendering.max_events_per_delta);
        let frame = self
            .controller
            .process_frame(&events, input.tick, input.frame_index)?;
        let planning_failures = frame
            .planning_failures
            .iter()
            .map(|failure| {
                format!(
                    "planet {:?} planning failed: {}",
                    failure.ticket.planet_id, failure.message
                )
            })
            .collect::<Vec<_>>();

        for planet_frame in frame.planet_frames {
            self.adapter
                .set_planet_frame(renderer, queue, planet_frame)?;
        }
        let render = self
            .adapter
            .apply_delta(renderer, device, queue, frame.render_delta)?;
        let visibility = self.adapter.apply_visible_sets(
            renderer,
            queue,
            frame.frame_index,
            frame.visible_sets,
        )?;
        self.controller
            .acknowledge_render_feedback(&render.feedback)?;

        Ok(PlanetTerrainAdvanceReport {
            plans_applied: frame.plans_applied,
            planning_failures,
            render,
            visibility,
        })
    }
}

fn live_runtime_config() -> TerrainRuntimeConfig {
    TerrainRuntimeConfig {
        max_planets: LIVE_MAX_PLANETS,
        max_component_sources: 16,
        max_resident_pages: 8_192,
        max_resident_dense_bytes: 8_192 * CELL_COUNT * core::mem::size_of::<u32>(),
        ..TerrainRuntimeConfig::default()
    }
}

fn live_controller_config() -> TerrainControllerConfig {
    let renderer = PlanetTerrainRuntime::renderer_config();
    TerrainControllerConfig {
        planning: TerrainPlanningConfig {
            streaming: TerrainStreamingConfig {
                max_pages: LIVE_ACTIVE_PAGES_PER_PLANET,
                ..TerrainStreamingConfig::default()
            },
            ..TerrainPlanningConfig::default()
        },
        refinement: TerrainRefinementConfig {
            max_active_pages: LIVE_ACTIVE_PAGES_PER_PLANET,
            max_transition_pages: LIVE_TRANSITION_PAGES_PER_PLANET,
            initial_coarse_pages: 32,
            max_requests_per_reconcile: 16,
            max_commits_per_reconcile: 8,
            ..TerrainRefinementConfig::default()
        },
        rendering: TerrainRenderDeltaConfig {
            max_events_per_delta: 128,
            max_commands_per_delta: 128,
            max_upload_bytes_per_delta: 16 * 1024 * 1024,
            max_tracked_pages: usize::try_from(renderer.residency.max_resident_pages)
                .expect("Helio residency page count fits usize"),
            max_visible_pages: LIVE_GPU_VISIBLE_PAGES,
        },
        max_planets: LIVE_MAX_PLANETS,
        max_planning_results_per_frame: LIVE_MAX_PLANETS,
    }
}

impl From<SubsystemError> for PlanetTerrainLiveError {
    fn from(error: SubsystemError) -> Self {
        Self::Subsystem(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_cpu_and_gpu_budgets_are_one_consistent_bounded_contract() {
        let controller = live_controller_config();
        let renderer = PlanetTerrainRuntime::renderer_config();
        assert_eq!(
            controller.max_planets * controller.refinement.max_active_pages,
            controller.rendering.max_visible_pages
        );
        assert_eq!(
            usize::try_from(renderer.residency.max_resident_pages).unwrap(),
            controller.rendering.max_tracked_pages
        );
        assert!(
            controller.max_planets * controller.refinement.max_transition_pages
                <= controller.rendering.max_tracked_pages
        );
        assert!(controller.rendering.max_visible_pages <= controller.rendering.max_tracked_pages);
        renderer.allocation_plan().unwrap();
    }
}
