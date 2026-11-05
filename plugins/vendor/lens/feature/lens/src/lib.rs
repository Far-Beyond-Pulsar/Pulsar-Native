//! Physically based screen-space lens: a focus field over the sensor, the
//! thin-lens defocus it implies, a ray map (projection, distortion, spectral
//! lateral chromatic aberration) and a transmission (vignetting, filters), in
//! scene-linear radiance. See README.md for the optical model and the
//! `lens_color` / `lens_resolved` graph contract.

pub mod feature;
pub mod scene_rows;

pub use feature::LensFeature;

use helio_core::graph::ResourceBuilder;
use helio_core::{
    CommandRecorder, PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult,
};

mod components;
pub use components::*;
mod gpu_types;
pub use gpu_types::*;
mod masks;
pub mod ray_map;
pub use masks::*;
mod prescriptions;
pub use prescriptions::*;
mod bundled;
pub use bundled::*;
pub use components::LensElementRow;
pub use components::LensRow;
pub use components::CAMERA_LENSES;
pub use components::LENS_ELEMENTS;

/// Scene-linear image after the lens, at the size of the pass's input.
pub const LENS_COLOR: &str = "lens_color";
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Optional registry entry: a `D2Array` view of [`APERTURE_SHAPE_FORMAT`] iris
/// shapes for `pupil::TEXTURE_LAYER`. Without it one white layer is bound.
pub const APERTURE_SHAPES: &str = "lens_aperture_shapes";
/// Format of aperture shape textures: coverage in red.
pub const APERTURE_SHAPE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// Optional registry entry: a `D2Array` view of [`MASK_FORMAT`] layers for
/// `region_kind::MASK` regions and the normals of water drops, see
/// [`mask_array`]. Without it one transparent black 1x1 layer is bound, so a
/// mask region covers nothing.
pub const MASKS: &str = "lens_masks";
/// Format of mask textures: four channels, normals or coverage.
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Tile edge of the circle-of-confusion extent grid; equals `TILE` in the WGSL.
const TILE: u32 = 16;
/// Most levels of the aberration stage's pyramid (the first is half the
/// image); equals `PYRAMID_LEVELS` in the WGSL.
const PYRAMID_LEVELS: u32 = 8;

/// Levels the pyramid of a `width` x `height` image has: halving to one texel,
/// at most [`PYRAMID_LEVELS`]; the WGSL computes the same.
fn pyramid_levels(width: u32, height: u32) -> u32 {
    let half = width.div_ceil(2).max(height.div_ceil(2)).max(1);
    (32 - half.leading_zeros()).min(PYRAMID_LEVELS)
}

const COMPUTE: wgpu::ShaderStages = wgpu::ShaderStages::COMPUTE;
/// Binding of the prescription bank in the stages that consult it (resolve,
/// circles of confusion, gather, composite, aberration); equals `bank` in the WGSL.
const BANK_BINDING: u32 = 30;

/// Markers around the default no-op `USER` hooks in `lens.wgsl`; a host
/// snippet replaces what lies between them.
const USER_BEGIN: &str = "// %%USER_BEGIN";
const USER_END: &str = "// %%USER_END";
/// The hooks a snippet may define, with the stand-in used for any it omits.
const USER_HOOKS: [(&str, &str); 3] = [
    (
        "user_ray_map",
        "fn user_ray_map(uv: vec2<f32>, params: array<f32, 16>, t: f32) -> vec2<f32> { return uv; }",
    ),
    (
        "user_transmission",
        "fn user_transmission(uv: vec2<f32>, params: array<f32, 16>) -> vec3<f32> { return vec3(1.0); }",
    ),
    (
        "user_focus",
        "fn user_focus(uv: vec2<f32>, params: array<f32, 16>, s: f32) -> f32 { return s; }",
    ),
];

/// `lens.wgsl` with its default `USER` hooks replaced by `snippet`, plus the
/// default of every hook the snippet leaves undefined.
fn lens_source_with_user(snippet: &str) -> String {
    let base = helio_core::shader::source_text(
        "Screen-space lens",
        helio_core::include_wgsl!("../shaders/lens.wgsl"),
    );
    let begin = base
        .find(USER_BEGIN)
        .expect("lens.wgsl lost its user hook markers");
    let end = base
        .find(USER_END)
        .expect("lens.wgsl lost its user hook markers");
    let mut hooks = String::from("const USER_ENABLED: bool = true;\n");
    hooks.push_str(snippet);
    hooks.push('\n');
    for (name, fallback) in USER_HOOKS {
        if !snippet.contains(&format!("fn {name}")) {
            hooks.push_str(fallback);
            hooks.push('\n');
        }
    }
    format!("{}{}\n{}", &base[..begin], hooks, &base[end..])
}

fn image(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// Everything sized by the input image.
struct Targets {
    size: (u32, u32),
    /// Per pixel: circle of confusion a and b, seam weight, transmission.
    coc: wgpu::TextureView,
    half_color: wgpu::TextureView,
    half_coc: wgpu::TextureView,
    far: wgpu::TextureView,
    near: wgpu::TextureView,
    output: wgpu::TextureView,
    tile_raw: wgpu::Buffer,
    tile_dilated: wgpu::Buffer,
    /// The sensor image (defocus, ray map, transmission) while an aberration
    /// element is present; `output` is then written by `cs_ab_apply`.
    composed: wgpu::TextureView,
    /// Box-filtered pyramid of `composed`: level 0 is half its size.
    pyramid: wgpu::TextureView,
    /// One single-level view per pyramid level, for the downsample.
    pyramid_levels: Vec<wgpu::TextureView>,
}

impl Targets {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let (half_w, half_h) = (width.div_ceil(2), height.div_ceil(2));
        let levels = pyramid_levels(width, height);
        let pyramid_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lens aberration pyramid"),
            size: wgpu::Extent3d {
                width: half_w,
                height: half_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let pyramid_levels = (0..levels)
            .map(|level| {
                pyramid_texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let tiles = u64::from(width.div_ceil(TILE)) * u64::from(height.div_ceil(TILE));
        let tile_buffer = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: tiles * 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        Self {
            size: (width, height),
            coc: image(device, width, height, "Lens circles of confusion"),
            half_color: image(device, half_w, half_h, "Lens half-resolution colour"),
            half_coc: image(device, half_w, half_h, "Lens half-resolution circles"),
            far: image(device, half_w, half_h, "Lens far layer"),
            near: image(device, half_w, half_h, "Lens near layer"),
            output: image(device, width, height, "Lens colour"),
            tile_raw: tile_buffer("Lens tile extents"),
            tile_dilated: tile_buffer("Lens dilated tile extents"),
            composed: image(device, width, height, "Lens composed sensor image"),
            pyramid: pyramid_texture.create_view(&Default::default()),
            pyramid_levels,
        }
    }
}

/// What [`LensPass::with_bundled_resources`] publishes, as it becomes ready.
#[derive(Default)]
struct Bundled {
    bank: Option<PrescriptionBank>,
    masks: Option<wgpu::TextureView>,
    shapes: Option<wgpu::TextureView>,
}

impl Bundled {
    fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.masks.is_none() {
            let layers = bundled_mask_layers();
            let layers: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            self.masks = Some(mask_array(device, queue, MASK_SIZE, &layers));
        }
        if self.shapes.is_none() {
            let layers = bundled_aperture_layers();
            let layers: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            self.shapes = Some(aperture_shape_array(
                device,
                queue,
                APERTURE_SHAPE_SIZE,
                &layers,
            ));
        }
        if self.bank.is_none() {
            if let Some(bakes) = bundled_bakes_ready() {
                let bakes: Vec<&helio_lens_optics::LensBake> = bakes.iter().collect();
                self.bank = Some(PrescriptionBank::new(device, &bakes));
            }
        }
    }
}

/// Scene inputs. Every field is optional: missing producers fall back to
/// neutral stand-ins (no lens row, no elements).
#[derive(Default, Clone, Copy)]
pub struct LensInputs<'a> {
    pub camera: Option<&'a wgpu::Buffer>,
    pub depth: Option<&'a wgpu::TextureView>,
    pub lenses: Option<&'a wgpu::Buffer>,
    pub elements: Option<&'a wgpu::Buffer>,
    /// Iris shape array, see [`APERTURE_SHAPES`].
    pub aperture_shapes: Option<&'a wgpu::TextureView>,
    /// Mask array, see [`MASKS`].
    pub masks: Option<&'a wgpu::TextureView>,
    /// Baked lens prescriptions, see [`PRESCRIPTIONS`] and
    /// [`PrescriptionBank::buffer`].
    pub prescriptions: Option<&'a wgpu::Buffer>,
}

type BindingKey = ([Option<wgpu::TextureView>; 4], [Option<wgpu::Buffer>; 4]);

// Stages in recording order; `args` holds one indirect triple per stage after
// the resolve. The aberration stage follows the composite: `DOWN` runs once
// per pyramid level (its triples are `ARGS_DOWN + level`), then `APPLY`.
const RESOLVE: usize = 0;
const COC: usize = 1;
const TILES: usize = 2;
const DILATE: usize = 3;
const PREFILTER: usize = 4;
const GATHER: usize = 5;
const COMPOSITE: usize = 6;
const DOWN: usize = 7;
const APPLY: usize = 8;
const STAGES: usize = 9;
/// Triples before the pyramid's (resolve has none): the six sized stages.
const ARGS_DOWN: u64 = 6;
const ARGS_APPLY: u64 = ARGS_DOWN + PYRAMID_LEVELS as u64;
const ARGS_SIZE: u64 = (ARGS_APPLY + 1) * 12;

struct Stage {
    entry: &'static str,
    label: &'static str,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn texture_array(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

/// A `D2Array` view of iris shapes for [`LensInputs::aperture_shapes`]:
/// `layers` are `size` x `size` coverage images (row-major, 0..255, the
/// square around the pupil circle, y down), one array layer each.
pub fn aperture_shape_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    layers: &[&[u8]],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let data: Vec<u8> = layers
        .iter()
        .flat_map(|layer| layer.iter().copied())
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Lens aperture shapes"),
                size: wgpu::Extent3d {
                    width: size.max(1),
                    height: size.max(1),
                    depth_or_array_layers: layers.len().max(1) as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: APERTURE_SHAPE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}

fn storage_texture(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: OUTPUT_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

/// All persistent state is derived GPU machinery: the autofocus position and
/// the resolved lens. No CPU copy of rows or settings.
pub struct LensPass {
    input_key: &'static str,
    /// The image the input may be a view of (`helio_frame_api::color::alias`).
    input_alias: Option<&'static str>,
    stages: [Stage; STAGES],
    sampler: wgpu::Sampler,
    /// Indirect dispatch triples, sized by `cs_resolve` from the lens row.
    args: wgpu::Buffer,
    /// Autofocus position, persistent between frames.
    state: wgpu::Buffer,
    /// This frame's [`ResolvedLens`], published as [`RESOLVED_KEY`].
    resolved: wgpu::Buffer,
    /// Frame time in seconds, written in prepare().
    frame: wgpu::Buffer,
    fallback_camera: wgpu::Buffer,
    fallback_lenses: wgpu::Buffer,
    fallback_elements: wgpu::Buffer,
    /// A bank of no lenses: the prescription binding when the host has none.
    fallback_prescriptions: wgpu::Buffer,
    fallback_depth: wgpu::TextureView,
    /// One white layer: the aperture array when the host supplies none.
    fallback_shapes: wgpu::Texture,
    fallback_shapes_view: wgpu::TextureView,
    fallback_shapes_ready: bool,
    /// One transparent black layer: the mask array when the host supplies none.
    fallback_masks_view: wgpu::TextureView,
    targets: Targets,
    /// The bundled bank, masks and iris shapes, published as the registry
    /// entries when the host asked for them ([`Self::with_bundled_resources`]).
    bundled: Option<Bundled>,
    bindings: Option<(BindingKey, Vec<wgpu::BindGroup>)>,
}

/// A host's WGSL for [`element_kind::USER`] elements (see
/// [`LensPass::new_with_user`]), as a feature setting: a graph's lens node
/// builds with it.
#[derive(Clone, Copy, Debug)]
pub struct LensUser(pub &'static str);

impl LensPass {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::new_with_user(device, width, height, None)
    }

    /// Like [`Self::new`], with a WGSL snippet implementing
    /// [`element_kind::USER`] elements compiled into the lens shader (as
    /// `PostProcessPass::new_with_user_effects` does for its effects). The
    /// snippet may define `user_ray_map`, `user_transmission` and `user_focus`
    /// (see [`user`]); a hook it omits is the identity. Without a snippet
    /// `USER` elements are ignored entirely: they set no stage flags and the
    /// shader is the stock one.
    pub fn new_with_user(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        user_wgsl: Option<&str>,
    ) -> Self {
        let shader = match user_wgsl {
            None => helio_core::shader::module(
                device,
                "Screen-space lens",
                helio_core::include_wgsl!("../shaders/lens.wgsl"),
            ),
            Some(snippet) => helio_core::shader::module(
                device,
                "Screen-space lens",
                lens_source_with_user(snippet).as_str(),
            ),
        };
        let stage = |entry_point: &'static str,
                     label: &'static str,
                     entries: &[wgpu::BindGroupLayoutEntry]| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                },
            );
            Stage {
                entry: entry_point,
                label,
                layout,
                pipeline,
            }
        };
        // The bindings of each stage are the globals its entry point uses
        // (lens.wgsl); one texture is never read and written in one stage.
        let stages = [
            stage(
                "cs_resolve",
                "Lens resolve",
                &[
                    storage(0, true),
                    storage(1, true),
                    storage(2, true),
                    texture(3, false),
                    texture(4, false),
                    storage(5, false),
                    storage(6, false),
                    storage(7, false),
                    uniform(8),
                    texture_array(24),
                    storage(BANK_BINDING, true),
                ],
            ),
            stage(
                "cs_coc",
                "Lens circles of confusion",
                &[
                    storage(1, true),
                    storage(2, true),
                    texture(3, false),
                    storage(6, false),
                    storage_texture(9),
                    texture_array(24),
                    storage(BANK_BINDING, true),
                ],
            ),
            stage(
                "cs_tiles",
                "Lens tile extents",
                &[texture(10, false), storage(11, false)],
            ),
            stage(
                "cs_dilate",
                "Lens tile dilation",
                &[
                    storage(6, false),
                    texture(10, false),
                    storage(11, false),
                    storage(12, false),
                ],
            ),
            stage(
                "cs_prefilter",
                "Lens half-resolution prefilter",
                &[
                    texture(4, false),
                    texture(10, false),
                    storage_texture(13),
                    storage_texture(14),
                ],
            ),
            stage(
                "cs_gather",
                "Lens gather",
                &[
                    storage(1, true),
                    storage(6, false),
                    storage(12, false),
                    texture(15, false),
                    texture(16, false),
                    storage_texture(17),
                    storage_texture(18),
                    sampler(22),
                    texture_array(23),
                    texture_array(24),
                    storage(BANK_BINDING, true),
                ],
            ),
            stage(
                "cs_composite",
                "Lens composite",
                &[
                    storage(1, true),
                    texture(4, false),
                    storage(6, false),
                    texture(10, false),
                    texture(19, true),
                    texture(20, true),
                    storage_texture(21),
                    sampler(22),
                    texture_array(24),
                    storage_texture(25),
                    storage(BANK_BINDING, true),
                ],
            ),
            stage(
                "cs_ab_down",
                "Lens aberration pyramid",
                &[texture(26, true), storage_texture(27), sampler(22)],
            ),
            stage(
                "cs_ab_apply",
                "Lens aberration filters",
                &[
                    storage(1, true),
                    storage(6, false),
                    storage_texture(21),
                    sampler(22),
                    texture_array(24),
                    texture(28, true),
                    texture(29, true),
                    storage(BANK_BINDING, true),
                ],
            ),
        ];
        debug_assert_eq!(stages[RESOLVE].entry, "cs_resolve");
        debug_assert_eq!(stages[APPLY].entry, "cs_ab_apply");
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let fallback_shapes = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lens neutral aperture shape"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: APERTURE_SHAPE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            input_key: helio_frame_api::color::SCENE_COLOR,
            input_alias: None,
            stages,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lens sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            args: buffer(
                "Lens indirect dispatch",
                ARGS_SIZE,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            // Zero: no history, the first autofocus snaps.
            state: buffer("Lens state", 16, wgpu::BufferUsages::STORAGE),
            resolved: buffer(
                "Lens resolved",
                std::mem::size_of::<ResolvedLens>() as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            frame: buffer(
                "Lens frame",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            // Zero rows: no enabled lens, no elements.
            fallback_camera: buffer(
                "Lens neutral camera",
                2 * std::mem::size_of::<helio_core::GpuCameraUniforms>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_lenses: buffer(
                "Lens no rows",
                std::mem::size_of::<LensRow>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_elements: buffer(
                "Lens no elements",
                std::mem::size_of::<LensElementRow>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_prescriptions: buffer(
                "Lens no prescriptions",
                16,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_depth: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Lens neutral depth"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default()),
            fallback_shapes_view: fallback_shapes.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }),
            fallback_shapes,
            fallback_shapes_ready: false,
            fallback_masks_view: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Lens neutral mask"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: MASK_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                }),
            targets: Targets::new(device, width, height),
            bundled: None,
            bindings: None,
        }
    }

    /// Registers the bundled resources of [`crate::bundled`] as the pass's
    /// optional registry entries: the example prescriptions
    /// ([`PRESCRIPTIONS`], layers by [`BundledLens`]), a mask array with a
    /// water-drop layer ([`MASKS`]) and the iris shapes of
    /// [`BundledAperture`] ([`APERTURE_SHAPES`]). The lenses are traced on a
    /// background thread and appear once done (a `PRESCRIPTION` element covers
    /// nothing until then); the textures are made on the first frame.
    /// Configure before graph construction.
    pub fn with_bundled_resources(mut self) -> Self {
        start_baking();
        self.bundled = Some(Bundled::default());
        self
    }

    /// Select a resolved HDR texture, e.g. the TSR output. Configure before graph
    /// construction.
    pub fn with_color_input(mut self, key: &'static str) -> Self {
        self.input_key = key;
        self
    }

    /// The image the input may be a view of (`helio_frame_api::color::alias`):
    /// read too, so it stays alive, and used when the input is not published.
    pub fn with_color_alias(mut self, alias: Option<&'static str>) -> Self {
        self.input_alias = alias;
        self
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.targets.output
    }

    /// The buffer holding this frame's [`ResolvedLens`].
    pub fn resolved_buffer(&self) -> &wgpu::Buffer {
        &self.resolved
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.targets.size != (width.max(1), height.max(1)) {
            self.targets = Targets::new(device, width, height);
            self.bindings = None;
        }
    }

    fn clear(&self, cmds: &mut CommandRecorder<'_>) {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &self.targets.output,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        // Fast clear: no stale image when the input vanishes.
        let _pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Lens clear"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    fn record(
        &mut self,
        device: &wgpu::Device,
        cmds: &mut CommandRecorder<'_>,
        input: Option<&wgpu::TextureView>,
        scene: LensInputs<'_>,
    ) {
        let Some(input) = input else {
            self.clear(cmds);
            return;
        };
        self.resize(device, input.texture().width(), input.texture().height());
        if !self.fallback_shapes_ready {
            // The neutral aperture layer is plain white; a texture cannot be
            // created filled without a queue, so the first frame clears it.
            let view = self.fallback_shapes.create_view(&Default::default());
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lens neutral aperture"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            drop(pass);
            self.fallback_shapes_ready = true;
        }
        let camera = scene.camera.unwrap_or(&self.fallback_camera);
        let depth = scene.depth.unwrap_or(&self.fallback_depth);
        let shapes = scene.aperture_shapes.unwrap_or(&self.fallback_shapes_view);
        let masks = scene.masks.unwrap_or(&self.fallback_masks_view);
        let lenses = scene.lenses.unwrap_or(&self.fallback_lenses);
        let elements = scene.elements.unwrap_or(&self.fallback_elements);
        let bank = scene.prescriptions.unwrap_or(&self.fallback_prescriptions);
        // Bind groups are cached per GPU handle; comparing handles, not
        // pointers, is safe across TSR's rotating outputs and resize.
        let key: BindingKey = (
            [
                Some(input.clone()),
                Some(depth.clone()),
                Some(shapes.clone()),
                Some(masks.clone()),
            ],
            [
                Some(lenses.clone()),
                Some(elements.clone()),
                Some(camera.clone()),
                Some(bank.clone()),
            ],
        );
        if self.bindings.as_ref().map(|(k, _)| k) != Some(&key) {
            let t = &self.targets;
            let groups = [
                (
                    RESOLVE,
                    vec![
                        entry(0, lenses.as_entire_binding()),
                        entry(1, elements.as_entire_binding()),
                        entry(2, camera.as_entire_binding()),
                        entry(3, wgpu::BindingResource::TextureView(depth)),
                        entry(4, wgpu::BindingResource::TextureView(input)),
                        entry(5, self.state.as_entire_binding()),
                        entry(6, self.resolved.as_entire_binding()),
                        entry(7, self.args.as_entire_binding()),
                        entry(8, self.frame.as_entire_binding()),
                        entry(24, wgpu::BindingResource::TextureView(masks)),
                        entry(BANK_BINDING, bank.as_entire_binding()),
                    ],
                ),
                (
                    COC,
                    vec![
                        entry(1, elements.as_entire_binding()),
                        entry(2, camera.as_entire_binding()),
                        entry(3, wgpu::BindingResource::TextureView(depth)),
                        entry(6, self.resolved.as_entire_binding()),
                        entry(9, wgpu::BindingResource::TextureView(&t.coc)),
                        entry(24, wgpu::BindingResource::TextureView(masks)),
                        entry(BANK_BINDING, bank.as_entire_binding()),
                    ],
                ),
                (
                    TILES,
                    vec![
                        entry(10, wgpu::BindingResource::TextureView(&t.coc)),
                        entry(11, t.tile_raw.as_entire_binding()),
                    ],
                ),
                (
                    DILATE,
                    vec![
                        entry(6, self.resolved.as_entire_binding()),
                        entry(10, wgpu::BindingResource::TextureView(&t.coc)),
                        entry(11, t.tile_raw.as_entire_binding()),
                        entry(12, t.tile_dilated.as_entire_binding()),
                    ],
                ),
                (
                    PREFILTER,
                    vec![
                        entry(4, wgpu::BindingResource::TextureView(input)),
                        entry(10, wgpu::BindingResource::TextureView(&t.coc)),
                        entry(13, wgpu::BindingResource::TextureView(&t.half_color)),
                        entry(14, wgpu::BindingResource::TextureView(&t.half_coc)),
                    ],
                ),
                (
                    GATHER,
                    vec![
                        entry(1, elements.as_entire_binding()),
                        entry(6, self.resolved.as_entire_binding()),
                        entry(12, t.tile_dilated.as_entire_binding()),
                        entry(15, wgpu::BindingResource::TextureView(&t.half_color)),
                        entry(16, wgpu::BindingResource::TextureView(&t.half_coc)),
                        entry(17, wgpu::BindingResource::TextureView(&t.far)),
                        entry(18, wgpu::BindingResource::TextureView(&t.near)),
                        entry(22, wgpu::BindingResource::Sampler(&self.sampler)),
                        entry(23, wgpu::BindingResource::TextureView(shapes)),
                        entry(24, wgpu::BindingResource::TextureView(masks)),
                        entry(BANK_BINDING, bank.as_entire_binding()),
                    ],
                ),
                (
                    COMPOSITE,
                    vec![
                        entry(1, elements.as_entire_binding()),
                        entry(4, wgpu::BindingResource::TextureView(input)),
                        entry(6, self.resolved.as_entire_binding()),
                        entry(10, wgpu::BindingResource::TextureView(&t.coc)),
                        entry(19, wgpu::BindingResource::TextureView(&t.far)),
                        entry(20, wgpu::BindingResource::TextureView(&t.near)),
                        entry(21, wgpu::BindingResource::TextureView(&t.output)),
                        entry(22, wgpu::BindingResource::Sampler(&self.sampler)),
                        entry(24, wgpu::BindingResource::TextureView(masks)),
                        entry(25, wgpu::BindingResource::TextureView(&t.composed)),
                        entry(BANK_BINDING, bank.as_entire_binding()),
                    ],
                ),
            ];
            // The pyramid: level 0 reads the composed image, level k level k-1.
            let down = t.pyramid_levels.iter().enumerate().map(|(level, dst)| {
                let src = if level == 0 {
                    &t.composed
                } else {
                    &t.pyramid_levels[level - 1]
                };
                (
                    DOWN,
                    vec![
                        entry(26, wgpu::BindingResource::TextureView(src)),
                        entry(27, wgpu::BindingResource::TextureView(dst)),
                        entry(22, wgpu::BindingResource::Sampler(&self.sampler)),
                    ],
                )
            });
            let apply = (
                APPLY,
                vec![
                    entry(1, elements.as_entire_binding()),
                    entry(6, self.resolved.as_entire_binding()),
                    entry(21, wgpu::BindingResource::TextureView(&t.output)),
                    entry(22, wgpu::BindingResource::Sampler(&self.sampler)),
                    entry(24, wgpu::BindingResource::TextureView(masks)),
                    entry(28, wgpu::BindingResource::TextureView(&t.composed)),
                    entry(29, wgpu::BindingResource::TextureView(&t.pyramid)),
                    entry(BANK_BINDING, bank.as_entire_binding()),
                ],
            );
            // Order: the seven main stages, the pyramid levels, then the apply.
            let groups = groups
                .into_iter()
                .chain(down)
                .chain(std::iter::once(apply))
                .map(|(stage, entries)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(self.stages[stage].label),
                        layout: &self.stages[stage].layout,
                        entries: &entries,
                    })
                })
                .collect();
            self.bindings = Some((key, groups));
        }
        let groups = &self.bindings.as_ref().unwrap().1;
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(self.stages[RESOLVE].label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.stages[RESOLVE].pipeline);
            pass.set_bind_group(0, &groups[RESOLVE], &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Every later dispatch is GPU-sized: zero groups while the view has no
        // enabled lens, except the composite, which copies the image through.
        for (stage, group) in groups.iter().enumerate().take(COMPOSITE + 1).skip(COC) {
            self.dispatch(cmds, stage, group, stage as u64 - 1);
        }
        // The aberration stage: zero groups unless an element asks for it.
        let levels = self.targets.pyramid_levels.len();
        for level in 0..levels {
            self.dispatch(cmds, DOWN, &groups[DOWN + level], ARGS_DOWN + level as u64);
        }
        self.dispatch(cmds, APPLY, &groups[DOWN + levels], ARGS_APPLY);
    }

    /// One GPU-sized dispatch: the workgroup counts are the `slot`th triple of
    /// the indirect arguments.
    fn dispatch(
        &self,
        cmds: &mut CommandRecorder<'_>,
        stage: usize,
        group: &wgpu::BindGroup,
        slot: u64,
    ) {
        let spec = &self.stages[stage];
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(spec.label),
            timestamp_writes: None,
        });
        pass.set_pipeline(&spec.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups_indirect(&self.args, 12 * slot);
    }
}

impl RenderPass for LensPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["camera_lenses", "lens_elements"]
    }

    fn name(&self) -> &'static str {
        "Lens"
    }
    fn writes(&self) -> &'static [&'static str] {
        &[LENS_COLOR, RESOLVED_KEY]
    }
    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(self.input_key);
        if let Some(alias) = self.input_alias {
            builder.read(alias);
        }
        // Manually owned resources: these declarations track the dependency,
        // while publish routes the actual views without a second texture.
        builder.write_buffer(LENS_COLOR);
        builder.write_buffer(RESOLVED_KEY);
    }
    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let frame = [ctx.delta_time.max(0.0), 0.0, 0.0, 0.0];
        ctx.queue
            .write_buffer(&self.frame, 0, bytemuck::cast_slice(&frame));
        if let Some(bundled) = &mut self.bundled {
            bundled.update(ctx.device, ctx.queue);
        }
        Ok(())
    }
    fn publish_frame_inputs<'a>(
        &self,
        _camera: &helio_core::GpuCameraUniforms,
        frame: &mut helio_core::ResourceRegistry<'a>,
    ) {
        let Some(bundled) = &self.bundled else { return };
        // SAFETY: the resources are owned by this pass, which the graph keeps
        // alive for longer than the per-frame registry (as `publish` does for
        // `lens_resolved`).
        if let Some(bank) = &bundled.bank {
            let buffer: &'a wgpu::Buffer = unsafe { std::mem::transmute(bank.buffer()) };
            frame.write(ResourceKey::new(PRESCRIPTIONS), buffer, self.name());
        }
        if let Some(masks) = &bundled.masks {
            let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(masks) };
            frame.write(ResourceKey::new(MASKS), view, self.name());
        }
        if let Some(shapes) = &bundled.shapes {
            let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(shapes) };
            frame.write(ResourceKey::new(APERTURE_SHAPES), view, self.name());
        }
    }
    fn on_resize(&mut self, device: &wgpu::Device, size: helio_core::RenderSize) {
        let [width, height] = size.internal;
        self.resize(device, width, height);
    }
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        frame.route_named_texture(LENS_COLOR, &self.targets.output, self.name());
        // SAFETY: the buffer is owned by this pass, which the graph keeps
        // alive for longer than the per-frame registry (as PostProcess does
        // for `postprocess_uniforms`).
        let resolved: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.resolved) };
        frame.write(ResourceKey::new(RESOLVED_KEY), resolved, self.name());
    }
    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let input = ctx
            .registry
            .get::<&wgpu::TextureView>(ResourceKey::new(self.input_key))
            .or_else(|| {
                self.input_alias.and_then(|alias| {
                    ctx.registry
                        .get::<&wgpu::TextureView>(ResourceKey::new(alias))
                })
            });
        let scene = LensInputs {
            camera: Some(ctx.camera),
            depth: Some(ctx.depth),
            aperture_shapes: ctx
                .registry
                .get::<&wgpu::TextureView>(ResourceKey::new(APERTURE_SHAPES)),
            masks: ctx
                .registry
                .get::<&wgpu::TextureView>(ResourceKey::new(MASKS)),
            prescriptions: ctx
                .registry
                .get::<&wgpu::Buffer>(ResourceKey::new(PRESCRIPTIONS)),
            lenses: ctx
                .scene_buffers
                .get(helio_core::BufferKey::of("camera_lenses"))
                .map(|handle| &handle.buffer),
            elements: ctx
                .scene_buffers
                .get(helio_core::BufferKey::of("lens_elements"))
                .map(|handle| &handle.buffer),
        };
        // Must follow fog/TSR on the graphics encoder. The separate compute
        // encoder is submitted BEFORE graphics and would sample stale HDR.
        self.record(ctx.device, &mut ctx.graphics_cmds(), input, scene);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
