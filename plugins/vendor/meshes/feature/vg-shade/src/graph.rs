//! Render-graph wrapper of the shading: [`VgShadePass`] runs `vis_resolve`,
//! material binning, `vg_shade` (which writes the G-buffer by storage
//! texture) and the final depth pass.
//!
//! It is the last pass of the virtual-geometry chain and reads what the
//! earlier ones published: `vg_cull_inputs`, `vg_cull`, `vg_raster` and
//! (optionally) `vg_dense`. A frame without published cull inputs (virtual
//! geometry idle) records nothing.

use helio_core::graph::ResourceBuilder;
use helio_core::{
    BufferKey, PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult,
    ViewGroup,
};
use helio_pass_vg_cull::{VgCullFrameData, VgCullInputs, KEY_CULL, KEY_INPUTS};
use helio_pass_vg_dense::{VgDenseFrameData, KEY_DENSE};
use helio_pass_vg_raster::{VgRasterFrameData, KEY_RASTER};

use std::sync::Arc;

use helio_core::{DebugViewDescriptor, DebugViewLegend, DebugViewLegendStop};
use helio_vg_format::debug_view::{self, VgDebugView};

use crate::{VgShade, VgShadeDebug, VgShadeGlobals, VgShadeInputs, VgShadeTargets};

/// Source of the LOD error threshold (pixels), re-read every frame; only the
/// debug views use it.
pub type VgTauSource = Arc<dyn Fn() -> f32 + Send + Sync>;

const VIEW_COUNT: usize = debug_view::ALL.len();

const fn view_descriptors() -> [DebugViewDescriptor; VIEW_COUNT] {
    let mut out = [DebugViewDescriptor {
        name: "",
        debug_mode: 0,
        description: "",
    }; VIEW_COUNT];
    let mut i = 0;
    while i < VIEW_COUNT {
        let view = debug_view::ALL[i];
        out[i] = DebugViewDescriptor {
            name: view.name(),
            debug_mode: view.id(),
            description: view.description(),
        };
        i += 1;
    }
    out
}

static VIEWS: [DebugViewDescriptor; VIEW_COUNT] = view_descriptors();

const fn stop(label: &'static str, color: [u8; 3]) -> DebugViewLegendStop {
    DebugViewLegendStop { label, color }
}

const HEAT: [[u8; 3]; 5] = [
    [0, 0, 255],
    [0, 255, 255],
    [0, 255, 0],
    [255, 255, 0],
    [255, 0, 0],
];

macro_rules! heat_stops {
    ($($label:expr),*) => {{
        const LABELS: [&str; 5] = [$($label),*];
        const STOPS: [DebugViewLegendStop; 5] = [
            stop(LABELS[0], HEAT[0]),
            stop(LABELS[1], HEAT[1]),
            stop(LABELS[2], HEAT[2]),
            stop(LABELS[3], HEAT[3]),
            stop(LABELS[4], HEAT[4]),
        ];
        &STOPS as &'static [DebugViewLegendStop]
    }};
}

/// The color scale of the views that have one.
fn legend(view: VgDebugView) -> Option<DebugViewLegend> {
    static TIER: [DebugViewLegendStop; 5] = [
        stop("HW 32", [77, 140, 255]),
        stop("HW 96-128", [13, 31, 153]),
        stop("Micro SW", [26, 230, 51]),
        stop("Dense", [255, 140, 13]),
        stop("Ray traced", [255, 26, 230]),
    ];
    static RESIDENCY: [DebugViewLegendStop; 4] = [
        stop("Leaf", [26, 179, 179]),
        stop("Refined", [51, 217, 51]),
        stop("Fallback", [115, 10, 10]),
        stop("Urgent", [255, 10, 10]),
    ];
    static CULL: [DebugViewLegendStop; 5] = [
        stop("Phase 0", [26, 179, 51]),
        stop("Phase 1", [242, 204, 26]),
        stop("Frustum edge", [0, 255, 255]),
        stop("Backface edge", [255, 0, 255]),
        stop("Near tau", [255, 26, 26]),
    ];
    static MASK: [DebugViewLegendStop; 3] = [
        stop("Empty", [10, 10, 10]),
        stop("SW tris", [64, 64, 64]),
        stop("Dense", [255, 128, 13]),
    ];
    let (title, unit, stops): (&'static str, &'static str, &'static [DebugViewLegendStop]) =
        match view {
            VgDebugView::LodLevel => (
                "Cluster coarseness",
                "error / size",
                heat_stops!("Finest", "", "Mid", "", "Coarsest"),
            ),
            VgDebugView::DetailRatio => (
                "Detail vs asset root",
                "log error ratio",
                heat_stops!("Full detail", "", "", "", "Root"),
            ),
            VgDebugView::TriangleSize => (
                "Projected triangle area",
                "px²",
                heat_stops!("64+", "16", "4", "1", "0.1"),
            ),
            VgDebugView::SwOverdraw => (
                "SW overdraw per tile",
                "hits / pixel",
                heat_stops!("0", "2", "4", "6", "8+"),
            ),
            VgDebugView::DenseOverlap => (
                "SW triangles per tile",
                "triangles",
                heat_stops!("0", "16", "32", "48", "64+"),
            ),
            VgDebugView::ErrorVsTau => (
                "Projected error / tau",
                "ratio",
                heat_stops!("0", "0.25", "0.5", "0.75", "1"),
            ),
            VgDebugView::RasterTier => ("Raster tier", "", &TIER),
            VgDebugView::PageResidency => ("Page residency", "", &RESIDENCY),
            VgDebugView::CullSurvival => ("Cull survival", "", &CULL),
            VgDebugView::DenseMask => ("Dense tile mask", "", &MASK),
            _ => return None,
        };
    Some(DebugViewLegend { title, unit, stops })
}

/// Tier-3-aware shading as a graph pass.
pub struct VgShadePass {
    shade: VgShade,
    has_dense: bool,
    debug_mode: u32,
    tau: Option<VgTauSource>,
    globals: VgShadeGlobals,
}

impl VgShadePass {
    /// `has_dense`: a `VgDensePass` precedes this one (declares the ordering).
    pub fn new(device: &wgpu::Device, has_dense: bool) -> Self {
        Self {
            shade: VgShade::new(device),
            has_dense,
            debug_mode: 0,
            tau: None,
            globals: VgShadeGlobals::default(),
        }
    }

    /// The LOD error threshold the debug views compare against (default 1 px).
    pub fn with_tau_source(mut self, tau: VgTauSource) -> Self {
        self.tau = Some(tau);
        self
    }

    /// The wrapped pass, e.g. to share a template registry.
    pub fn shade_mut(&mut self) -> &mut VgShade {
        &mut self.shade
    }
}

impl RenderPass for VgShadePass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["materials", "scene_lights"]
    }

    fn name(&self) -> &'static str {
        "VgShade"
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            "gbuffer",
            "gbuffer_lightmap_uv",
            "gbuffer_sss",
            "gbuffer_extra",
            "gbuffer_velocity",
            "depth",
            "material_textures",
            "render_environment",
        ]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(KEY_INPUTS);
        builder.read(KEY_CULL);
        builder.read(KEY_RASTER);
        if self.has_dense {
            builder.read(KEY_DENSE);
        }
    }

    fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        &VIEWS
    }

    fn debug_view_category(&self, debug_mode: u32) -> &'static str {
        if VgDebugView::from_id(debug_mode).is_some() {
            debug_view::CATEGORY
        } else {
            ""
        }
    }

    fn debug_view_legend(&self, debug_mode: u32) -> Option<DebugViewLegend> {
        legend(VgDebugView::from_id(debug_mode)?)
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // The classic G-buffer pass's per-frame constants, so a material
        // evaluates identically in both paths.
        let environment = ctx
            .registry
            .get::<helio_core::RenderEnvironment>(helio_core::resource_keys::render_environment());
        let (ambient_color, ambient_intensity) = match environment {
            Some(e) => (
                [
                    e.ambient_color[0],
                    e.ambient_color[1],
                    e.ambient_color[2],
                    1.0,
                ],
                e.ambient_intensity,
            ),
            None => ([0.1, 0.1, 0.15, 1.0], 0.1),
        };
        self.globals = VgShadeGlobals {
            frame: ctx.frame_num as u32,
            delta_time: ctx.delta_time,
            light_count: ctx
                .scene_buffers
                .get(BufferKey::of("scene_lights"))
                .map_or(0, |lights| lights.row_capacity()),
            ambient_intensity,
            ambient_color,
            csm_splits: helio_pass_shadow_matrix_splits(),
            debug_mode: self.debug_mode,
            screen_width: ctx.width as f32,
            screen_height: ctx.height as f32,
            time: ctx.time,
        };
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(cull_inputs) = ctx
            .registry
            .get::<VgCullInputs<'_>>(ResourceKey::new(KEY_INPUTS))
        else {
            return Ok(());
        };
        let Some(cull) = ctx
            .registry
            .get::<VgCullFrameData<'_>>(ResourceKey::new(KEY_CULL))
        else {
            return Ok(());
        };
        let Some(raster) = ctx
            .registry
            .get::<VgRasterFrameData<'_>>(ResourceKey::new(KEY_RASTER))
        else {
            return Ok(());
        };
        let dense = ctx
            .registry
            .get::<VgDenseFrameData<'_>>(ResourceKey::new(KEY_DENSE));
        let Some(material_textures) = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
        else {
            return Ok(());
        };
        let Some(materials) = ctx.scene_buffers.get(BufferKey::of("materials")) else {
            return Ok(());
        };
        let coordinate_spaces = ctx
            .registry
            .get::<helio_core::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            );
        let (spaces, spaces_prev) = coordinate_spaces
            .map(|c| (c.coordinate_spaces, c.coordinate_spaces_prev))
            .unwrap_or((ctx.camera, ctx.camera));
        let Some(gbuffer) = ctx
            .registry
            .get::<ViewGroup<'_, 4>>(ResourceKey::new("gbuffer"))
        else {
            return Ok(());
        };
        let reg = ctx.registry;
        let (Some(lightmap_uv), Some(sss), Some(extra), Some(velocity)) = (
            reg.texture_view(ResourceKey::new("gbuffer_lightmap_uv")),
            reg.texture_view(ResourceKey::new("gbuffer_sss")),
            reg.texture_view(ResourceKey::new("gbuffer_extra")),
            reg.texture_view(ResourceKey::new("gbuffer_velocity")),
        ) else {
            return Ok(());
        };

        let inputs = VgShadeInputs {
            camera: ctx.camera,
            static_objects: cull_inputs.objects,
            coordinate_spaces: spaces,
            coordinate_spaces_prev: spaces_prev,
            materials: &materials.buffer,
            material_textures,
            visible: cull.visible_clusters,
            asset_quant: cull_inputs.stream.asset_quant,
            instances: cull_inputs.instances,
            pages: cull_inputs.stream.page_pool,
            hw_vis: raster.vis,
            hw_depth: raster.depth,
            scene_depth: ctx.depth,
            sw_keys: Some(raster.sw_keys),
            dense_keys: dense.map(|d| d.dense_key),
            rt: None,
            rt_visible: dense.and_then(|d| d.rt_visible),
            dense_mask: dense.map(|d| d.tile_mask),
            dense_merge: dense.is_some_and(|d| d.merge),
            // `from_active_id`, not `from_id`: the renderer-wide "Ray Trace vs Raster"
            // mode is registered by the lighting pass, but the pixels of virtual
            // geometry in it can only be painted here (they know their source).
            debug: VgDebugView::from_active_id(self.debug_mode).map(|view| VgShadeDebug {
                view,
                stream: cull_inputs.stream,
                cull_state: cull.state,
                bin_capacity: cull.bin_capacity,
                tile_counters: raster.tile_counters,
                dense_mask: raster.dense_mask,
                resolved_mask: dense.map(|d| d.tile_mask),
                tiles: raster.tiles,
                tau_px: self.tau.as_ref().map_or(1.0, |tau| tau()),
            }),
        };
        let targets = VgShadeTargets {
            albedo: gbuffer.views[0],
            normal: gbuffer.views[1],
            orm: gbuffer.views[2],
            emissive: gbuffer.views[3],
            lightmap_uv,
            sss,
            extra,
            velocity,
            depth: ctx.depth,
        };
        let mut rec = ctx.graphics_cmds();
        self.shade.encode_rec(
            ctx.device,
            ctx.queue,
            &mut rec,
            raster.width,
            raster.height,
            &inputs,
            &targets,
            &self.globals,
        );
        Ok(())
    }
}

/// The classic G-buffer's cascade splits (`helio_pass_shadow_matrix::CSM_SPLITS`).
fn helio_pass_shadow_matrix_splits() -> [f32; 4] {
    helio_pass_shadow_matrix::CSM_SPLITS
}
