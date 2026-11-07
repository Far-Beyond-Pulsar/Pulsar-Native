//! 2D radiance cascades — real-time global illumination for the 2D sprite
//! passes, ported from
//! <https://raw.githubusercontent.com/radiance-cascades/radiance-cascades.com/refs/heads/main/public/js/rc.js>
//! (the reference WebGL2 implementation behind radiance-cascades.com) to a
//! GPU-compute pipeline: jump-flood distance field → hierarchical
//! probe/interval cascades → a multiply-blended composite over whatever the
//! sprite passes already drew.
//!
//! Two passes, wired together like `helio-pass-shadow-cull`/
//! `helio-pass-shadow` or `helio-pass-sprite-cull`/`helio-pass-sprite-batch`
//! — no Cargo dependency between them, just a texture view handed from one
//! to the other at graph-construction time:
//!
//! - [`RadianceCascades2DPass`] (compute-only, add to the graph first): each
//!   frame, rebuilds a small "scene" texture (RGB = emissive light sources,
//!   A = occluder mask) from a caller-owned occupancy grid + emitter list,
//!   derives a jump-flood-accelerated distance field from it, then computes
//!   `cascade_count` levels of radiance (coarsest first, each merging in
//!   the next-coarser level's result), leaving the final cascade-0 result
//!   in [`RadianceCascades2DPass::radiance_view`].
//! - [`RadianceCascadesCompositePass`] (add second): draws a fullscreen
//!   triangle sampling that texture, blended over the existing render
//!   target via a multiply blend (`ambient + radiance`, `Dst * Zero` — no
//!   framebuffer read needed).
//!
//! # Algorithm notes (see `shaders/rc_cascade.wgsl` for the merge/raymarch)
//!
//! Cascade *N* has `spacing_base^N` probes per axis (`spacing_base =
//! sqrt(base_ray_count)`) each casting `base_ray_count^(N+1)` rays over an
//! interval that starts where cascade *N-1*'s interval ends (so consecutive
//! cascades tile the ray-length axis without gaps or overlaps). Every level
//! is stored at the *same* fixed texture resolution (`scene_size`)
//! regardless of its actual probe count, by tiling angularly-different "ray
//! buckets" across the texel budget a coarser level's sparser probe grid
//! frees up — this is what keeps a single fixed-size compute dispatch valid
//! for every cascade level. Merging (mixing in the next-coarser cascade's radiance wherever a
//! level's own raymarch found nothing) is what turns N independent
//! ray-interval samples into full-length rays. Each ray merges the four
//! surrounding coarser probes separately, along a ray that ends at that
//! probe's interval start, then weights them bilinearly (the "bilinear
//! fix"), so a coarser probe behind an occluder contributes nothing.
//!
//! The distance field raymarch (`raymarch()` in `rc_cascade.wgsl`) sphere-
//! traces: at each step it jumps by the distance-to-nearest-occluder rather
//! than a fixed small increment, so empty space is crossed in a handful of
//! iterations instead of hundreds.

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result};
use std::sync::Arc;

const WG_SIZE: u32 = 8;

fn dispatch_count(n: u32) -> u32 {
    n.div_ceil(WG_SIZE)
}

/// `ceil(log(sqrt(w²+h²)) / log(base_ray_count)) + 1` — the reference's
/// `radianceCascades` formula: enough cascade levels that the coarsest
/// level's ray length can reach corner-to-corner across the scene texture.
fn cascade_count_for(w: u32, h: u32, base_ray_count: f32) -> u32 {
    let angular_size = ((w * w + h * h) as f32).sqrt();
    (angular_size.ln() / base_ray_count.ln()).ceil() as u32 + 1
}

/// `ceil(log2(max(w,h))) + 1` — the reference's jump-flood pass count
/// (enough halving-offset passes for a seed to propagate corner-to-corner).
fn jfa_passes_for(w: u32, h: u32) -> u32 {
    (w.max(h) as f32).log2().ceil() as u32 + 1
}

/// Tunables for [`RadianceCascades2DPass::new`]. The scene/cascade
/// resolution is deliberately much lower than the game's render
/// resolution — global illumination reads as soft, low-frequency light
/// even at a fraction of native res, and it keeps `cascade_count` (and so
/// total compute dispatches per frame) small.
pub struct RadianceCascadesConfig {
    pub scene_width: u32,
    pub scene_height: u32,
    /// Rays-per-probe growth factor between cascade levels (`4.0` matches
    /// the reference's default and is what the merge math is tuned for —
    /// changing it changes `spacing_base = sqrt(base_ray_count)` too).
    pub base_ray_count: f32,
    /// Texel spacing between cascade-0 probes. `1.0` (a probe per texel)
    /// matches the reference's default.
    pub base_pixels_between_probes: f32,
    pub max_emitters: u32,
}

impl Default for RadianceCascadesConfig {
    fn default() -> Self {
        Self {
            scene_width: 320,
            scene_height: 180,
            base_ray_count: 4.0,
            base_pixels_between_probes: 1.0,
            max_emitters: 64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SceneUniforms {
    view_center: [f32; 2],
    view_half_extent: [f32; 2],
    occupancy_dims: [u32; 2],
    occupancy_cell_size: f32,
    emitter_count: u32,
    occupancy_origin: [f32; 2],
    scene_size: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DimsUniform {
    dims: [f32; 2],
    _pad0: f32,
    _pad1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct JfaUniform {
    offset: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CascadeUniforms {
    cascade_index: f32,
    cascade_count: f32,
    base_ray_count: f32,
    base_pixels_between_probes: f32,
    cascade_interval: f32,
    ray_interval: f32,
    _pad2: f32,
    is_top_cascade: f32,
    scene_size: [f32; 2],
    _pad0: f32,
    _pad1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompositeUniforms {
    ambient: [f32; 3],
    exposure: f32,
}

fn create_storage_texture(device: &wgpu::Device, label: &str, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry_vis(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_read_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_tex_write_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn sampled_tex_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Compute-only: scene build → jump-flood distance field → cascade levels.
/// See the module doc comment for the full pipeline and how it pairs with
/// [`RadianceCascadesCompositePass`].
pub struct RadianceCascades2DPass {
    scene_w: u32,
    scene_h: u32,
    cascade_count: u32,
    jfa_passes: u32,

    occupancy_dims: (u32, u32),
    occupancy_cell_size: f32,
    occupancy_origin: [f32; 2],

    view_center: [f32; 2],
    view_half_extent: [f32; 2],
    emitter_count: u32,
    dirty: bool,

    scene_uniform_buf: wgpu::Buffer,
    scene_pipeline: wgpu::ComputePipeline,
    scene_bind_group: wgpu::BindGroup,

    seed_pipeline: wgpu::ComputePipeline,
    seed_bind_group: wgpu::BindGroup,
    step_pipeline: wgpu::ComputePipeline,
    jfa_step_bind_groups: Vec<wgpu::BindGroup>,

    dist_pipeline: wgpu::ComputePipeline,
    dist_bind_group: wgpu::BindGroup,

    cascade_pipeline: wgpu::ComputePipeline,
    /// Indexed by cascade level (0 = finest); dispatched in reverse order.
    cascade_bind_groups: Vec<wgpu::BindGroup>,

    final_radiance_view: wgpu::TextureView,
}

impl RadianceCascades2DPass {
    /// `occupancy_buf` is a packed bitset (one bit per grid cell, LSB-first
    /// within each `u32`) — bit set means "opaque, blocks light". Caller
    /// owns and updates it (e.g. `queue.write_buffer` when a tile is mined
    /// or placed); this pass only reads it. `emitters_buf` is an array of
    /// `{pos: vec2<f32>, radius: f32, r,g,b: f32, _pad: f32}` records (32
    /// bytes each), sized for at least `config.max_emitters` — set the live
    /// count via [`Self::set_emitter_count`].
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: RadianceCascadesConfig,
        occupancy_buf: Arc<wgpu::Buffer>,
        occupancy_dims: (u32, u32),
        occupancy_cell_size: f32,
        occupancy_origin: [f32; 2],
        emitters_buf: Arc<wgpu::Buffer>,
    ) -> Self {
        let scene_w = config.scene_width;
        let scene_h = config.scene_height;
        let cascade_count = cascade_count_for(scene_w, scene_h, config.base_ray_count);
        let jfa_passes = jfa_passes_for(scene_w, scene_h);
        log::info!(
            "[helio-pass-radiance-cascades-2d] {scene_w}x{scene_h} scene, {cascade_count} cascades, {jfa_passes} JFA passes"
        );

        let scene_tex = create_storage_texture(device, "RC Scene", scene_w, scene_h);
        let scene_view = scene_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let jfa_a = create_storage_texture(device, "RC JFA A", scene_w, scene_h);
        let jfa_b = create_storage_texture(device, "RC JFA B", scene_w, scene_h);
        let jfa_a_view = jfa_a.create_view(&wgpu::TextureViewDescriptor::default());
        let jfa_b_view = jfa_b.create_view(&wgpu::TextureViewDescriptor::default());
        let dist_tex = create_storage_texture(device, "RC Distance", scene_w, scene_h);
        let dist_view = dist_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let cascade_a = create_storage_texture(device, "RC Cascade A", scene_w, scene_h);
        let cascade_b = create_storage_texture(device, "RC Cascade B", scene_w, scene_h);
        let cascade_a_view = cascade_a.create_view(&wgpu::TextureViewDescriptor::default());
        let cascade_b_view = cascade_b.create_view(&wgpu::TextureViewDescriptor::default());
        let dummy_last = create_storage_texture(device, "RC Cascade Dummy (top level)", 1, 1);
        let dummy_last_view = dummy_last.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RC Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // ── Scene build ───────────────────────────────────────────────────
        let scene_shader = helio_core::shader::module(
            device,
            "RC Scene Shader",
            helio_core::include_wgsl!("../shaders/rc_scene.wgsl"),
        );
        let scene_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC Scene BGL"),
            entries: &[
                uniform_entry(0),
                storage_read_entry(1),
                storage_read_entry(2),
                storage_tex_write_entry(3),
            ],
        });
        let scene_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC Scene PL"),
            bind_group_layouts: &[Some(&scene_bgl)],
            immediate_size: 0,
        });
        let scene_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("RC Scene Pipeline"),
                layout: Some(&scene_pl),
                module: &scene_shader,
                entry_point: Some("cs_build_scene"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let scene_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RC Scene Uniforms"),
            size: std::mem::size_of::<SceneUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RC Scene BG"),
            layout: &scene_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scene_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: occupancy_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: emitters_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&scene_view),
                },
            ],
        });

        // ── Jump-flood seed + step ──────────────────────────────────────
        let jfa_shader = helio_core::shader::module(
            device,
            "RC JFA Shader",
            helio_core::include_wgsl!("../shaders/rc_jfa.wgsl"),
        );
        let jfa_dims_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RC Dims Uniform"),
            size: std::mem::size_of::<DimsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &jfa_dims_buf,
            0,
            bytemuck::bytes_of(&DimsUniform {
                dims: [scene_w as f32, scene_h as f32],
                _pad0: 0.0,
                _pad1: 0.0,
            }),
        );

        let seed_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC JFA Seed BGL"),
            entries: &[
                uniform_entry(0),
                sampled_tex_entry(1, wgpu::ShaderStages::COMPUTE),
                storage_tex_write_entry(2),
            ],
        });
        let seed_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC JFA Seed PL"),
            bind_group_layouts: &[Some(&seed_bgl)],
            immediate_size: 0,
        });
        let seed_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("RC JFA Seed Pipeline"),
                layout: Some(&seed_pl),
                module: &jfa_shader,
                entry_point: Some("cs_jfa_seed"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        // Seed always writes into JFA texture A.
        let seed_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RC JFA Seed BG"),
            layout: &seed_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: jfa_dims_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&scene_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&jfa_a_view),
                },
            ],
        });

        let step_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC JFA Step BGL"),
            entries: &[
                uniform_entry(0),
                uniform_entry(1),
                sampled_tex_entry(2, wgpu::ShaderStages::COMPUTE),
                storage_tex_write_entry(3),
            ],
        });
        let step_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC JFA Step PL"),
            bind_group_layouts: &[Some(&step_bgl)],
            immediate_size: 0,
        });
        let step_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("RC JFA Step Pipeline"),
                layout: Some(&step_pl),
                module: &jfa_shader,
                entry_point: Some("cs_jfa_step"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        // Seed wrote A; step 0 reads A writes B, step 1 reads B writes A, ...
        let mut jfa_step_bind_groups = Vec::with_capacity(jfa_passes as usize);
        for i in 0..jfa_passes {
            let offset = 2f32.powi(jfa_passes as i32 - i as i32 - 1);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RC JFA Pass Uniform"),
                size: std::mem::size_of::<JfaUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(
                &buf,
                0,
                bytemuck::bytes_of(&JfaUniform {
                    offset,
                    _pad0: 0.0,
                    _pad1: 0.0,
                    _pad2: 0.0,
                }),
            );
            let (input_view, output_view) = if i % 2 == 0 {
                (&jfa_a_view, &jfa_b_view)
            } else {
                (&jfa_b_view, &jfa_a_view)
            };
            jfa_step_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RC JFA Step BG"),
                layout: &step_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: jfa_dims_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(output_view),
                    },
                ],
            }));
        }
        // After `jfa_passes` alternating writes starting from "seed wrote A,
        // pass 0 writes B", the final write lands in B if `jfa_passes` is
        // odd, A if even.
        let jfa_final_view = if jfa_passes % 2 == 0 {
            &jfa_a_view
        } else {
            &jfa_b_view
        };

        // ── Distance field ──────────────────────────────────────────────
        let dist_shader = helio_core::shader::module(
            device,
            "RC Distance Shader",
            helio_core::include_wgsl!("../shaders/rc_distance.wgsl"),
        );
        let dist_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC Distance BGL"),
            entries: &[
                uniform_entry(0),
                sampled_tex_entry(1, wgpu::ShaderStages::COMPUTE),
                storage_tex_write_entry(2),
            ],
        });
        let dist_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC Distance PL"),
            bind_group_layouts: &[Some(&dist_bgl)],
            immediate_size: 0,
        });
        let dist_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("RC Distance Pipeline"),
                layout: Some(&dist_pl),
                module: &dist_shader,
                entry_point: Some("cs_distance_field"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let dist_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RC Distance BG"),
            layout: &dist_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: jfa_dims_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(jfa_final_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dist_view),
                },
            ],
        });

        // ── Cascades ────────────────────────────────────────────────────
        let cascade_shader = helio_core::shader::module(
            device,
            "RC Cascade Shader",
            helio_core::include_wgsl!("../shaders/rc_cascade.wgsl"),
        );
        let cascade_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC Cascade BGL"),
            entries: &[
                uniform_entry(0),
                sampled_tex_entry(1, wgpu::ShaderStages::COMPUTE),
                sampled_tex_entry(2, wgpu::ShaderStages::COMPUTE),
                sampled_tex_entry(3, wgpu::ShaderStages::COMPUTE),
                sampler_entry(4, wgpu::ShaderStages::COMPUTE),
                storage_tex_write_entry(5),
            ],
        });
        let cascade_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC Cascade PL"),
            bind_group_layouts: &[Some(&cascade_bgl)],
            immediate_size: 0,
        });
        let cascade_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("RC Cascade Pipeline"),
                layout: Some(&cascade_pl),
                module: &cascade_shader,
                entry_point: Some("cs_cascade"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        // Dispatched i = cascade_count-1 downto 0; k = that loop's 0-based
        // iteration count. Output ping-pongs A/B by k's parity; `last_tex`
        // (the next-coarser level, i.e. the PREVIOUS iteration's output) is
        // therefore always the *other* texture from this iteration's own
        // output, except the very first iteration (the top cascade), which
        // has no coarser level to merge and reads an unused 1x1 dummy.
        let mut cascade_bind_groups: Vec<Option<wgpu::BindGroup>> =
            (0..cascade_count).map(|_| None).collect();
        for k in 0..cascade_count {
            let i = cascade_count - 1 - k;
            let is_top = i == cascade_count - 1;
            let out_is_a = k % 2 == 0;
            let out_view = if out_is_a {
                &cascade_a_view
            } else {
                &cascade_b_view
            };
            let last_view = if is_top {
                &dummy_last_view
            } else if out_is_a {
                &cascade_b_view
            } else {
                &cascade_a_view
            };

            let cu = CascadeUniforms {
                cascade_index: i as f32,
                cascade_count: cascade_count as f32,
                base_ray_count: config.base_ray_count,
                base_pixels_between_probes: config.base_pixels_between_probes,
                cascade_interval: 1.0,
                ray_interval: 1.0,
                _pad2: 0.0,
                is_top_cascade: if is_top { 1.0 } else { 0.0 },
                scene_size: [scene_w as f32, scene_h as f32],
                _pad0: 0.0,
                _pad1: 0.0,
            };
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RC Cascade Pass Uniform"),
                size: std::mem::size_of::<CascadeUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, bytemuck::bytes_of(&cu));

            cascade_bind_groups[i as usize] =
                Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("RC Cascade BG"),
                    layout: &cascade_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&scene_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&dist_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(last_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(out_view),
                        },
                    ],
                }));
        }
        let cascade_bind_groups: Vec<wgpu::BindGroup> = cascade_bind_groups
            .into_iter()
            .map(|o| o.unwrap())
            .collect();

        // Final iteration is k = cascade_count-1 (i = 0); its output parity
        // tells us which physical texture holds the finished cascade-0
        // result.
        let cascade_final_is_a = (cascade_count - 1) % 2 == 0;
        let final_radiance_view = if cascade_final_is_a {
            cascade_a_view
        } else {
            cascade_b_view
        };

        Self {
            scene_w,
            scene_h,
            cascade_count,
            jfa_passes,
            occupancy_dims,
            occupancy_cell_size,
            occupancy_origin,
            view_center: [0.0, 0.0],
            view_half_extent: [scene_w as f32 * 0.5, scene_h as f32 * 0.5],
            emitter_count: 0,
            dirty: true,
            scene_uniform_buf,
            scene_pipeline,
            scene_bind_group,
            seed_pipeline,
            seed_bind_group,
            step_pipeline,
            jfa_step_bind_groups,
            dist_pipeline,
            dist_bind_group,
            cascade_pipeline,
            cascade_bind_groups,
            final_radiance_view,
        }
    }

    /// World-space region the scene texture covers this frame — pass the
    /// same view the paired sprite/cull passes use, so lighting and what's
    /// on screen agree.
    pub fn set_view(&mut self, center: [f32; 2], half_extent: [f32; 2]) {
        self.view_center = center;
        self.view_half_extent = half_extent;
        self.dirty = true;
    }

    /// How many of the caller's `emitters_buf` records are live this frame.
    pub fn set_emitter_count(&mut self, count: u32) {
        self.emitter_count = count;
        self.dirty = true;
    }

    /// The finished cascade-0 radiance texture, for
    /// [`RadianceCascadesCompositePass::new`].
    pub fn radiance_view(&self) -> &wgpu::TextureView {
        &self.final_radiance_view
    }
}

impl RenderPass for RadianceCascades2DPass {
    fn name(&self) -> &'static str {
        "RadianceCascades2D"
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None // compute-only pass
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> Result<()> {
        if self.dirty {
            self.dirty = false;
            let u = SceneUniforms {
                view_center: self.view_center,
                view_half_extent: self.view_half_extent,
                occupancy_dims: [self.occupancy_dims.0, self.occupancy_dims.1],
                occupancy_cell_size: self.occupancy_cell_size,
                emitter_count: self.emitter_count,
                occupancy_origin: self.occupancy_origin,
                scene_size: [self.scene_w as f32, self.scene_h as f32],
            };
            ctx.write_buffer(&self.scene_uniform_buf, 0, bytemuck::bytes_of(&u));
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        let mut encoder = ctx.graphics_cmds();
        let wg_x = dispatch_count(self.scene_w);
        let wg_y = dispatch_count(self.scene_h);

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RC Scene Build"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scene_pipeline);
            pass.set_bind_group(0, &self.scene_bind_group, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RC JFA Seed"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.seed_pipeline);
            pass.set_bind_group(0, &self.seed_bind_group, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        for bg in &self.jfa_step_bind_groups {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RC JFA Step"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.step_pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RC Distance Field"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.dist_pipeline);
            pass.set_bind_group(0, &self.dist_bind_group, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        for i in (0..self.cascade_count).rev() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RC Cascade"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cascade_pipeline);
            pass.set_bind_group(0, &self.cascade_bind_groups[i as usize], &[]);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }

        Ok(())
    }
}

/// Render pass: composites [`RadianceCascades2DPass`]'s finished radiance
/// texture over whatever's already in the target via a multiply blend.
pub struct RadianceCascadesCompositePass {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
}

impl RadianceCascadesCompositePass {
    /// `radiance_view` is [`RadianceCascades2DPass::radiance_view`].
    /// `ambient` is the light floor applied everywhere (so unlit areas
    /// aren't pure black); `exposure` scales the computed radiance before
    /// it's added on top.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        radiance_view: &wgpu::TextureView,
        ambient: [f32; 3],
        exposure: f32,
    ) -> Self {
        let shader = helio_core::shader::module(
            device,
            "RC Composite Shader",
            helio_core::include_wgsl!("../shaders/rc_composite.wgsl"),
        );
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RC Composite BGL"),
            entries: &[
                uniform_entry_vis(0, wgpu::ShaderStages::FRAGMENT),
                sampled_tex_entry(1, wgpu::ShaderStages::FRAGMENT),
                sampler_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RC Composite Uniforms"),
            size: std::mem::size_of::<CompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform_buf,
            0,
            bytemuck::bytes_of(&CompositeUniforms { ambient, exposure }),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RC Composite Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RC Composite BG"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(radiance_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RC Composite PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("RC Composite Pipeline"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        // Multiply blend against the existing target contents —
                        // no framebuffer texture read needed.
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Dst,
                                dst_factor: wgpu::BlendFactor::Zero,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::Zero,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        Self {
            pipeline,
            bind_group,
        }
    }
}

impl RenderPass for RadianceCascadesCompositePass {
    fn name(&self) -> &'static str {
        "RadianceCascadesComposite"
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("RC Composite Pass"),
            color_attachments: attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn prepare(&mut self, _ctx: &PrepareContext) -> Result<()> {
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        let Some(mut rp) = ctx.render_cmds() else {
            return Ok(());
        };
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group, &[]);
        rp.draw(0..3, 0..1);
        Ok(())
    }
}
