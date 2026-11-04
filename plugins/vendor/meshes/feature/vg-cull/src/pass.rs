//! Render-graph wrappers: the cull is two passes (early / late) so the Hi-Z
//! build can sit between them.

use std::sync::{Arc, Mutex};

use helio_core::{PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult};

use crate::core::{VgCullBuffers, VgCullConfig, VgCullCore, VgCullFrame, VgCullInputs};

/// What the raster tiers and shading read. `Copy` so it can live in the frame
/// registry.
#[derive(Clone, Copy)]
pub struct VgCullFrameData<'a> {
    /// `array<vec2<u32>>` `{instance_id, cluster_slot}`; bin `b` entry `k` is
    /// at `b * bin_capacity + k` (the vis-buffer visible cluster index).
    pub visible_clusters: &'a wgpu::Buffer,
    /// See [`crate::state`]: counts, stats, ranges, indirect args.
    pub state: &'a wgpu::Buffer,
    pub bin_capacity: u32,
    /// 0 after the early pass, 1 after the late pass: the args/ranges block
    /// to draw (the late block is the delta).
    pub phase: u32,
}

fn frame_data<'a>(buffers: &'a VgCullBuffers, phase: u32) -> VgCullFrameData<'a> {
    VgCullFrameData {
        visible_clusters: &buffers.visible,
        state: &buffers.state,
        bin_capacity: buffers.bin_capacity,
        phase,
    }
}

pub(crate) fn make_passes(
    device: &wgpu::Device,
    config: VgCullConfig,
) -> (VgCullEarlyPass, VgCullLatePass) {
    let base_tau_px = config.tau_px;
    let core = VgCullCore::new(device, config);
    let buffers = core.buffers().clone();
    let core = Arc::new(Mutex::new(core));
    (
        VgCullEarlyPass {
            core: core.clone(),
            buffers: buffers.clone(),
            settings: None,
            base_tau_px,
            visible: helio_vg_format::density::VisibleFraction::default(),
            last_selection_feedback_revision: 0,
        },
        VgCullLatePass { core, buffers },
    )
}

impl VgCullEarlyPass {
    /// Latest asynchronously completed GPU sample; never waits for the GPU.
    pub fn selected_triangle_count(&self) -> Option<u32> {
        self.core.lock().ok()?.last_state_words().and_then(|words| {
            words
                .get((crate::state::STATS + crate::stats::TRIANGLES) as usize)
                .copied()
        })
    }

    /// Latest asynchronously completed stats sample (never waits for the GPU).
    pub fn stats_snapshot(&self) -> Option<crate::VgCullStats> {
        crate::VgCullStats::from_state_words(self.core.lock().ok()?.last_state_words()?)
    }

    /// Both halves of the cull sharing one core. Put the early pass before the
    /// early raster, the late pass after the Hi-Z rebuild.
    pub fn pair(device: &wgpu::Device, config: VgCullConfig) -> (VgCullEarlyPass, VgCullLatePass) {
        make_passes(device, config)
    }

    /// Re-read threshold, mode, occlusion and the visible-cluster ceiling from
    /// `source` at the start of every frame.
    pub fn with_settings_source(mut self, source: VgCullSettingsSource) -> Self {
        self.settings = Some(source);
        self
    }
}

/// Settings that change at runtime (project settings), re-read every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VgCullDynamic {
    pub tau_px: f32,
    pub selection_triangles_per_pixel: f32,
    pub selection_sensitivity: f32,
    pub mode: helio_vg_format::VgMode,
    pub two_phase_occlusion: bool,
    /// Ceiling on visible clusters per frame.
    pub max_visible_clusters: u32,
}

/// Source of [`VgCullDynamic`] values.
pub type VgCullSettingsSource = Arc<dyn Fn() -> VgCullDynamic + Send + Sync>;

/// Phase 0: re-draw last frame's visible set against last frame's Hi-Z.
pub struct VgCullEarlyPass {
    core: Arc<Mutex<VgCullCore>>,
    buffers: Arc<VgCullBuffers>,
    settings: Option<VgCullSettingsSource>,
    base_tau_px: f32,
    /// Share of the predicted triangles that survives occlusion.
    visible: helio_vg_format::density::VisibleFraction,
    last_selection_feedback_revision: u64,
}

/// Phase 1: retest deferred work against the Hi-Z of the phase-0 depth.
pub struct VgCullLatePass {
    core: Arc<Mutex<VgCullCore>>,
    buffers: Arc<VgCullBuffers>,
}

fn inputs<'a>(registry: &'a helio_core::ResourceRegistry<'a>) -> Option<VgCullInputs<'a>> {
    registry.get::<VgCullInputs<'a>>(ResourceKey::new(crate::KEY_INPUTS))
}

impl RenderPass for VgCullEarlyPass {
    fn name(&self) -> &'static str {
        "VgCullEarly"
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        // The integration publishes `VgCullInputs` before this pass.
        builder.read(crate::KEY_INPUTS);
        builder.write_buffer(crate::KEY_CULL_EARLY);
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // Same frame-scoped lifetime bridge as the other passes that publish
        // pass-owned buffers.
        let buffers: &'a VgCullBuffers = unsafe { std::mem::transmute(&*self.buffers) };
        frame.write(
            ResourceKey::new(crate::KEY_CULL_EARLY),
            frame_data(buffers, 0),
            "VgCullEarly",
        );
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let Some(inputs) = inputs(ctx.registry) else {
            return Ok(());
        };
        let mut core = self.core.lock().unwrap();
        if let Some(source) = &self.settings {
            let s = source();
            self.base_tau_px = if s.tau_px.is_finite() {
                s.tau_px.max(0.01)
            } else {
                1.0
            };
            core.config.selection_triangles_per_pixel =
                if s.selection_triangles_per_pixel.is_finite() {
                    s.selection_triangles_per_pixel.clamp(0.25, 32.0)
                } else {
                    2.0
                };
            core.config.selection_sensitivity = if s.selection_sensitivity.is_finite() {
                s.selection_sensitivity.clamp(0.0, 1.0)
            } else {
                1.0
            };
            if core.config.selection_sensitivity == 0.0 {
                self.visible = helio_vg_format::density::VisibleFraction::default();
            }
            core.config.mode = s.mode;
            core.config.two_phase_occlusion = s.two_phase_occlusion;
            // One bin may hold a quarter of the visible-cluster budget (distant crowds put
            // nearly everything in the smallest hardware class; an even 1/8 split
            // dropped 475k clusters of a 100k-instance view). Clamped to MAX_BIN_CAPACITY
            // by the core, so bin * capacity stays under the 24-bit visible index.
            core.config.max_bin_capacity = (s.max_visible_clusters / 4).max(16).next_power_of_two();
        }
        let revision = core.stats_revision();
        if revision != self.last_selection_feedback_revision {
            if let Some(words) = core.last_state_words() {
                let observed = words
                    .get((crate::state::STATS + crate::stats::TRIANGLES) as usize)
                    .copied()
                    .unwrap_or(0);
                let predicted = words
                    .get(crate::state::PREDICTED as usize)
                    .copied()
                    .unwrap_or(0);
                // The GPU predicts the frustum-visible triangles from the asset
                // tables; occlusion removes some. Learn that share from the
                // frame's own counts (they arrive a few frames late).
                self.visible.update(predicted as f32, observed as f32);
            }
            self.last_selection_feedback_revision = revision;
        }
        // The density solve runs on the GPU before traversal (`cs_density`): the
        // tolerance multiplier fits the triangles the target allows, with no
        // feedback on tau itself. The target gets a margin because the table is
        // quantised and instances in one cut mix neighbouring levels.
        core.config.tau_px = self.base_tau_px.clamp(0.01, 1.0e6);
        core.config.density_budget_tris = if core.config.selection_sensitivity > 0.0 {
            let pixels = f64::from(ctx.width.max(1)) * f64::from(ctx.height.max(1));
            let target = f64::from(core.config.selection_triangles_per_pixel.max(0.25)) * 0.75;
            self.visible.predicted_budget(target * pixels) as f32
        } else {
            0.0
        };
        core.begin_frame(
            ctx.device,
            ctx.queue,
            &VgCullFrame {
                width: ctx.width,
                height: ctx.height,
            },
            &inputs,
        );
        self.buffers = core.buffers().clone();
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(inputs) = inputs(ctx.registry) else {
            return Ok(());
        };
        let hiz = ctx
            .registry
            .texture_view(ResourceKey::new(crate::KEY_HIZ_EARLY));
        let mut core = self.core.lock().unwrap();
        if !core.prepared() {
            return Ok(());
        }
        let mut rec = ctx.graphics_cmds();
        core.record_phase(&mut rec, ctx.device, ctx.queue, ctx.camera, &inputs, hiz, 0);
        Ok(())
    }
}

impl RenderPass for VgCullLatePass {
    fn name(&self) -> &'static str {
        "VgCullLate"
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read(crate::KEY_CULL_EARLY);
        builder.write_buffer(crate::KEY_CULL);
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        let buffers: &'a VgCullBuffers = unsafe { std::mem::transmute(&*self.buffers) };
        frame.write(
            ResourceKey::new(crate::KEY_CULL),
            frame_data(buffers, 1),
            "VgCullLate",
        );
    }

    fn prepare(&mut self, _ctx: &PrepareContext) -> HelioResult<()> {
        self.buffers = self.core.lock().unwrap().buffers().clone();
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(inputs) = inputs(ctx.registry) else {
            return Ok(());
        };
        let mut core = self.core.lock().unwrap();
        if !core.prepared() || !core.config.two_phase_occlusion {
            return Ok(());
        }
        let hiz = ctx
            .registry
            .texture_view(ResourceKey::new(crate::KEY_HIZ_LATE));
        let mut rec = ctx.graphics_cmds();
        core.record_phase(&mut rec, ctx.device, ctx.queue, ctx.camera, &inputs, hiz, 1);
        Ok(())
    }
}
