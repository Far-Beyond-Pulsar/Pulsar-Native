use std::collections::{BTreeMap, BTreeSet};

use engine_subsystems::{Subsystem, SubsystemContext, SubsystemError};
use helio_pass_planetary_voxel::{GpuResidencyCounters, PlanetaryVoxelRenderConfig};
use helio_planet_voxel_core::VisibilityOutcome;
use pulsar_reflection::LiveKeySet;
use pulsar_terrain::{
    CELL_COUNT, PlanetId, PlanetPosition, PlanetView, PositionError, TerrainControllerConfig,
    TerrainControllerError, TerrainPlanningConfig, TerrainPlanningCounters,
    TerrainRefinementConfig, TerrainRenderDeltaConfig, TerrainRuntimeConfig,
    TerrainRuntimeCounters, TerrainRuntimeError, TerrainRuntimeHandle, TerrainStreamingConfig,
    TerrainStreamingController, TerrainStreamingError, TerrainSubsystem,
};
use thiserror::Error;

use super::{
    PlanetTerrainComponentRenderAdapter, PlanetaryTerrainRenderError, TerrainRenderApplyReport,
};

const LIVE_MAX_PLANETS: usize = 4;
const LIVE_ACTIVE_PAGES_PER_PLANET: usize = 96;
const LIVE_TRANSITION_PAGES_PER_PLANET: usize = 96;
const LIVE_GPU_VISIBLE_PAGES: usize = 384;

/// Component identities retained between revisions of Pulsar's current legacy
/// `engine_backend::scene::SceneDb` snapshot bridge. This is not the external
/// production SceneDB integration. The canonical terrain runtime remains the
/// source of truth; this cache only lets component sync remove sources that
/// disappeared from the scene.
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

/// Cross-layer limits for the live canonical-runtime-to-disposable-GPU path.
/// The aggregate contract is validated before any worker or GPU state is
/// created, so no valid per-planet configuration can overcommit the shared
/// renderer when all configured planets are active at once.
#[derive(Clone, Debug)]
pub struct PlanetTerrainLiveConfig {
    pub runtime: TerrainRuntimeConfig,
    pub controller: TerrainControllerConfig,
    pub renderer: PlanetaryVoxelRenderConfig,
}

impl PlanetTerrainLiveConfig {
    pub fn production() -> Self {
        let renderer = PlanetaryVoxelRenderConfig::horizon_demo();
        Self {
            runtime: live_runtime_config(),
            controller: live_controller_config(renderer),
            renderer,
        }
    }

    fn validate(&self) -> Result<(), PlanetTerrainLiveError> {
        let aggregate_active = self
            .controller
            .max_planets
            .checked_mul(self.controller.refinement.max_active_pages)
            .ok_or_else(|| live_config_error("aggregate active-page budget overflows usize"))?;
        let aggregate_transition = self
            .controller
            .max_planets
            .checked_mul(self.controller.refinement.max_transition_pages)
            .ok_or_else(|| live_config_error("aggregate transition-page budget overflows usize"))?;
        let aggregate_dense_bytes = aggregate_transition
            .checked_mul(CELL_COUNT)
            .and_then(|cells| cells.checked_mul(core::mem::size_of::<u32>()))
            .ok_or_else(|| live_config_error("aggregate dense-page budget overflows usize"))?;
        let gpu_residents = usize::try_from(self.renderer.residency.max_resident_pages)
            .map_err(|_| live_config_error("GPU resident-page budget does not fit usize"))?;
        let gpu_surfaces = usize::try_from(self.renderer.max_surface_pages)
            .map_err(|_| live_config_error("GPU surface-page budget does not fit usize"))?;

        if self.controller.max_planets > self.runtime.max_planets {
            return Err(live_config_error(
                "controller planet capacity exceeds the canonical runtime",
            ));
        }
        if aggregate_active > self.controller.rendering.max_visible_pages {
            return Err(live_config_error(
                "aggregate active frontier exceeds the renderer-visible page budget",
            ));
        }
        if aggregate_transition > self.controller.rendering.max_tracked_pages {
            return Err(live_config_error(
                "aggregate transition frontier exceeds the renderer-tracked page budget",
            ));
        }
        if aggregate_transition > self.runtime.max_resident_pages
            || aggregate_dense_bytes > self.runtime.max_resident_dense_bytes
        {
            return Err(live_config_error(
                "aggregate transition frontier exceeds canonical runtime residency",
            ));
        }
        if self.controller.rendering.max_tracked_pages > gpu_residents {
            return Err(live_config_error(
                "renderer-tracked page budget exceeds Helio GPU residency",
            ));
        }
        if self.controller.rendering.max_visible_pages > gpu_surfaces {
            return Err(live_config_error(
                "renderer-visible page budget exceeds Helio surface capacity",
            ));
        }
        self.renderer
            .allocation_plan()
            .map_err(|error| live_config_error(error.to_string()))?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanetTerrainLiveDiagnostics {
    pub component_sources: usize,
    pub controller_planets: usize,
    pub controller_published_pages: usize,
    pub runtime: TerrainRuntimeCounters,
    pub planning: TerrainPlanningCounters,
    pub gpu: GpuResidencyCounters,
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
    #[error("invalid live planet terrain configuration: {0}")]
    Configuration(String),
}

/// Live owner joining scene components, Pulsar's canonical terrain runtime,
/// the incremental controller, and Helio's graph-owned disposable cache.
pub struct PlanetTerrainRuntime {
    config: PlanetTerrainLiveConfig,
    subsystem: TerrainSubsystem,
    runtime: TerrainRuntimeHandle,
    controller: TerrainStreamingController,
    adapter: PlanetTerrainComponentRenderAdapter,
    component_cache: PlanetTerrainComponentCache,
}

impl PlanetTerrainRuntime {
    pub fn new() -> Result<Self, PlanetTerrainLiveError> {
        Self::new_with_config(PlanetTerrainLiveConfig::production())
    }

    pub fn new_with_config(
        config: PlanetTerrainLiveConfig,
    ) -> Result<Self, PlanetTerrainLiveError> {
        config.validate()?;
        let mut subsystem = TerrainSubsystem::new(config.runtime.clone())?;
        subsystem
            .init(&SubsystemContext::new())
            .map_err(|error| PlanetTerrainLiveError::Subsystem(error.to_string()))?;
        let runtime = subsystem.runtime_handle();
        let controller = TerrainStreamingController::new(
            runtime.clone(),
            subsystem.planning_handle(),
            config.controller,
        )?;
        Ok(Self {
            config,
            subsystem,
            runtime,
            controller,
            adapter: PlanetTerrainComponentRenderAdapter::new(),
            component_cache: PlanetTerrainComponentCache::default(),
        })
    }

    pub fn renderer_config() -> PlanetaryVoxelRenderConfig {
        PlanetTerrainLiveConfig::production().renderer
    }

    pub const fn config(&self) -> &PlanetTerrainLiveConfig {
        &self.config
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
        self.adapter
            .renderer_config(renderer)
            .is_ok_and(|config| config == self.config.renderer)
    }

    pub fn diagnostics(
        &self,
        renderer: &helio::Renderer,
    ) -> Result<PlanetTerrainLiveDiagnostics, PlanetTerrainLiveError> {
        self.validate_renderer_contract(renderer)?;
        Ok(PlanetTerrainLiveDiagnostics {
            component_sources: self.component_cache.sources.len(),
            controller_planets: self.controller.planet_count(),
            controller_published_pages: self.controller.published_page_count(),
            runtime: self.runtime.counters(),
            planning: self.subsystem.planning_handle().counters(),
            gpu: self.adapter.residency(renderer)?.counters(),
        })
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
        self.validate_renderer_contract(renderer)?;
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
        self.controller
            .acknowledge_render_feedback(&render.feedback)?;
        let visible_sets = self.controller.visible_sets(frame.frame_index)?;
        let visibility =
            self.adapter
                .apply_visible_sets(renderer, queue, frame.frame_index, visible_sets)?;

        Ok(PlanetTerrainAdvanceReport {
            plans_applied: frame.plans_applied,
            planning_failures,
            render,
            visibility,
        })
    }

    fn validate_renderer_contract(
        &self,
        renderer: &helio::Renderer,
    ) -> Result<(), PlanetTerrainLiveError> {
        let actual = self.adapter.renderer_config(renderer)?;
        if actual != self.config.renderer {
            return Err(live_config_error(format!(
                "Helio planetary graph configuration {actual:?} does not match the live contract {:?}",
                self.config.renderer
            )));
        }
        Ok(())
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

fn live_controller_config(renderer: PlanetaryVoxelRenderConfig) -> TerrainControllerConfig {
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

fn live_config_error(message: impl Into<String>) -> PlanetTerrainLiveError {
    PlanetTerrainLiveError::Configuration(message.into())
}

impl From<SubsystemError> for PlanetTerrainLiveError {
    fn from(error: SubsystemError) -> Self {
        Self::Subsystem(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::planet_terrain_component::PlanetTerrainComponent;
    use helio::{
        RendererBuilder, RendererConfig, required_experimental_features, required_wgpu_features,
        required_wgpu_limits,
    };
    use helio_default_graphs::build_default_graph_external_with_planetary_voxels;
    use helio_pass_planetary_voxel::{
        PlanetaryVoxelGpuConfig, TransvoxelGpuExtractorConfig,
        TransvoxelGpuTransitionExtractorConfig,
    };
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn live_cpu_and_gpu_budgets_are_one_consistent_bounded_contract() {
        let controller = live_controller_config(PlanetTerrainRuntime::renderer_config());
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

    #[test]
    fn live_configuration_rejects_aggregate_multi_planet_overcommit() {
        let mut config = PlanetTerrainLiveConfig::production();
        config.controller.rendering.max_visible_pages -= 1;
        assert!(matches!(
            config.validate(),
            Err(PlanetTerrainLiveError::Configuration(message))
                if message.contains("aggregate active frontier")
        ));

        let mut config = PlanetTerrainLiveConfig::production();
        config.runtime.max_resident_pages =
            config.controller.max_planets * config.controller.refinement.max_transition_pages - 1;
        assert!(matches!(
            config.validate(),
            Err(PlanetTerrainLiveError::Configuration(message))
                if message.contains("canonical runtime residency")
        ));
    }

    #[test]
    fn replacement_device_recovers_canonical_pages_without_an_extra_empty_frame() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let Some(adapter) = request_test_adapter(&instance).await else {
                eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: live planet cache recovery");
                return;
            };
            let (device, queue) = request_test_device(&adapter).await;
            let config = test_live_config();
            let mut renderer =
                test_renderer(Arc::clone(&device), Arc::clone(&queue), config.renderer);
            let mut live = PlanetTerrainRuntime::new_with_config(config.clone()).unwrap();
            let source_key = "earth:0".to_owned();
            let component = PlanetTerrainComponent {
                max_resident_pages: config.runtime.max_resident_pages as u64,
                ..PlanetTerrainComponent::default()
            };
            let definition = component.definition(&source_key).unwrap();
            let planet_id = definition.planet_id;
            live.runtime
                .upsert_component(source_key.clone(), definition)
                .unwrap();
            live.component_cache.record(source_key, planet_id);

            let mut frame_index = 0;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let report = live
                    .advance(
                        &mut renderer,
                        &device,
                        &queue,
                        test_frame_input(frame_index, false),
                    )
                    .unwrap();
                assert!(report.planning_failures.is_empty());
                let diagnostics = live.diagnostics(&renderer).unwrap();
                assert_live_bounds(&diagnostics, &config);
                if live.controller.is_converged(planet_id) == Some(true)
                    && diagnostics.controller_published_pages > 0
                    && diagnostics.gpu.resident_pages > 0
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "live planet path did not converge"
                );
                frame_index += 1;
                std::thread::yield_now();
            }

            let canonical_before = live.runtime.resident_page_generations(planet_id).unwrap();
            let planet_generation_before = live.runtime.planet_generation(planet_id).unwrap();
            let visible_pages_before = live
                .controller
                .visible_sets(frame_index)
                .unwrap()
                .into_iter()
                .map(|set| set.pages.len())
                .sum::<usize>();
            assert!(visible_pages_before > 0);
            drop(renderer);
            drop(queue);
            drop(device);
            let (device, queue) = request_test_device(&adapter).await;
            let mut rebuilt =
                test_renderer(Arc::clone(&device), Arc::clone(&queue), config.renderer);
            assert_eq!(live.diagnostics(&rebuilt).unwrap().gpu.resident_pages, 0);

            let recovery_deadline = Instant::now() + Duration::from_secs(5);
            loop {
                frame_index += 1;
                let report = live
                    .advance(
                        &mut rebuilt,
                        &device,
                        &queue,
                        test_frame_input(frame_index, false),
                    )
                    .unwrap();
                let diagnostics = live.diagnostics(&rebuilt).unwrap();
                assert_live_bounds(&diagnostics, &config);
                if matches!(
                    report.visibility,
                    VisibilityOutcome::Applied { resident, .. }
                        if resident == visible_pages_before
                ) {
                    assert!(
                        !report.render.uploads.is_empty(),
                        "the completing recovery frame must contain the final uploads"
                    );
                    break;
                }
                assert!(
                    Instant::now() < recovery_deadline,
                    "live planet cache did not recover"
                );
            }

            assert_eq!(
                live.runtime.resident_page_generations(planet_id).unwrap(),
                canonical_before,
                "disposable GPU cache recovery must not mutate canonical pages"
            );
            assert_eq!(
                live.runtime.planet_generation(planet_id).unwrap(),
                planet_generation_before
            );
        });
    }

    fn test_live_config() -> PlanetTerrainLiveConfig {
        let renderer = PlanetaryVoxelRenderConfig {
            residency: PlanetaryVoxelGpuConfig::new(48, 128, 16, 16, 48).unwrap(),
            max_surface_pages: 32,
            max_pending_surfaces: 16,
            regular: TransvoxelGpuExtractorConfig::new(1_024, 2_048).unwrap(),
            transition: TransvoxelGpuTransitionExtractorConfig::new(512, 1_536).unwrap(),
            max_surface_bytes: 32 * 1024 * 1024,
        };
        PlanetTerrainLiveConfig {
            runtime: TerrainRuntimeConfig {
                worker_count: 2,
                max_planets: 1,
                max_component_sources: 1,
                request_capacity: 128,
                critical_request_reserve: 16,
                completion_capacity: 128,
                event_capacity: 256,
                max_resident_pages: 64,
                max_resident_dense_bytes: 64 * CELL_COUNT * core::mem::size_of::<u32>(),
                max_completions_per_frame: 64,
            },
            controller: TerrainControllerConfig {
                planning: TerrainPlanningConfig {
                    streaming: TerrainStreamingConfig {
                        max_pages: 32,
                        max_traversal_nodes: 4_096,
                        ..TerrainStreamingConfig::default()
                    },
                    ..TerrainPlanningConfig::default()
                },
                refinement: TerrainRefinementConfig {
                    max_active_pages: 32,
                    max_transition_pages: 48,
                    initial_coarse_pages: 8,
                    max_requests_per_reconcile: 16,
                    max_commits_per_reconcile: 8,
                    ..TerrainRefinementConfig::default()
                },
                rendering: TerrainRenderDeltaConfig {
                    max_events_per_delta: 256,
                    max_commands_per_delta: 16,
                    max_upload_bytes_per_delta: 16 * CELL_COUNT * core::mem::size_of::<u32>(),
                    max_tracked_pages: 48,
                    max_visible_pages: 32,
                },
                max_planets: 1,
                max_planning_results_per_frame: 1,
            },
            renderer,
        }
    }

    fn test_renderer(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        planetary: PlanetaryVoxelRenderConfig,
    ) -> helio::Renderer {
        let config = RendererConfig::new(32, 32, wgpu::TextureFormat::Rgba8UnormSrgb);
        RendererBuilder::new(config)
            .with_external_device()
            .with_graph(Box::new(
                move |device, queue, scene, config, debug, camera, cull| {
                    build_default_graph_external_with_planetary_voxels(
                        device, queue, scene, config, debug, camera, cull, None, planetary,
                    )
                    .expect("bounded live planet graph must build")
                },
            ))
            .build(
                device,
                queue,
                config.width,
                config.height,
                config.surface_format,
            )
    }

    fn test_frame_input(frame_index: u64, graph_rebuilt: bool) -> PlanetTerrainFrameInput {
        PlanetTerrainFrameInput {
            camera_m: [8_000_000.0, 0.0, 0.0],
            forward: [-1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            vertical_fov_radians: 60_f64.to_radians(),
            viewport_px: [1920, 1080],
            near_m: 0.1,
            far_m: 20_000_000.0,
            velocity_mps: [0.0; 3],
            delta_time_s: 1.0 / 60.0,
            tick: frame_index,
            frame_index,
            graph_rebuilt,
        }
    }

    fn assert_live_bounds(
        diagnostics: &PlanetTerrainLiveDiagnostics,
        config: &PlanetTerrainLiveConfig,
    ) {
        assert!(diagnostics.component_sources <= config.runtime.max_component_sources);
        assert!(diagnostics.controller_planets <= config.controller.max_planets);
        assert!(diagnostics.runtime.planets <= config.runtime.max_planets);
        assert!(diagnostics.runtime.queued <= config.runtime.request_capacity);
        assert!(diagnostics.runtime.completed <= config.runtime.completion_capacity);
        assert!(diagnostics.runtime.events <= config.runtime.event_capacity);
        assert!(diagnostics.runtime.resident_pages <= config.runtime.max_resident_pages);
        assert!(
            diagnostics.runtime.resident_dense_bytes <= config.runtime.max_resident_dense_bytes
        );
        assert!(diagnostics.planning.pending <= config.controller.max_planets);
        assert!(diagnostics.planning.completed <= config.controller.max_planets);
        assert!(diagnostics.gpu.resident_pages <= config.renderer.residency.max_resident_pages);
        assert!(
            diagnostics.controller_published_pages <= config.controller.rendering.max_tracked_pages
        );
    }

    async fn request_test_adapter(instance: &wgpu::Instance) -> Option<wgpu::Adapter> {
        for force_fallback_adapter in [false, true] {
            if let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter,
                    apply_limit_buckets: false,
                })
                .await
            {
                return Some(adapter);
            }
        }
        None
    }

    async fn request_test_device(adapter: &wgpu::Adapter) -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Pulsar Live Planet Recovery Device"),
                required_features: required_wgpu_features(adapter.features()),
                required_limits: required_wgpu_limits(adapter.limits()),
                experimental_features: required_experimental_features(adapter.features()),
                ..Default::default()
            })
            .await
            .expect("Helio-compatible adapter must create a device");
        (Arc::new(device), Arc::new(queue))
    }
}
