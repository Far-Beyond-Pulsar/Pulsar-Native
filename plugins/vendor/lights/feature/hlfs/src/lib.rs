//! Hierarchical Light-Field Sampling (HLFS) Pass
//!
//! Implements visibility-guided stochastic direct lighting with bounded
//! per-pixel sampling and shadow evaluation. [`HlfsMode`] selects visibility
//! evaluation within the shared grid, reservoir and denoising pipeline.
//!
//! Architecture:
//! 1. Conservative coarse-to-fine light culling
//! 2. Visible and hidden light reservoirs with visibility feedback
//! 3. Temporal filtering of diffuse and specular lighting
//! 4. Sparse spatial filtering and full-resolution composition
//!
//! CPU work is constant in scene size. GPU culling scales with light count;
//! per-pixel candidate and shadow work stays within the configured budget.

pub mod feature;
pub use feature::HlfsFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::{ResourceBuilder, ResourceSize};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result};
mod bindings;
mod pipelines;
mod resources;
use bindings::{ExternalBindings, Inputs, InternalBindings};
use pipelines::Pipelines;
use resources::{Fallbacks, Targets, COARSE_TILE_SIZE, TILE_SIZE};

/// Visibility evaluation used by the shared HLFS pipeline.
///
/// Modes share light grids, reservoir storage and denoising history. Only
/// implemented visibility backends are selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HlfsMode {
    /// Current-frame hierarchical depth tracing with shadow-map fallback.
    /// Available without ray-query hardware; execution time depends on scene
    /// visibility and sampling settings. Never selects hardware rays implicitly.
    #[default]
    ScreenSpace,
    /// Native Vulkan triangle ray queries, optionally with thin-sheet RGB transmission.
    /// Requires a current built TLAS;
    /// never falls back to screen-space visibility when it is unavailable.
    RayTraced,
}

/// Lighting output used by capture/benchmark tools. Reference deliberately
/// evaluates every light and disables denoising; never use it for gameplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum HlfsDebugMode {
    #[default]
    Final = 0,
    Reference = 1,
    Unfiltered = 2,
    Confidence = 3,
}

/// Visibility mode and bounded quality controls. Changes invalidate all history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HlfsConfig {
    /// Visibility backend, independent of sampling quality and debug output.
    pub mode: HlfsMode,
    /// Base shadowed light samples per shading pixel, clamped to 1..=4.
    /// Presampling raises glossy pixels to four and may add a full-size key query.
    pub samples_per_pixel: u32,
    /// Experimental coarse-tile importance proposals, including sparse light sets.
    pub tile_presampling: bool,
    /// Experimental weighted temporal RIS; requires presampled RT at half resolution.
    /// Reuses proposals only while light data is unchanged, with fresh visibility.
    /// Disabled by default; current examples still fail the visual quality gate.
    pub temporal_resampling: bool,
    /// Experimental confidence-of-the-mean history clipping.
    pub reactive_history: bool,
    /// Steady-state candidates per sample, clamped to 1..=16. Disocclusion
    /// doubles discovery candidates up to the hard limit of 16.
    pub candidates_per_sample: u32,
    /// 1 = full resolution, 2 = quarter as many shading pixels.
    pub sample_scale: u32,
    pub max_history_frames: u32,
    /// Fraction of sampling weight budget reserved for discovering hidden lights.
    pub discovery_fraction: f32,
    /// Pre-exposure of intermediate lighting; undone during composition.
    pub pre_exposure: f32,
    /// Maximum contact-shadow trace distance in world units; 0 disables it.
    pub screen_trace_distance: f32,
    /// Maximum directional-light shadow distance for RayTraced mode, in world
    /// units. Local-light rays always cover the complete receiver-to-light segment.
    pub ray_trace_distance: f32,
    pub debug_mode: HlfsDebugMode,
}
impl Default for HlfsConfig {
    fn default() -> Self {
        Self {
            mode: HlfsMode::ScreenSpace,
            samples_per_pixel: 2,
            tile_presampling: false,
            temporal_resampling: false,
            reactive_history: false,
            candidates_per_sample: 8,
            sample_scale: 1,
            max_history_frames: 16,
            discovery_fraction: 0.2,
            pre_exposure: 1.0,
            screen_trace_distance: 0.5,
            ray_trace_distance: 10_000.0,
            debug_mode: HlfsDebugMode::Final,
        }
    }
}
impl HlfsConfig {
    /// Experimental two-sample tier with two candidates and half-width/height
    /// shading. Glossy surfaces use four samples with sixteen independent candidates.
    /// A dominant emitter, when present, is shaded separately at full resolution.
    /// Uses current-frame coarse-tile proposals and reactive history.
    /// Requires hardware ray queries. The 1440p timing target is validated only
    /// on the documented synthetic workload, not general scene complexity.
    pub fn ray_traced_presampled() -> Self {
        Self {
            mode: HlfsMode::RayTraced,
            samples_per_pixel: 2,
            candidates_per_sample: 2,
            sample_scale: 2,
            tile_presampling: true,
            reactive_history: true,
            discovery_fraction: 1.0,
            ..Self::default()
        }
    }

    fn validate_device(&self, device: &wgpu::Device) -> Result<()> {
        if self.temporal_resampling
            && (self.mode != HlfsMode::RayTraced
                || !self.tile_presampling
                || self.sample_scale != 2)
        {
            return Err(helio_core::Error::InvalidPassConfig(
                "Temporal RIS requires presampled RT with sample_scale=2".into(),
            ));
        }
        if self.mode == HlfsMode::RayTraced
            && (!device
                .features()
                .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
                || device.limits().max_tlas_instance_count == 0)
        {
            return Err(helio_core::Error::InvalidPassConfig(
                "HLFS RayTraced requires enabled ray queries and nonzero TLAS limits".into(),
            ));
        }
        if !self.ray_trace_distance.is_finite() || self.ray_trace_distance <= 0.0 {
            return Err(helio_core::Error::InvalidPassConfig(
                "HLFS ray_trace_distance must be finite and positive".into(),
            ));
        }
        Ok(())
    }
    /// Two samples over each 2x2 shading block. Uses less sampling work than
    /// `performance()`, with more variance. Output format is selected separately;
    /// pair with `HlfsPass::preferred_output_format()` for packed HDR.
    pub fn compact() -> Self {
        Self {
            samples_per_pixel: 2,
            sample_scale: 2,
            ..Self::default()
        }
    }
    /// Four samples over each 2x2 shading block: one sample per output pixel.
    pub fn performance() -> Self {
        Self {
            samples_per_pixel: 4,
            sample_scale: 2,
            ..Self::default()
        }
    }
    fn normalized(mut self) -> Self {
        self.samples_per_pixel = self.samples_per_pixel.clamp(1, 4);
        self.candidates_per_sample = self.candidates_per_sample.clamp(1, 16);
        self.sample_scale = self.sample_scale.clamp(1, 2);
        self.max_history_frames = self.max_history_frames.clamp(1, 32);
        self.discovery_fraction = if self.discovery_fraction.is_finite() {
            self.discovery_fraction.clamp(0.05, 1.0)
        } else {
            0.2
        };
        self.pre_exposure = if self.pre_exposure.is_finite() {
            self.pre_exposure.clamp(1e-6, 1e6)
        } else {
            1.0
        };
        self.screen_trace_distance = if self.screen_trace_distance.is_finite() {
            self.screen_trace_distance.clamp(0.0, 10.0)
        } else {
            0.5
        };
        if self.debug_mode == HlfsDebugMode::Reference {
            self.sample_scale = 1;
        }
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    frame: u32,
    sample_count: u32,
    light_count: u32,
    history_valid: u32,
    screen_size: [u32; 2],
    sample_size: [u32; 2],
    sample_scale: u32,
    candidate_count: u32,
    has_velocity: u32,
    surface_flags: u32,
    max_history: f32,
    discovery_fraction: f32,
    exposure: f32,
    debug_mode: u32,
    ambient: [f32; 4],
    csm_splits: [f32; 4],
    previous_view: [f32; 16],
    ray_settings: [f32; 4],
    inverse_view: [f32; 16],
    inverse_projection: [f32; 16],
}

pub struct HlfsPass {
    pipelines: Pipelines,
    targets: Targets,
    fallbacks: Fallbacks,
    internal: InternalBindings,
    external: ExternalBindings,
    compact_lights: wgpu::Buffer,
    /// Live light rows, `[count, row...]` (Pulsar-Native#838). Rebuilt only
    /// when the light rows change: `live_key` is the `scene_lights` buffer's
    /// `(epoch, content_generation, row_capacity)` it was built from.
    live_lights: wgpu::Buffer,
    live_key: Option<(u64, u64, u32)>,
    /// Set by `prepare` when `live_key` is stale; `record` rebuilds the list.
    live_rebuild: Option<(u64, u64, u32)>,
    globals: wgpu::Buffer,
    shadows: wgpu::Buffer,
    config: HlfsConfig,
    shadow_quality: helio_light_api::ShadowQuality,
    output_format: wgpu::TextureFormat,
    write_history: usize,
    history_valid: bool,
    previous_camera: Option<helio_core::GpuCameraUniforms>,
    current_light_count: u32,
    previous_light_count: Option<u32>,
    previous_light_generation: Option<u64>,
    previous_frame: Option<u64>,
    timing_query: Option<wgpu::QuerySet>,
}
impl HlfsPass {
    /// Compact linear HDR when renderable on this device, with a portable fallback.
    pub fn preferred_output_format(device: &wgpu::Device) -> wgpu::TextureFormat {
        if device
            .features()
            .contains(wgpu::Features::RG11B10UFLOAT_RENDERABLE)
        {
            wgpu::TextureFormat::Rg11b10Ufloat
        } else {
            wgpu::TextureFormat::Rgba16Float
        }
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
    ) -> Self {
        Self::with_config(
            device,
            queue,
            width,
            height,
            output_format,
            HlfsConfig::default(),
        )
    }
    pub fn with_config(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
        config: HlfsConfig,
    ) -> Self {
        Self::try_with_config(device, queue, width, height, output_format, config)
            .expect("invalid HLFS configuration")
    }
    /// Fallible configuration entry point for an explicitly requested backend.
    pub fn try_with_config(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
        config: HlfsConfig,
    ) -> Result<Self> {
        config.validate_device(device)?;
        let config = config.normalized();
        let pipelines = Pipelines::new(device, output_format, config.mode, config.tile_presampling);
        let targets = Targets::new(device, width, height, output_format, config, None);
        let internal = InternalBindings::new(device, &pipelines, &targets);
        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = uniform("HLFS globals", std::mem::size_of::<Globals>() as u64);
        let shadows = uniform(
            "HLFS shadow settings",
            std::mem::size_of::<helio_light_api::ShadowConfig>() as u64,
        );
        let shadow_quality = helio_light_api::ShadowQuality::High;
        queue.write_buffer(
            &shadows,
            0,
            bytemuck::bytes_of(&helio_light_api::ShadowConfig::from_quality(shadow_quality)),
        );
        Ok(Self {
            pipelines,
            targets,
            fallbacks: Fallbacks::new(device, queue),
            internal,
            external: ExternalBindings::default(),
            compact_lights: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS compact light rows"),
                size: 64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            // Zero-initialised: a count of zero until the first list is built.
            live_lights: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS live light rows"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            live_key: None,
            live_rebuild: None,
            globals,
            shadows,
            config,
            shadow_quality,
            output_format,
            write_history: 0,
            history_valid: false,
            previous_camera: None,
            current_light_count: 0,
            previous_light_count: None,
            previous_light_generation: None,
            previous_frame: None,
            timing_query: None,
        })
    }
    pub fn config(&self) -> HlfsConfig {
        self.config
    }
    pub fn set_config(&mut self, device: &wgpu::Device, config: HlfsConfig) {
        self.try_set_config(device, config)
            .expect("invalid HLFS configuration");
    }
    /// Reject an unsupported backend without changing the current mode or history.
    pub fn try_set_config(&mut self, device: &wgpu::Device, config: HlfsConfig) -> Result<()> {
        config.validate_device(device)?;
        let config = config.normalized();
        if config == self.config {
            return Ok(());
        }
        let mode_changed = config.mode != self.config.mode
            || config.tile_presampling != self.config.tile_presampling;
        let resize = config.sample_scale != self.config.sample_scale
            || config.tile_presampling != self.config.tile_presampling
            || config.temporal_resampling != self.config.temporal_resampling
            || (config.temporal_resampling
                && config.samples_per_pixel != self.config.samples_per_pixel);
        self.config = config;
        if mode_changed {
            self.pipelines
                .set_mode(device, config.mode, config.tile_presampling);
            self.external.clear_ray_binding();
        }
        if resize {
            self.recreate_targets(device, self.targets.width, self.targets.height);
        }
        self.invalidate_history();
        Ok(())
    }
    pub fn set_shadow_quality(
        &mut self,
        quality: helio_light_api::ShadowQuality,
        queue: &wgpu::Queue,
    ) {
        self.shadow_quality = quality;
        queue.write_buffer(
            &self.shadows,
            0,
            bytemuck::bytes_of(&helio_light_api::ShadowConfig::from_quality(quality)),
        );
        self.invalidate_history();
    }
    pub fn invalidate_history(&mut self) {
        self.history_valid = false;
        self.previous_camera = None;
        self.previous_frame = None;
    }
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if (width.max(1), height.max(1)) == (self.targets.width, self.targets.height) {
            return;
        }
        self.recreate_targets(device, width, height);
    }
    fn recreate_targets(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        // Internal shading changes retain the published output handle so
        // downstream passes can keep their existing bindings.
        let output = ((width.max(1), height.max(1)) == (self.targets.width, self.targets.height))
            .then(|| resources::Image {
                texture: self.targets.output.texture.clone(),
                view: self.targets.output.view.clone(),
            });
        self.targets = Targets::new(
            device,
            width,
            height,
            self.output_format,
            self.config,
            output,
        );
        self.internal = InternalBindings::new(device, &self.pipelines, &self.targets);
        self.external = ExternalBindings::default();
        self.write_history = 0;
        self.invalidate_history();
    }
    /// Stable capture source. Reacquire after resize; includes COPY_SRC usage.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.targets.output.texture
    }
    /// Read-only GPU grids for opt-in capture diagnostics. Both buffers have
    /// COPY_SRC usage; each row begins with its accepted-light count.
    pub fn diagnostic_light_grids(&self) -> (&wgpu::Buffer, &wgpu::Buffer) {
        (&self.targets.coarse, &self.targets.grid)
    }
    /// Enable seven frame-boundary timestamps: coarse, fine, sampling, temporal,
    /// spatial, composite and completion. Resolve after the frame submission completes.
    pub fn enable_timing(&mut self, device: &wgpu::Device) -> bool {
        if !device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        ) {
            return false;
        }
        self.timing_query = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("HLFS stage timings"),
            ty: wgpu::QueryType::Timestamp,
            count: 7,
        }));
        true
    }
    pub fn timing_query(&self) -> Option<&wgpu::QuerySet> {
        self.timing_query.as_ref()
    }
    /// Exact requested allocation bytes, excluding backend alignment/driver overhead.
    pub fn allocation_bytes(&self) -> u64 {
        let t = &self.targets;
        let pixels = u64::from(t.sample_width) * u64::from(t.sample_height);
        let output = u64::from(t.width)
            * u64::from(t.height)
            * u64::from(self.output_format.block_copy_size(None).unwrap_or(4));
        pixels * 40
            + (0..t.depth_bounds.texture.mip_level_count())
                .map(|mip| {
                    u64::from((t.depth_bounds.texture.width() >> mip).max(1))
                        * u64::from((t.depth_bounds.texture.height() >> mip).max(1))
                        * 4
                })
                .sum::<u64>()
            + output
            + self.compact_lights.size()
            + self.live_lights.size()
            + t.coarse.size()
            + t.grid.size()
            + t.proposals.size()
            + t.history.iter().map(|h| h.visible.size()).sum::<u64>()
            + self.globals.size()
            + self.shadows.size()
            + 32 * 32 * 32
            + 8
            + 16
            + 4
    }
    fn record(&mut self, encoder: &mut CommandRecorder<'_>) {
        let p = &self.pipelines;
        let t = &self.targets;
        let parity = self.write_history;
        let common = self.external.common.as_ref().expect("HLFS inputs bound");
        let gbuffer = self.external.gbuffer.as_ref().expect("HLFS GBuffer bound");
        let timestamp = |encoder: &mut CommandRecorder<'_>, index| {
            if let Some(query) = &self.timing_query {
                encoder.write_timestamp(query, index);
            }
        };
        let dispatch = |encoder: &mut CommandRecorder<'_>,
                        name,
                        pipeline: &wgpu::ComputePipeline,
                        resources: &wgpu::BindGroup,
                        x,
                        y,
                        visibility_resources: Option<&wgpu::BindGroup>| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(name),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, common, &[]);
            pass.set_bind_group(1, gbuffer, &[]);
            pass.set_bind_group(2, resources, &[]);
            if let Some(resources) = visibility_resources {
                pass.set_bind_group(3, resources, &[]);
            }
            pass.dispatch_workgroups(x, y, 1);
        };
        timestamp(encoder, 0);
        if self.current_light_count > 0 {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("HLFS compact lights"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&p.compact);
            pass.set_bind_group(
                0,
                self.external
                    .compact
                    .as_ref()
                    .expect("HLFS light copy bound"),
                &[],
            );
            pass.dispatch_workgroups(self.current_light_count.div_ceil(256), 1, 1);
        }
        if let Some(key) = self.live_rebuild.take() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("HLFS live light list"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&p.list_live);
            pass.set_bind_group(
                0,
                self.external
                    .compact
                    .as_ref()
                    .expect("HLFS light copy bound"),
                &[],
            );
            // One workgroup: an ordered scan keeps the list deterministic.
            pass.dispatch_workgroups(1, 1, 1);
            drop(pass);
            self.live_key = Some(key);
        }
        if self.config.tile_presampling {
            dispatch(
                encoder,
                "HLFS dominant light",
                &p.select_key,
                &self.internal.grid,
                1,
                1,
                None,
            );
        }
        dispatch(
            encoder,
            "HLFS coarse light cull",
            &p.coarse,
            &self.internal.grid,
            t.width.div_ceil(COARSE_TILE_SIZE),
            t.height.div_ceil(COARSE_TILE_SIZE),
            None,
        );
        timestamp(encoder, 1);
        dispatch(
            encoder,
            "HLFS fine light cull",
            &p.fine,
            &self.internal.grid,
            t.width.div_ceil(TILE_SIZE),
            t.height.div_ceil(TILE_SIZE),
            None,
        );
        for (index, resources) in self
            .internal
            .depth_reduce
            .iter()
            .enumerate()
            .filter(|_| self.config.mode == HlfsMode::ScreenSpace)
        {
            let mip = index as u32 + 1;
            dispatch(
                encoder,
                "HLFS depth pyramid",
                &p.depth_reduce,
                resources,
                (t.depth_bounds.texture.width() >> mip).max(1).div_ceil(8),
                (t.depth_bounds.texture.height() >> mip).max(1).div_ceil(8),
                None,
            );
        }
        let small = self
            .previous_light_count
            .is_some_and(|count| count <= self.config.samples_per_pixel);
        timestamp(encoder, 2);
        dispatch(
            encoder,
            "HLFS sample and visibility",
            if self.external.transmission {
                p.transmission_visibility.as_ref().unwrap().pipeline(small)
            } else {
                p.visibility.pipeline(small)
            },
            &self.internal.sample[parity],
            t.sample_width.div_ceil(8),
            t.sample_height.div_ceil(8),
            self.external.ray.as_ref(),
        );
        timestamp(encoder, 3);
        dispatch(
            encoder,
            "HLFS temporal filter",
            &p.temporal,
            &self.internal.temporal[parity],
            t.sample_width.div_ceil(8),
            t.sample_height.div_ceil(8),
            None,
        );
        timestamp(encoder, 4);
        dispatch(
            encoder,
            "HLFS spatial filter",
            &p.spatial,
            &self.internal.spatial[parity],
            t.sample_width.div_ceil(8),
            t.sample_height.div_ceil(8),
            None,
        );
        timestamp(encoder, 5);
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &t.output.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HLFS spatial filter and composite"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if self.external.transmission {
                p.transmission_composite.as_ref().unwrap()
            } else {
                &p.composite
            });
            pass.set_bind_group(0, common, &[]);
            pass.set_bind_group(1, gbuffer, &[]);
            pass.set_bind_group(2, &self.internal.composite[parity], &[]);
            if let Some(ray) = &self.external.ray {
                pass.set_bind_group(3, ray, &[]);
            }
            pass.draw(0..3, 0..1);
        }
        timestamp(encoder, 6);
        self.write_history = 1 - parity;
        self.history_valid = true;
    }
}
impl RenderPass for HlfsPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[helio_light_api::LIGHT_LIST]
    }

    fn on_resize(&mut self, device: &wgpu::Device, size: helio_core::RenderSize) {
        let [width, height] = size.internal;
        self.resize(device, width, height);
    }
    fn requires_ray_tracing(&self) -> bool {
        self.config.mode == HlfsMode::RayTraced
    }

    fn name(&self) -> &'static str {
        "HLFS"
    }
    fn reads(&self) -> &'static [&'static str] {
        &[
            "gbuffer",
            "pre_aa",
            "render_environment",
            "ray_transmission",
            helio_light_api::STATIC_SHADOW_ATLAS,
            helio_light_api::SHADOW_TRANSMITTANCE,
            helio_frame_api::lighting::INDIRECT_DIFFUSE,
        ]
    }
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        let output: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(&self.targets.output.view) };
        frame.write(helio_core::ResourceKey::new("pre_aa"), output, "HLFS");
    }
    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }
    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.write_color_raw("pre_aa", self.output_format, ResourceSize::MatchSurface);
    }
    fn prepare(&mut self, ctx: &PrepareContext) -> Result<()> {
        if self.config.mode == HlfsMode::RayTraced
            && ctx
                .registry
                .get::<helio_core::RenderEnvironment>(helio_core::ResourceKey::new(
                    "render_environment",
                ))
                .and_then(|e| e.tlas)
                .is_none()
        {
            self.invalidate_history();
            self.external.clear_ray_binding();
            return Err(helio_core::Error::InvalidPassConfig(
                "HLFS RayTraced requires a built TLAS published for this frame".into(),
            ));
        }
        let camera = *ctx.camera_data;
        let scene_lights = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST));
        // SceneDB rows are sparse. Include every allocated row (vacant rows are
        // zeroed), never a fixed limit or a truncated live-light population.
        let light_count = scene_lights.map_or(0, |l| l.row_capacity());
        self.current_light_count = light_count;
        let required_size = u64::from(light_count.max(1)) * 64;
        if self.compact_lights.size() < required_size {
            self.compact_lights = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS compact light rows"),
                size: required_size.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
        }
        // Live row list: rebuilt when the rows change, not every frame, so a
        // mostly-despawned scene pays for its live lights only (#838).
        let live_size = (u64::from(light_count) + 1) * 4;
        if self.live_lights.size() < live_size {
            self.live_lights = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS live light rows"),
                size: live_size.next_power_of_two().max(16),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            self.live_key = None;
        }
        let live_key =
            scene_lights.map_or((0, 0, 0), |l| (l.epoch, l.content_generation, light_count));
        self.live_rebuild = (self.live_key != Some(live_key)).then_some(live_key);

        let continuity = self
            .previous_frame
            .is_some_and(|f| f.wrapping_add(1) == ctx.frame_num);
        let camera_continuity = self.previous_camera.as_ref().is_some_and(|old| {
            old.view_proj
                .iter()
                .zip(camera.prev_view_proj)
                .all(|(a, b)| (a - b).abs() < 1e-3)
        });
        // Guided IDs are reweighted against current light data every frame.
        // Smooth light animation retains history; photometric clamping handles
        // changed illumination and a changed population invalidates all IDs.
        self.history_valid &= continuity
            && camera_continuity
            && self.previous_light_count == Some(light_count)
            && !ctx.resize;
        let mut ambient = [0.03, 0.03, 0.03, self.config.screen_trace_distance];
        if let Some(environment) =
            ctx.registry
                .get::<helio_core::RenderEnvironment>(helio_core::ResourceKey::new(
                    "render_environment",
                ))
        {
            for (i, v) in ambient[..3].iter_mut().enumerate() {
                *v = environment.ambient_color[i] * environment.ambient_intensity;
            }
        }
        let g = Globals {
            frame: ctx.frame_num as u32,
            sample_count: self.config.samples_per_pixel,
            light_count,
            history_valid: self.history_valid as u32,
            screen_size: [self.targets.width, self.targets.height],
            sample_size: [self.targets.sample_width, self.targets.sample_height],
            sample_scale: self.config.sample_scale,
            candidate_count: self.config.candidates_per_sample,
            has_velocity: ctx
                .registry
                .get::<&wgpu::TextureView>(helio_core::ResourceKey::new("gbuffer_velocity"))
                .is_some() as u32,
            // Baked lightmaps are optional.  Screen-space HLFS is the dynamic,
            // no-bake path, so use non-panicking lookups for these resources.
            surface_flags: (ctx
                .registry
                .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                    helio_bake_types::BAKED_LIGHTMAP,
                ))
                .is_some()
                && ctx
                    .registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new("gbuffer_lightmap_uv"))
                    .is_some()) as u32
                | (u32::from(self.previous_light_generation == Some(ctx.frame_num)) << 1)
                | (u32::from(self.config.tile_presampling) << 2)
                | (u32::from(self.config.temporal_resampling) << 3)
                | (u32::from(
                    self.config.mode == HlfsMode::RayTraced
                        && ctx
                            .registry
                            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new("ray_transmission"))
                            .is_some(),
                ) << 4),
            max_history: self.config.max_history_frames as f32,
            discovery_fraction: self.config.discovery_fraction,
            exposure: self.config.pre_exposure,
            debug_mode: self.config.debug_mode as u32,
            ambient,
            csm_splits: helio_light_api::CSM_SPLITS,
            previous_view: self.previous_camera.map_or(camera.view, |c| c.view),
            ray_settings: [
                self.config.ray_trace_distance,
                0.0,
                u32::from(self.config.reactive_history) as f32,
                0.0,
            ],
            inverse_view: glam::Mat4::from_cols_array(&camera.view)
                .as_dmat4()
                .inverse()
                .as_mat4()
                .to_cols_array(),
            inverse_projection: glam::Mat4::from_cols_array(&camera.proj)
                .as_dmat4()
                .inverse()
                .as_mat4()
                .to_cols_array(),
        };
        ctx.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        self.previous_camera = Some(camera);
        self.previous_frame = Some(ctx.frame_num);
        self.previous_light_count = Some(light_count);
        // Allocation epochs do not report in-place SceneDB light edits. Keep
        // composite repair conservative until a content revision is provided.
        self.previous_light_generation = Some(ctx.frame_num);
        Ok(())
    }
    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        let gbuffer = ctx
            .registry
            .read::<helio_core::ViewGroup<'_, 4>>(helio_core::ResourceKey::new("gbuffer"), "HLFS")
            .ok_or_else(|| {
                helio_core::Error::InvalidPassConfig("HLFS requires a GBuffer".into())
            })?;
        let pre_aa = ctx
            .registry
            .read_texture_view(helio_core::ResourceKey::new("pre_aa"), "HLFS")
            .ok_or_else(|| helio_core::Error::InvalidPassConfig("HLFS requires pre_aa".into()))?;
        let f = &self.fallbacks;
        let lights_buf = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST))
            .map(|handle| &handle.buffer)
            .unwrap_or(&f.empty_lights);
        let shadow_matrices_buf = ctx
            .registry
            .get::<helio_light_api::ShadowMatricesFrameData<'_>>(helio_light_api::shadow_matrices())
            .map(|s| s.shadow_matrices)
            .unwrap_or(&f.empty_shadow_matrices);
        // Published only while translucent shadow casters exist; RGB
        // visibility (the transmission pipelines) is paid for only then.
        let glass = ctx
            .registry
            .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                helio_light_api::SHADOW_TRANSMITTANCE,
            ));
        let screen_space_glass = self.config.mode == HlfsMode::ScreenSpace && glass.is_some();
        let inputs = Inputs {
            camera: ctx.camera,
            lights: lights_buf,
            compact_lights: &self.compact_lights,
            live_lights: &self.live_lights,
            shadow_matrices: shadow_matrices_buf,
            shadow_atlas: ctx
                .registry
                .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                    helio_light_api::SHADOW_ATLAS,
                ))
                .unwrap_or(&f.shadow_view),
            static_shadow_atlas: ctx
                .registry
                .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                    helio_light_api::STATIC_SHADOW_ATLAS,
                ))
                .unwrap_or(&f.shadow_view),
            shadow_transmittance: glass.unwrap_or(&f.black_array_view),
            linear_sampler: &f.linear_sampler,
            shadow_sampler: ctx
                .registry
                .get::<&wgpu::Sampler>(helio_core::ResourceKey::new(
                    helio_light_api::SHADOW_SAMPLER,
                ))
                .unwrap_or(&f.shadow_sampler),
            textures: [
                gbuffer.views[0],
                gbuffer.views[1],
                gbuffer.views[2],
                gbuffer.views[3],
                ctx.depth,
                ctx.registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new("gbuffer_lightmap_uv"))
                    .unwrap_or(&f.lightmap_uv.view),
                ctx.registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                        helio_bake_types::BAKED_LIGHTMAP,
                    ))
                    .unwrap_or(&f.black.view),
                pre_aa,
                ctx.registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new("gbuffer_velocity"))
                    .unwrap_or(&f.black.view),
            ],
            lightmap_sampler: ctx
                .registry
                .get::<&wgpu::Sampler>(helio_core::ResourceKey::new("baked_lightmap_sampler"))
                .unwrap_or(&f.linear_sampler),
            gi_irradiance: ctx
                .registry
                .buffer_binding(helio_frame_api::lighting::INDIRECT_DIFFUSE)
                .unwrap_or(&f.empty_gi),
        };
        self.external.update(
            ctx.device,
            &self.pipelines,
            &self.fallbacks,
            &self.globals,
            &self.shadows,
            &inputs,
        );
        self.external.transmission = screen_space_glass
            || (self.config.mode == HlfsMode::RayTraced
                && ctx
                    .registry
                    .get::<&wgpu::Buffer>(helio_core::ResourceKey::new("ray_transmission"))
                    .is_some());
        if self.config.mode == HlfsMode::RayTraced {
            let tlas = ctx
                .registry
                .get::<helio_core::RenderEnvironment>(helio_core::ResourceKey::new(
                    "render_environment",
                ))
                .and_then(|e| e.tlas)
                .ok_or_else(|| {
                    helio_core::Error::InvalidPassConfig("HLFS lost its frame TLAS".into())
                })?;
            self.external.update_ray(
                ctx.device,
                self.pipelines.rt_bgl.as_ref().expect("validated RT layout"),
                tlas,
                ctx.registry
                    .get::<&wgpu::Buffer>(helio_core::ResourceKey::new("ray_transmission"))
                    .unwrap_or(&f.empty_lights),
            );
        }
        // These dispatches read the GBuffer just rendered, so they belong on the
        // render encoder rather than the early compute encoder.
        // Returning no render descriptor guarantees there is no active render pass.
        self.record(&mut ctx.graphics_cmds());
        Ok(())
    }
}

#[cfg(test)]
mod key_selection_tests;

#[cfg(test)]
mod tests {
    #[test]
    fn quality_and_reference_settings_retain_screen_space_mode() {
        use super::{HlfsConfig, HlfsDebugMode, HlfsMode};
        for preset in [
            HlfsConfig::default(),
            HlfsConfig::compact(),
            HlfsConfig::performance(),
        ] {
            for debug_mode in [
                HlfsDebugMode::Final,
                HlfsDebugMode::Reference,
                HlfsDebugMode::Unfiltered,
                HlfsDebugMode::Confidence,
            ] {
                let config = HlfsConfig {
                    debug_mode,
                    ..preset
                }
                .normalized();
                assert_eq!(config.mode, HlfsMode::ScreenSpace);
            }
        }
    }

    #[test]
    fn all_wgsl_stages_validate() {
        for stage in [
            "grid",
            "depth",
            "screen_space",
            "temporal",
            "spatial",
            "composite",
            "ray_traced",
            "ray_traced_composite",
        ] {
            for presampled in [false, true] {
                let source = super::pipelines::shader_source_for_sampler(stage, presampled);
                let source = helio_core::shader::resolve(&source);
                let module = naga::front::wgsl::parse_str(&source)
                    .unwrap_or_else(|e| panic!("{stage}: {}", e.emit_to_string(&source)));
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all(),
                )
                .validate(&module)
                .unwrap_or_else(|e| panic!("{stage}: {e:?}"));
            }
        }
    }
}
