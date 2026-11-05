use bytemuck::{Pod, Zeroable};
use helio_core::graph::{ResourceBuilder, ResourceSize};
use helio_core::{
    DebugViewDescriptor, DebugViewLegend, DebugViewLegendStop, PassContext, PrepareContext,
    RenderPass, Result as HelioResult,
};
use helio_frame_api::lighting::sky::{
    ATMOSPHERE_FRAME, ATMOSPHERE_FRAME_BYTES, ATMOSPHERE_SKY_TABLE, ATMOSPHERE_SKY_TABLE_BYTES,
    ATMOSPHERE_SKY_TABLE_SNIPPET, ATMOSPHERE_SNIPPET,
};
use helio_frame_api::lighting::{
    DIRECTIONAL_VISIBILITY, ENVIRONMENT_CUBES, ENVIRONMENT_CUBES_SAMPLER, ENVIRONMENT_PROBES,
    INDIRECT_DIFFUSE, INDIRECT_SPECULAR_LAYERS, SUN_VISIBILITY, TRANSMISSION, VIEW_REMAPS,
    VIEW_REMAPS_BYTES,
};
use helio_light_api::LIGHT_LIST;
use pulsar_scenedb::gpu::BufferKey;

mod components;
pub mod feature;
pub mod gpu_types;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub use components::REFLECTION_CAPTURES;
pub use components::{ReflectionCaptureRow, MAX_REFLECTION_CAPTURES};
pub use feature::DeferredLightingFeature;
pub use gpu_types::*;

/// The lights' IES profiles (a renderer config's `features`): a texture array
/// (`R8Unorm`, `D2Array`, one layer per profile; a light's
/// `ies_profile_index` selects the layer). Without it lights sample a
/// built-in fallback. See `examples/ies_demo.rs`.
#[derive(Clone, Debug)]
pub struct IesProfiles(pub wgpu::TextureView);

/// Sampled textures visible to each deferred-light fragment entry point
/// (their pipeline layouts; a test counts them).
///
/// Base lighting uses ten G-buffer inputs (the sun's visibility included) and
/// five scene textures. Reflection composition uses six G-buffer inputs, the
/// environment cubes, two indirect-specular layers and the transmission.
pub const BASE_SAMPLED_TEXTURE_COUNT: u32 = 15;
pub const REFLECTION_SAMPLED_TEXTURE_COUNT: u32 = 10;
const _: () = assert!(BASE_SAMPLED_TEXTURE_COUNT <= 16);
const _: () = assert!(REFLECTION_SAMPLED_TEXTURE_COUNT <= 16);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DeferredGlobals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: [f32; 4],
    csm_splits: [f32; 4],
    debug_mode: u32,
    /// Number of tiles in the X dimension for tiled light culling.
    num_tiles_x: u32,
    /// Number of reflection captures in the capture storage buffer. Zero means
    /// the shader skips capture blending and falls straight through to the
    /// skylight cubemap.
    reflection_capture_count: u32,
    /// 0 on targets where `helio_core::REFLECTIONS_SUPPORTED` is false. Makes
    /// the shader skip the reflection-capture cube array along with the SSR and
    /// planar composites, so no reflection path contributes to indirect
    /// specular. Direct light and ambient are unaffected.
    enable_reflections: u32,
    /// 0 disables the environment-cubemap indirect specular term specifically.
    ///
    /// The cubemap is the *base* reflection layer — SSR and planar reflections
    /// only composite on top of it — so gating those two passes does not remove
    /// reflections from the image. This is the switch that does.
    enable_env_reflections: u32,
    /// Pads the struct to a 16-byte multiple, as WGSL requires of a uniform.
    measured_lighting_ms: f32,
    pixel_count: u32,
    _pad: u32,
}

pub struct DeferredLightPass {
    pipeline: wgpu::RenderPipeline,
    reflection_pipeline: wgpu::RenderPipeline,
    reflection_debug_pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    shadow_config_buf: wgpu::Buffer,
    bgl_0: wgpu::BindGroupLayout,
    fallback_shadow_caster_counts: wgpu::Buffer,
    /// A zeroed `AtmosphereFrame`: no atmosphere, bound until one publishes.
    fallback_atmosphere: wgpu::Buffer,
    fallback_sky_table: wgpu::Buffer,
    fallback_view_remaps: wgpu::Buffer,
    /// `(camera, shadow caster counts, atmosphere, sky table, view remaps)` `bind_group_0` was
    /// built against.
    bind_group_0_key: Option<[wgpu::Buffer; 5]>,
    bgl_1: wgpu::BindGroupLayout,
    bgl_2: wgpu::BindGroupLayout,
    bgl_3: wgpu::BindGroupLayout,
    reflection_bgl_1: wgpu::BindGroupLayout,
    reflection_bgl_2: wgpu::BindGroupLayout,
    bind_group_0: wgpu::BindGroup,
    bind_group_1: Option<wgpu::BindGroup>,
    bind_group_2: Option<wgpu::BindGroup>,
    bind_group_3: Option<wgpu::BindGroup>,
    reflection_bind_group_1: Option<wgpu::BindGroup>,
    reflection_bind_group_2: Option<wgpu::BindGroup>,
    /// Views bound in `bind_group_1`, compared by resource identity: an
    /// address key can match a new view allocated where a freed one was
    /// (a recreated voxel renderer's sun texture read stale shadows).
    bind_group_1_key: Option<Vec<wgpu::TextureView>>,
    bind_group_2_key: Option<(
        usize,
        [wgpu::TextureView; 5],
        [wgpu::Sampler; 4],
        wgpu::Buffer,
    )>,
    bind_group_3_key: Option<[wgpu::Buffer; 4]>,
    reflection_bind_group_1_key: Option<Vec<wgpu::TextureView>>,
    reflection_bind_group_2_key: Option<([wgpu::TextureView; 4], wgpu::Sampler, wgpu::Buffer)>,
    fallback_tile_lists: wgpu::Buffer,
    fallback_tile_counts: wgpu::Buffer,
    /// One word, every light seen: no terrain shadows local lights.
    fallback_local_visibility: wgpu::Buffer,
    /// One pixel's indirect diffuse (none held): nothing publishes it.
    fallback_indirect_diffuse: wgpu::Buffer,
    pre_aa_format: wgpu::TextureFormat,
    fallback_shadow_view: wgpu::TextureView,
    fallback_static_shadow_view: wgpu::TextureView,
    /// Zeroed: no translucent casters filter any light.
    fallback_transmittance_view: wgpu::TextureView,
    fallback_shadow_sampler: wgpu::Sampler,
    shadow_depth_sampler: wgpu::Sampler,
    fallback_env_view: wgpu::TextureView,
    fallback_env_sampler: wgpu::Sampler,
    /// 1×1 black Rgba16Float: nothing adds transmission.
    fallback_transmission_view: wgpu::TextureView,
    /// Bound in place of `"reflection_captures"` when no
    /// `ReflectionCaptureRow` has ever been inserted -- SceneDB is the
    /// only source this pass reads. `reflection_capture_count` is 0
    /// whenever this is bound, so it's never dereferenced.
    fallback_reflection_captures: wgpu::Buffer,
    /// 1×1 white R8Unorm fallback used when neither SSAO nor baked AO is available.
    fallback_ao_view: wgpu::TextureView,
    fallback_ao_sampler: wgpu::Sampler,
    /// 1×1 black Rgba16Float fallback used when baked lightmap is not available.
    fallback_lightmap_view: wgpu::TextureView,
    fallback_lightmap_sampler: wgpu::Sampler,
    /// 1×1 black Rg16Float fallback for lightmap UVs when not available.
    fallback_lightmap_uv_view: wgpu::TextureView,
    /// 1×1 black Rgba16Float stand-ins for the indirect-specular layers
    /// (no coverage) and the sun's visibility.
    fallback_ssr_view: wgpu::TextureView,
    fallback_planar_view: wgpu::TextureView,
    fallback_ies_view: wgpu::TextureView,
    /// The lights' IES profiles ([`IesProfiles`]); the fallback without.
    ies_profiles: Option<wgpu::TextureView>,
    ies_sampler: wgpu::Sampler,
    pub debug_mode: u32,
    /// Most recently completed GPU duration for this pass.
    measured_lighting_ms: f32,
    /// Whether the environment cubemap contributes indirect specular.
    pub enable_env_reflections: bool,
}

impl DeferredLightPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_buf: &wgpu::Buffer,
        pre_aa_format: wgpu::TextureFormat,
    ) -> Self {
        // Fallback 1-entry storage buffers used when LightCullPass is absent.
        let fallback_tile_lists = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deferred Fallback TileLists"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fallback_tile_counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deferred Fallback TileCounts"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fallback_local_visibility = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deferred Fallback Local Visibility"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        fallback_local_visibility
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped at creation")
            .copy_from_slice(&u32::MAX.to_le_bytes());
        fallback_local_visibility.unmap();
        let fallback_indirect_diffuse = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deferred Fallback Indirect Diffuse"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        // `//!use pbr_eval` pulls helio-mats' PBR evaluation library in ahead
        // of the shader, exactly what the hand-rolled concatenation used to do.
        let shader = helio_core::shader::module_with(
            device,
            "Deferred Lighting Shader",
            helio_core::include_wgsl!("../shaders/deferred_lighting.wgsl"),
            &[
                helio_mats::PBR_EVAL_SNIPPET,
                ATMOSPHERE_SNIPPET,
                ATMOSPHERE_SKY_TABLE_SNIPPET,
            ],
        );

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deferred Globals"),
            size: std::mem::size_of::<DeferredGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_config_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow Config"),
            size: std::mem::size_of::<helio_light_api::ShadowConfig>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &shadow_config_buf,
            0,
            bytemuck::bytes_of(&helio_light_api::ShadowConfig::from_quality(
                helio_light_api::ShadowQuality::Medium,
            )),
        );

        let bgl_0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredLight BGL0"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Per-atlas shadow-caster counts (see `shadow_caster_counts`).
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
                // The resolved atmosphere (see `atmosphere` in the shader).
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(ATMOSPHERE_FRAME_BYTES),
                    },
                    count: None,
                },
                // The sky over each surface (`atmosphere_sky_table`).
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(ATMOSPHERE_SKY_TABLE_BYTES),
                    },
                    count: None,
                },
                // Per remap chain: host world -> the world its pixels show
                // (`view_remaps` in the shader).
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(VIEW_REMAPS_BYTES),
                    },
                    count: None,
                },
            ],
        });
        // Bound until view remaps are published; never read then, because only
        // remapped pixels index it.
        let fallback_view_remaps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DeferredLight No View Remaps"),
            size: VIEW_REMAPS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let fallback_atmosphere = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DeferredLight No Atmosphere"),
            size: ATMOSPHERE_FRAME_BYTES,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let fallback_sky_table = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DeferredLight No Sky Table"),
            size: ATMOSPHERE_SKY_TABLE_BYTES,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        // Bound until ObjectBatch publishes its counts: claims casters in
        // both atlases, so both are sampled, as before the counts existed.
        let fallback_shadow_caster_counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DeferredLight Shadow Caster Counts Fallback"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        fallback_shadow_caster_counts
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped at creation")
            .copy_from_slice(bytemuck::cast_slice(&[1u32, 1, 1, 0]));
        fallback_shadow_caster_counts.unmap();
        let bgl_1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredLight BGL1"),
            entries: &base_group_1_layout(),
        });
        let bgl_2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredLight BGL2"),
            entries: &base_group_2_layout(),
        });

        // Group 3: tiled light culling results (tile_light_lists, tile_light_counts).
        // These are storage buffers written by LightCullPass and consumed here.
        let bgl_3 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredLight BGL3"),
            entries: &[
                storage_entry(0), // tile_light_lists
                storage_entry(1), // tile_light_counts
                storage_entry(2), // local_visibility (terrain shadows of the tile's lights)
                storage_entry(3), // indirect diffuse a pixel
            ],
        });

        // Reflection composition is a second draw in the same render pass. Its
        // deliberately narrow layouts keep each fragment stage within the
        // portable 16 sampled textures (see `BASE_SAMPLED_TEXTURE_COUNT`).
        let reflection_bgl_1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredReflection BGL1"),
            entries: &reflection_group_1_layout(),
        });
        let reflection_bgl_2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DeferredReflection BGL2"),
            entries: &reflection_group_2_layout(),
        });

        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DeferredLight BG0"),
            layout: &bgl_0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: globals_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: shadow_config_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: fallback_shadow_caster_counts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: fallback_atmosphere.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: fallback_sky_table.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: fallback_view_remaps.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("DeferredLight PL"),
            bind_group_layouts: &[Some(&bgl_0), Some(&bgl_1), Some(&bgl_2), Some(&bgl_3)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("DeferredLight Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: pre_aa_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let reflection_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("DeferredReflection PL"),
                bind_group_layouts: &[
                    Some(&bgl_0),
                    Some(&reflection_bgl_1),
                    Some(&reflection_bgl_2),
                ],
                immediate_size: 0,
            });
        let reflection_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("DeferredReflection Additive Pipeline"),
                layout: Some(&reflection_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_reflection"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: pre_aa_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let reflection_debug_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("DeferredReflection Debug Pipeline"),
                layout: Some(&reflection_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_reflection"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: pre_aa_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        let (_fallback_shadow_tex, fallback_shadow_view) = fallback_shadow_texture(device);
        let (_fallback_static_shadow_tex, fallback_static_shadow_view) =
            fallback_shadow_texture(device);
        let fallback_transmittance_view = clear_transmittance_texture(device);
        let fallback_shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Deferred Fallback Shadow Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow Depth Sampler (PCSS)"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: None, // No comparison - returns actual depth values for PCSS blocker search
            ..Default::default()
        });
        let (_fallback_env_texture, fallback_env_view) = black_cube_texture(device, queue);
        let fallback_env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Deferred Env Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let (_fallback_transmission_texture, fallback_transmission_view) =
            black_2d_texture(device, queue, "Deferred No Transmission");

        // IES fallback: 32×32, 2 layers.
        // Layer 0 = spotlight gradient (bright centre, gaussian falloff) — for IES profiles
        // Layer 1 = checkerboard grid — for gobo/cookie projection
        const IES_FALLBACK_SIZE: u32 = 32;
        let mut ies_data = Vec::with_capacity((IES_FALLBACK_SIZE * IES_FALLBACK_SIZE * 2) as usize);
        // Layer 0: gaussian spotlight
        for y in 0..IES_FALLBACK_SIZE {
            for x in 0..IES_FALLBACK_SIZE {
                let u = (x as f32 + 0.5) / IES_FALLBACK_SIZE as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / IES_FALLBACK_SIZE as f32 * 2.0 - 1.0;
                let dist = (u * u + v * v).sqrt();
                let gaussian = (-dist * dist * 4.0).exp();
                ies_data.push((gaussian.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
        // Layer 1: checkerboard gobo (alternating 4px squares)
        for y in 0..IES_FALLBACK_SIZE {
            for x in 0..IES_FALLBACK_SIZE {
                let tile = ((x / 4) + (y / 4)) & 1;
                ies_data.push(if tile == 0 { 255u8 } else { 40u8 });
            }
        }
        let fallback_ies_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("DeferredLight IES Fallback"),
            size: wgpu::Extent3d {
                width: IES_FALLBACK_SIZE,
                height: IES_FALLBACK_SIZE,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Write layer 0 (IES spotlight)
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &fallback_ies_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_data[..(IES_FALLBACK_SIZE * IES_FALLBACK_SIZE) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_FALLBACK_SIZE),
                rows_per_image: Some(IES_FALLBACK_SIZE),
            },
            wgpu::Extent3d {
                width: IES_FALLBACK_SIZE,
                height: IES_FALLBACK_SIZE,
                depth_or_array_layers: 1,
            },
        );
        // Write layer 1 (gobo checkerboard)
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &fallback_ies_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 1 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_data[(IES_FALLBACK_SIZE * IES_FALLBACK_SIZE) as usize..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_FALLBACK_SIZE),
                rows_per_image: Some(IES_FALLBACK_SIZE),
            },
            wgpu::Extent3d {
                width: IES_FALLBACK_SIZE,
                height: IES_FALLBACK_SIZE,
                depth_or_array_layers: 1,
            },
        );
        let fallback_ies_view = fallback_ies_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        let ies_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("DeferredLight IES Sampler"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let fallback_reflection_captures = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fallback Reflection Captures"),
            size: std::mem::size_of::<crate::GpuReflectionCapture>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Fallback 1×1 white R8Unorm AO texture.
        // Used when neither SSAO nor pre-baked AO is available so the shader sees
        // AO = 1.0 (fully unoccluded) rather than undefined data.
        let fallback_ao_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Deferred Fallback AO"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &fallback_ao_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8], // white = AO 1.0
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let fallback_ao_view = fallback_ao_tex.create_view(&Default::default());
        let fallback_ao_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Deferred Fallback AO Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Fallback 1×1 black Rgba16Float lightmap texture.
        // Used when baked lightmap is not available (no indirect lighting).
        let (_fallback_lightmap_tex, fallback_lightmap_view) =
            black_2d_texture(device, queue, "Deferred Fallback Lightmap");
        let fallback_lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Deferred Fallback Lightmap Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Fallback 1×1 black Rgba16Float SSR texture (no SSR available).
        let (_fallback_ssr_texture, fallback_ssr_view) =
            black_2d_texture(device, queue, "Deferred Fallback SSR");

        // Fallback 1×1 black Rgba16Float planar reflection texture.
        let (_fallback_planar_texture, fallback_planar_view) =
            black_2d_texture(device, queue, "Deferred Fallback Planar");

        // Fallback 1×1 zero Rg16Float lightmap UV texture.
        // Used when lightmap UVs are not available from GBuffer.
        let fallback_lightmap_uv_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Deferred Fallback Lightmap UV"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &fallback_lightmap_uv_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8; 4], // (0.0, 0.0) UV coords
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let fallback_lightmap_uv_view = fallback_lightmap_uv_tex.create_view(&Default::default());

        Self {
            pipeline,
            reflection_pipeline,
            reflection_debug_pipeline,
            globals_buf,
            shadow_config_buf,
            bgl_0,
            fallback_shadow_caster_counts,
            fallback_view_remaps,
            fallback_atmosphere,
            fallback_sky_table,
            bind_group_0_key: None,
            bgl_1,
            bgl_2,
            bgl_3,
            reflection_bgl_1,
            reflection_bgl_2,
            bind_group_0,
            bind_group_1: None,
            bind_group_2: None,
            bind_group_3: None,
            reflection_bind_group_1: None,
            reflection_bind_group_2: None,
            bind_group_1_key: None,
            bind_group_2_key: None,
            bind_group_3_key: None,
            reflection_bind_group_1_key: None,
            reflection_bind_group_2_key: None,
            fallback_tile_lists,
            fallback_tile_counts,
            fallback_local_visibility,
            fallback_indirect_diffuse,
            pre_aa_format,
            fallback_shadow_view,
            fallback_static_shadow_view,
            fallback_transmittance_view,
            fallback_shadow_sampler,
            shadow_depth_sampler,
            fallback_env_view,
            fallback_env_sampler,
            fallback_transmission_view,
            fallback_reflection_captures,
            fallback_ao_view,
            fallback_ao_sampler,
            fallback_lightmap_view,
            fallback_lightmap_sampler,
            fallback_lightmap_uv_view,
            fallback_ssr_view,
            fallback_planar_view,
            fallback_ies_view,
            ies_profiles: None,
            ies_sampler,
            debug_mode: 0,
            measured_lighting_ms: 0.0,
            enable_env_reflections: false,
        }
    }

    /// Set the debug visualisation mode:
    /// - 0  = normal PBR lighting
    /// - 10 = shadow factor greyscale (white=lit, black=shadowed)
    /// - 11 = raw shadow atlas depth slice 0 (unmipped, linear)
    ///
    /// Enable/disable the environment-cubemap indirect specular term.
    pub fn set_env_reflections(&mut self, enabled: bool) {
        self.enable_env_reflections = enabled;
    }

    pub fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
    }

    /// Lights with an IES profile sample `profiles`' layers.
    pub fn set_ies_profiles(&mut self, profiles: Option<wgpu::TextureView>) {
        self.ies_profiles = profiles;
    }

    /// Set shadow quality at runtime (zero CPU cost per frame, one-time buffer write).
    pub fn set_shadow_quality(
        &mut self,
        quality: helio_light_api::ShadowQuality,
        queue: &wgpu::Queue,
    ) {
        let config = helio_light_api::ShadowConfig::from_quality(quality);
        queue.write_buffer(&self.shadow_config_buf, 0, bytemuck::bytes_of(&config));
    }
}

impl RenderPass for DeferredLightPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[VIEW_REMAPS, ENVIRONMENT_PROBES, LIGHT_LIST]
    }

    fn name(&self) -> &'static str {
        "DeferredLight"
    }

    /// Its cost view (debug mode 34) shows its own measured time.
    fn set_measured_gpu_ms(&mut self, milliseconds: f32) -> bool {
        let milliseconds = milliseconds.max(0.0);
        if (self.measured_lighting_ms - milliseconds).abs() < 0.001 {
            return false;
        }
        self.measured_lighting_ms = milliseconds;
        true
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_frame_api::gbuffer::GROUP,
            helio_frame_api::gbuffer::LIGHTMAP_UV,
            helio_frame_api::gbuffer::SSS,
            helio_frame_api::gbuffer::EXTRA,
            helio_frame_api::gbuffer::DEPTH,
            helio_light_api::SHADOW_ATLAS,
            helio_light_api::STATIC_SHADOW_ATLAS,
            helio_light_api::SHADOW_SAMPLER,
            helio_light_api::SHADOW_TRANSMITTANCE,
            helio_light_api::SHADOW_CASTER_COUNTS,
            ATMOSPHERE_FRAME,
            ATMOSPHERE_SKY_TABLE,
            SUN_VISIBILITY,
            "ssao",
            helio_light_api::TILE_LIGHT_LISTS,
            helio_light_api::TILE_LIGHT_COUNTS,
            INDIRECT_DIFFUSE,
            helio_core::resource_keys::RENDER_ENVIRONMENT,
            TRANSMISSION,
            helio_frame_api::color::SCENE_COLOR,
            helio_bake_types::BAKED_LIGHTMAP,
            "baked_lightmap_sampler",
            INDIRECT_SPECULAR_LAYERS[0],
            INDIRECT_SPECULAR_LAYERS[1],
        ]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.write_color_raw(
            helio_frame_api::color::SCENE_COLOR,
            self.pre_aa_format,
            ResourceSize::MatchSurface,
        );
    }

    fn on_resize(&mut self, _device: &wgpu::Device, _size: helio_core::RenderSize) {}

    fn publish<'a>(&self, _frame: &mut helio_core::ResourceRegistry<'a>) {}

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let environment = ctx
            .registry
            .get::<helio_core::RenderEnvironment>(helio_core::resource_keys::render_environment());
        let (ambient_color, ambient_intensity) = if let Some(environment) = environment {
            (environment.ambient_color, environment.ambient_intensity)
        } else {
            ([0.5, 0.5, 0.6], 1.0) // Brighter fallback ambient: sky-blue tint
        };
        let globals = DeferredGlobals {
            frame: ctx.frame_num as u32,
            delta_time: ctx.delta_time,
            // The buffer's live row count: it grows with the scene, so any
            // constant either reads past its end or drops lights.
            light_count: ctx
                .scene_buffers
                .get(BufferKey::of(helio_light_api::LIGHT_LIST))
                .map_or(0, |lights| lights.row_capacity()),
            ambient_intensity,
            ambient_color: [ambient_color[0], ambient_color[1], ambient_color[2], 1.0],
            // Must match CSM_SPLITS constant in shadow_matrices.wgsl ([16,80,300,1400]).
            // The shadow matrices are computed for these distances, so cascade selection
            // must use the same values or shadow maps will be sampled outside their valid range.
            csm_splits: helio_light_api::CSM_SPLITS,
            debug_mode: self.debug_mode,
            num_tiles_x: ctx.width.div_ceil(16),
            // SceneDB is the only reflection-capture source: fixed capacity,
            // not a live count -- see `MAX_REFLECTION_CAPTURES`'s doc.
            reflection_capture_count: if ctx
                .scene_buffers
                .contains(BufferKey::of(helio_frame_api::lighting::ENVIRONMENT_PROBES))
            {
                MAX_REFLECTION_CAPTURES
            } else {
                0
            },
            enable_reflections: helio_core::REFLECTIONS_SUPPORTED as u32,
            enable_env_reflections: self.enable_env_reflections as u32,
            measured_lighting_ms: self.measured_lighting_ms,
            pixel_count: ctx.width.saturating_mul(ctx.height).max(1),
            _pad: 0,
        };
        ctx.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let pre_aa_view = resources.read(
            helio_core::ResourceKey::new(helio_frame_api::color::SCENE_COLOR),
            "DeferredLight",
        )?;
        // Loaded: the graph's background pass has cleared `pre_aa`, and
        // reflections may have sampled it, before lighting covers it.
        let load_op = wgpu::LoadOp::Load;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: pre_aa_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: load_op,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("DeferredLight"),
            color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let gbuffer_opt = ctx.registry.read::<helio_core::ViewGroup<'_, 4>>(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::GROUP),
            "DeferredLight",
        );
        let gbuffer = gbuffer_opt.as_ref().ok_or_else(|| {
            helio_core::Error::InvalidPassConfig(
                "DeferredLight requires published gbuffer resources".to_string(),
            )
        })?;

        // Screen-space AO: use baked AO (via frame.ssao, which SsaoPass publishes as override
        // when a baked AO texture is present) or fall back to the 1×1 white texture.
        let ao_view = ctx
            .registry
            .get(helio_core::ResourceKey::new("ssao"))
            .unwrap_or(&self.fallback_ao_view);

        // Lightmap UVs from GBuffer
        let lightmap_uv_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_frame_api::gbuffer::LIGHTMAP_UV,
            ))
            .unwrap_or(&self.fallback_lightmap_uv_view);

        // SSS/Extra data from GBuffer
        let sss_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(helio_frame_api::gbuffer::SSS))
            .unwrap_or(&self.fallback_lightmap_uv_view);
        let extra_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_frame_api::gbuffer::EXTRA,
            ))
            .unwrap_or(&self.fallback_lightmap_uv_view);
        // The sun's visibility at each pixel: occluders' and media's together
        // (`sun_visibility`), or the occluders' alone.
        let directional_visibility_view = ctx
            .registry
            .texture_binding(SUN_VISIBILITY)
            .or_else(|| ctx.registry.texture_binding(DIRECTIONAL_VISIBILITY))
            .unwrap_or(&self.fallback_ssr_view);

        let gbuffer_key: Vec<wgpu::TextureView> = [
            gbuffer.views[0],
            gbuffer.views[1],
            gbuffer.views[2],
            gbuffer.views[3],
            ctx.depth,
            ao_view,
            lightmap_uv_view,
            sss_view,
            extra_view,
            directional_visibility_view,
        ]
        .into_iter()
        .cloned()
        .collect();
        if self.bind_group_1_key.as_ref() != Some(&gbuffer_key) {
            self.bind_group_1 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("DeferredLight BG1"),
                layout: &self.bgl_1,
                entries: &[
                    texture_view_entry(0, gbuffer.views[0]),
                    texture_view_entry(1, gbuffer.views[1]),
                    texture_view_entry(2, gbuffer.views[2]),
                    texture_view_entry(3, gbuffer.views[3]),
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(ctx.depth),
                    },
                    // Screen-space AO (binding 5): SSAO or pre-baked AO from SsaoPass.publish()
                    texture_view_entry(5, ao_view),
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.fallback_ao_sampler),
                    },
                    // Lightmap UVs from GBuffer (binding 7)
                    texture_view_entry(7, lightmap_uv_view),
                    // SSS data (binding 8)
                    texture_view_entry(8, sss_view),
                    // Extra surface data (binding 9)
                    texture_view_entry(9, extra_view),
                    texture_view_entry(10, directional_visibility_view),
                ],
            }));
            self.bind_group_1_key = Some(gbuffer_key);
        }

        let reflection_gbuffer_key: Vec<wgpu::TextureView> = [
            gbuffer.views[1],
            gbuffer.views[2],
            gbuffer.views[3],
            ctx.depth,
            ao_view,
            lightmap_uv_view,
        ]
        .into_iter()
        .cloned()
        .collect();
        if self.reflection_bind_group_1_key.as_ref() != Some(&reflection_gbuffer_key) {
            self.reflection_bind_group_1 =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("DeferredReflection BG1"),
                    layout: &self.reflection_bgl_1,
                    entries: &[
                        texture_view_entry(1, gbuffer.views[1]),
                        texture_view_entry(2, gbuffer.views[2]),
                        texture_view_entry(3, gbuffer.views[3]),
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(ctx.depth),
                        },
                        texture_view_entry(5, ao_view),
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::Sampler(&self.fallback_ao_sampler),
                        },
                        texture_view_entry(7, lightmap_uv_view),
                    ],
                }));
            self.reflection_bind_group_1_key = Some(reflection_gbuffer_key);
        }

        let shadow_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(helio_light_api::SHADOW_ATLAS))
            .unwrap_or(&self.fallback_shadow_view);
        let static_shadow_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_light_api::STATIC_SHADOW_ATLAS,
            ))
            .unwrap_or(&self.fallback_static_shadow_view);
        let transmittance_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_light_api::SHADOW_TRANSMITTANCE,
            ))
            .unwrap_or(&self.fallback_transmittance_view);
        let shadow_sampler = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_light_api::SHADOW_SAMPLER,
            ))
            .unwrap_or(&self.fallback_shadow_sampler);
        // Baked reflection cube array from the probe bake. Falls back to a 1×1
        // black cube array when nothing has been baked, which reads as "no
        // environment" rather than failing to bind.
        let env_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(ENVIRONMENT_CUBES))
            .unwrap_or(&self.fallback_env_view);
        let env_sampler = ctx
            .registry
            .get(helio_core::ResourceKey::new(ENVIRONMENT_CUBES_SAMPLER))
            .unwrap_or(&self.fallback_env_sampler);

        // Baked lightmap atlas from bake inject pass
        let lightmap_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_bake_types::BAKED_LIGHTMAP,
            ))
            .unwrap_or(&self.fallback_lightmap_view);
        let lightmap_sampler = ctx
            .registry
            .get(helio_core::ResourceKey::new("baked_lightmap_sampler"))
            .unwrap_or(&self.fallback_lightmap_sampler);

        // Radiance media add (the frame's `transmission`).
        let transmission_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(TRANSMISSION))
            .unwrap_or(&self.fallback_transmission_view);
        // SceneDB is the only reflection-capture source, resolved fresh by
        // key every frame -- see `ReflectionCaptureRow`'s doc.
        let reflection_captures_buf = ctx
            .scene_buffers
            .get(BufferKey::of(ENVIRONMENT_PROBES))
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_reflection_captures);
        let ies_view = self
            .ies_profiles
            .as_ref()
            .unwrap_or(&self.fallback_ies_view);

        // The frame's indirect-specular layers, in composite order.
        let ssr_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(INDIRECT_SPECULAR_LAYERS[0]))
            .unwrap_or(&self.fallback_ssr_view);
        let planar_view = ctx
            .registry
            .get(helio_core::ResourceKey::new(INDIRECT_SPECULAR_LAYERS[1]))
            .unwrap_or(&self.fallback_planar_view);

        let lights_handle = ctx.scene_buffers.get(BufferKey::of(LIGHT_LIST));
        let lights_buf = lights_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(ctx.camera);
        let shadow_matrices_buf = ctx
            .registry
            .get::<helio_light_api::ShadowMatricesFrameData<'_>>(helio_light_api::shadow_matrices())
            .map(|s| s.shadow_matrices)
            .unwrap_or(ctx.camera);
        let scene_key = (
            // SceneDB epoch, not this frame's handle address: a reallocated
            // lights buffer can land at the same address.
            lights_handle.map_or(usize::MAX, |handle| handle.epoch as usize),
            [
                shadow_view.clone(),
                static_shadow_view.clone(),
                lightmap_view.clone(),
                ies_view.clone(),
                transmittance_view.clone(),
            ],
            [
                shadow_sampler.clone(),
                self.shadow_depth_sampler.clone(),
                lightmap_sampler.clone(),
                self.ies_sampler.clone(),
            ],
            shadow_matrices_buf.clone(),
        );
        if self.bind_group_2_key.as_ref() != Some(&scene_key) {
            self.bind_group_2 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("DeferredLight BG2"),
                layout: &self.bgl_2,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: lights_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(shadow_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(shadow_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: shadow_matrices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&self.shadow_depth_sampler),
                    },
                    // Static shadow atlas (binding 11) — cached, only changes with Static topology
                    texture_view_entry(11, static_shadow_view),
                    // Baked lightmap atlas (binding 12)
                    texture_view_entry(12, lightmap_view),
                    // Baked lightmap sampler (binding 13)
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                    },
                    // IES textures (binding 18) — from frame resources, fallback to identity
                    wgpu::BindGroupEntry {
                        binding: 18,
                        resource: wgpu::BindingResource::TextureView(ies_view),
                    },
                    // IES sampler (binding 19)
                    wgpu::BindGroupEntry {
                        binding: 19,
                        resource: wgpu::BindingResource::Sampler(&self.ies_sampler),
                    },
                    texture_view_entry(20, transmittance_view),
                    wgpu::BindGroupEntry {
                        binding: 21,
                        resource: wgpu::BindingResource::Sampler(&self.fallback_env_sampler),
                    },
                ],
            }));
            self.bind_group_2_key = Some(scene_key);
        }

        let reflection_scene_key = (
            [
                env_view.clone(),
                ssr_view.clone(),
                planar_view.clone(),
                transmission_view.clone(),
            ],
            env_sampler.clone(),
            reflection_captures_buf.clone(),
        );
        if self.reflection_bind_group_2_key.as_ref() != Some(&reflection_scene_key) {
            self.reflection_bind_group_2 =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("DeferredReflection BG2"),
                    layout: &self.reflection_bgl_2,
                    entries: &[
                        texture_view_entry(3, env_view),
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::Sampler(env_sampler),
                        },
                        texture_view_entry(14, ssr_view),
                        wgpu::BindGroupEntry {
                            binding: 15,
                            resource: reflection_captures_buf.as_entire_binding(),
                        },
                        texture_view_entry(16, planar_view),
                        texture_view_entry(8, transmission_view),
                    ],
                }));
            self.reflection_bind_group_2_key = Some(reflection_scene_key);
        }

        // ── Bind group 3: tile light culling results ──────────────────────────
        let tile_lists = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                helio_light_api::TILE_LIGHT_LISTS,
            ))
            .unwrap_or(&self.fallback_tile_lists);
        let tile_counts = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                helio_light_api::TILE_LIGHT_COUNTS,
            ))
            .unwrap_or(&self.fallback_tile_counts);
        // The voxel terrain's shadows of each pixel's tile lights (a word a
        // pixel; none: every light seen).
        let local_visibility = ctx
            .registry
            .buffer_binding("local_visibility")
            .unwrap_or(&self.fallback_local_visibility);
        // Indirect diffuse a pixel (the frame's `indirect_diffuse`).
        let gi_irradiance = ctx
            .registry
            .buffer_binding(INDIRECT_DIFFUSE)
            .unwrap_or(&self.fallback_indirect_diffuse);
        let tile_key = [
            tile_lists.clone(),
            tile_counts.clone(),
            local_visibility.clone(),
            gi_irradiance.clone(),
        ];
        if self.bind_group_3_key.as_ref() != Some(&tile_key) {
            self.bind_group_3 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("DeferredLight BG3"),
                layout: &self.bgl_3,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: tile_lists.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: tile_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: local_visibility.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: gi_irradiance.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_3_key = Some(tile_key);
        }

        // ── Bind group 0: camera, globals, shadow config, caster counts,
        //    atmosphere ─────────────────────────────────────────────────────
        let caster_counts = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                helio_light_api::SHADOW_CASTER_COUNTS,
            ))
            .unwrap_or(&self.fallback_shadow_caster_counts);
        let atmosphere = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(ATMOSPHERE_FRAME))
            .unwrap_or(&self.fallback_atmosphere);
        let sky_table = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(ATMOSPHERE_SKY_TABLE))
            .unwrap_or(&self.fallback_sky_table);
        let view_remaps = ctx
            .scene_buffers
            .get(BufferKey::of(VIEW_REMAPS))
            .map_or_else(
                || self.fallback_view_remaps.clone(),
                |handle| handle.buffer.clone(),
            );
        let key_0 = [
            ctx.camera.clone(),
            caster_counts.clone(),
            atmosphere.clone(),
            sky_table.clone(),
            view_remaps.clone(),
        ];
        if self.bind_group_0_key.as_ref() != Some(&key_0) {
            self.bind_group_0 = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("DeferredLight BG0"),
                layout: &self.bgl_0,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.globals_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.shadow_config_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: caster_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: atmosphere.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: sky_table.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: view_remaps.as_entire_binding(),
                    },
                ],
            });
            self.bind_group_0_key = Some(key_0);
        }

        let mut rp = ctx.render_cmds().unwrap();
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group_0, &[]);
        rp.set_bind_group(1, self.bind_group_1.as_ref().unwrap(), &[]);
        rp.set_bind_group(2, self.bind_group_2.as_ref().unwrap(), &[]);
        rp.set_bind_group(3, self.bind_group_3.as_ref().unwrap(), &[]);
        rp.draw(0..3, 0..1);

        let reflection_pipeline = if matches!(self.debug_mode, 30 | 31) {
            &self.reflection_debug_pipeline
        } else {
            &self.reflection_pipeline
        };
        rp.set_pipeline(reflection_pipeline);
        rp.set_bind_group(0, &self.bind_group_0, &[]);
        rp.set_bind_group(1, self.reflection_bind_group_1.as_ref().unwrap(), &[]);
        rp.set_bind_group(2, self.reflection_bind_group_2.as_ref().unwrap(), &[]);
        rp.draw(0..3, 0..1);
        Ok(())
    }

    fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        static VIEWS: &[DebugViewDescriptor] = &[
            DebugViewDescriptor {
                name: "Albedo Only",
                debug_mode: 4,
                description: "G-buffer albedo without lighting",
            },
            DebugViewDescriptor {
                name: "World Normals",
                debug_mode: 5,
                description: "World-space normals remapped to RGB",
            },
            DebugViewDescriptor {
                name: "Roughness",
                debug_mode: 6,
                description: "Material roughness as grayscale",
            },
            DebugViewDescriptor {
                name: "Metallic",
                debug_mode: 7,
                description: "Material metallic value as grayscale",
            },
            DebugViewDescriptor {
                name: "Material AO",
                debug_mode: 8,
                description: "Per-material ambient occlusion",
            },
            DebugViewDescriptor {
                name: "Emissive",
                debug_mode: 9,
                description: "Emissive material contribution",
            },
            DebugViewDescriptor {
                name: "Shadow Heatmap",
                debug_mode: 10,
                description: "Shadow factor: white=lit, black=shadowed",
            },
            DebugViewDescriptor {
                name: "Light Depth",
                debug_mode: 11,
                description: "Light-space depth projection",
            },
            DebugViewDescriptor {
                name: "Combined AO",
                debug_mode: 12,
                description: "Material AO multiplied by screen-space AO",
            },
            DebugViewDescriptor {
                name: "Specular F0",
                debug_mode: 13,
                description: "Base specular reflectance before Fresnel",
            },
            DebugViewDescriptor {
                name: "Lightmap UV",
                debug_mode: 14,
                description:
                    "Baked lightmap UVs; gray means no lightmap, magenta means virtual geometry",
            },
            DebugViewDescriptor {
                name: "Subsurface Color",
                debug_mode: 15,
                description: "Authored subsurface scattering color",
            },
            DebugViewDescriptor {
                name: "Anisotropy",
                debug_mode: 16,
                description: "Anisotropic roughness axes and tangent rotation",
            },
            DebugViewDescriptor {
                name: "Surface Flags",
                debug_mode: 17,
                description: "Red=anisotropic, green=subsurface, blue=low specular",
            },
            DebugViewDescriptor {
                name: "Subsurface Radius",
                debug_mode: 18,
                description: "Subsurface scattering radius as grayscale",
            },
            DebugViewDescriptor {
                name: "Fresnel",
                debug_mode: 19,
                description: "Schlick specular Fresnel response at the current view angle",
            },
            DebugViewDescriptor {
                name: "Direct Lighting",
                debug_mode: 22,
                description: "Direct light contribution after shadowing",
            },
            DebugViewDescriptor {
                name: "Indirect Lighting",
                debug_mode: 23,
                description: "Diffuse indirect light: the sky ambient or GI probes, or the baked lightmap",
            },
            DebugViewDescriptor {
                name: "Lighting Only",
                debug_mode: 24,
                description:
                    "Direct plus indirect diffuse lighting without emissive or subsurface glow",
            },
            DebugViewDescriptor {
                name: "Emissive and SSS",
                debug_mode: 25,
                description: "Emissive material output plus subsurface rim transmission",
            },
            DebugViewDescriptor {
                name: "Voxel Sun Visibility",
                debug_mode: 26,
                description: "Voxel sun-visibility factor sampled by deferred lighting",
            },
            DebugViewDescriptor {
                name: "GI Blend",
                debug_mode: 27,
                description: "Red=GI probe weight, blue=baked lightmap weight",
            },
            DebugViewDescriptor {
                name: "GI Irradiance",
                debug_mode: 28,
                description: "Irradiance / pi from the GI probes at each pixel (purple: none)",
            },
            DebugViewDescriptor {
                name: "GI vs Sky Ambient",
                debug_mode: 29,
                description: "What GI changes against the sky's ambient: green brighter, red darker (full at 3 stops), blue where no probe holds the point",
            },
            DebugViewDescriptor {
                name: "Light Complexity",
                debug_mode: 50,
                description: "Scalar heat map of directional lights and local lights within their attenuation radii",
            },
            DebugViewDescriptor {
                name: "Shader Complexity",
                debug_mode: 51,
                description: "Scalar heat map of per-surface light and material shader work, without lighting contrast",
            },
            DebugViewDescriptor {
                name: "GPU-Time-Weighted Shader Cost",
                debug_mode: 52,
                description: "Estimated absolute shader time contribution per pixel in microseconds, weighted by the last measured DeferredLight GPU duration",
            },
            DebugViewDescriptor {
                name: "Ray Trace vs Raster",
                debug_mode: 33,
                description: "Per-pixel source of the visible surface, as recorded where it was produced: warm = rasterized (orange hardware, yellow compute micro-raster, red dense-tile peel), cool = ray traced (cyan voxels, blue virtual-geometry ray queries)",
            },
        ];
        VIEWS
    }

    fn debug_view_legend(&self, debug_mode: u32) -> Option<DebugViewLegend> {
        static COLORS: &[DebugViewLegendStop] = &[
            DebugViewLegendStop {
                label: "0 µs/px",
                color: [0, 0, 255],
            },
            DebugViewLegendStop {
                label: "0.01 µs/px",
                color: [0, 255, 255],
            },
            DebugViewLegendStop {
                label: "0.03 µs/px",
                color: [0, 255, 0],
            },
            DebugViewLegendStop {
                label: "0.1 µs/px",
                color: [255, 255, 0],
            },
            DebugViewLegendStop {
                label: "0.5+ µs/px",
                color: [255, 0, 0],
            },
        ];
        static LIGHTS: &[DebugViewLegendStop] = &[
            DebugViewLegendStop {
                label: "0",
                color: [0, 0, 255],
            },
            DebugViewLegendStop {
                label: "2",
                color: [0, 255, 255],
            },
            DebugViewLegendStop {
                label: "4",
                color: [0, 255, 0],
            },
            DebugViewLegendStop {
                label: "6",
                color: [255, 255, 0],
            },
            DebugViewLegendStop {
                label: "8+",
                color: [255, 0, 0],
            },
        ];
        static WORK: &[DebugViewLegendStop] = &[
            DebugViewLegendStop {
                label: "0",
                color: [0, 0, 255],
            },
            DebugViewLegendStop {
                label: "8",
                color: [0, 255, 255],
            },
            DebugViewLegendStop {
                label: "16",
                color: [0, 255, 0],
            },
            DebugViewLegendStop {
                label: "24",
                color: [255, 255, 0],
            },
            DebugViewLegendStop {
                label: "32+",
                color: [255, 0, 0],
            },
        ];

        // Colours match `deferred_lighting.wgsl` (classic, voxel) and the virtual
        // geometry debug pass (`DBG_RVR_*` in `vg_debug.wgsl`).
        static RAY_VS_RASTER: &[DebugViewLegendStop] = &[
            DebugViewLegendStop {
                label: "Raster: hardware (classic, VG tier 1)",
                color: [255, 92, 20],
            },
            DebugViewLegendStop {
                label: "Raster: compute micro-raster (VG tier 2)",
                color: [255, 217, 26],
            },
            DebugViewLegendStop {
                label: "Raster: dense-tile depth peel (VG tier 3)",
                color: [230, 26, 26],
            },
            DebugViewLegendStop {
                label: "Ray traced: voxels",
                color: [31, 199, 255],
            },
            DebugViewLegendStop {
                label: "Ray traced: VG ray query (tier 3)",
                color: [77, 102, 255],
            },
        ];

        match debug_mode {
            33 => Some(DebugViewLegend {
                title: "Ray trace vs raster",
                unit: "",
                stops: RAY_VS_RASTER,
            }),
            28 => Some(DebugViewLegend {
                title: "Light complexity",
                unit: "lights",
                stops: LIGHTS,
            }),
            29 => Some(DebugViewLegend {
                title: "Shader complexity",
                unit: "work units",
                stops: WORK,
            }),
            34 => Some(DebugViewLegend {
                title: "Estimated GPU time per pixel",
                unit: "µs / pixel",
                stops: COLORS,
            }),
            _ => None,
        }
    }
}

/// Base lighting, group 1: the G-buffer, screen AO and voxel sun visibility.
fn base_group_1_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        // Screen-space AO (SSAO result or pre-baked AO). Filterable so the
        // bilinear sampler can soften the AO at the edges of the screen.
        texture_entry(5, wgpu::TextureSampleType::Float { filterable: true }),
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // Lightmap UVs from GBuffer (binding 7, Rg16Float)
        texture_entry(7, wgpu::TextureSampleType::Float { filterable: false }),
        // SSS data: subsurface_color.rgb + subsurface_radius (Rgba16Float)
        texture_entry(8, wgpu::TextureSampleType::Float { filterable: false }),
        // Extra surface data: roughness_aniso_x, roughness_aniso_y, aniso_rotation, bitcast<f32>(flags) (Rgba16Float)
        texture_entry(9, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(10, wgpu::TextureSampleType::Float { filterable: false }),
    ]
}

/// Base lighting, group 2: lights, shadows, lightmaps, IES profiles.
fn base_group_2_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        storage_entry(0),
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        storage_entry(4),
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // Static shadow atlas (cached, rendered only when Static/Stationary topology changes)
        wgpu::BindGroupLayoutEntry {
            binding: 11,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        // Baked lightmap atlas texture
        texture_entry(12, wgpu::TextureSampleType::Float { filterable: true }),
        // Baked lightmap sampler
        wgpu::BindGroupLayoutEntry {
            binding: 13,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // IES texture array (binding 18)
        wgpu::BindGroupLayoutEntry {
            binding: 18,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        // IES sampler (binding 19)
        wgpu::BindGroupLayoutEntry {
            binding: 19,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // Coloured shadow transmittance (binding 20) + its sampler (21)
        wgpu::BindGroupLayoutEntry {
            binding: 20,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 21,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// Reflection composition, group 1: the G-buffer inputs it reads.
fn reflection_group_1_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
        texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        texture_entry(5, wgpu::TextureSampleType::Float { filterable: true }),
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        texture_entry(7, wgpu::TextureSampleType::Float { filterable: false }),
    ]
}

/// Reflection composition, group 2: environment probes, the indirect-specular
/// layers and the transmission.
fn reflection_group_2_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        texture_entry(14, wgpu::TextureSampleType::Float { filterable: false }),
        storage_entry(15),
        texture_entry(16, wgpu::TextureSampleType::Float { filterable: true }),
        // The frame's transmission (radiance media add).
        texture_entry(8, wgpu::TextureSampleType::Float { filterable: false }),
    ]
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn texture_view_entry<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn fallback_shadow_texture(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Deferred Fallback Shadow"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    (texture, view)
}

fn black_2d_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let zero = [0u8; 8];
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &zero,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// A 1×1×6 black cube *array* of a single layer, bound when no baked
/// reflection array is resident. The view dimension has to match BGL2's
/// `CubeArray` or bind group creation fails, so this cannot be a plain Cube.
fn black_cube_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Deferred Fallback Env Cube Array"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let zero = [0u8; 8 * 6];
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &zero,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::CubeArray),
        ..Default::default()
    });
    (texture, view)
}

/// 1×1 zeroed `Rgba16Float` array: the transmittance layer stores 1 - T, so
/// zero filters nothing when no shadow pass publishes a layer.
fn clear_transmittance_texture(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Deferred Fallback Transmittance"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn sampled_textures(layouts: &[Vec<wgpu::BindGroupLayoutEntry>]) -> u32 {
        layouts
            .iter()
            .flatten()
            .filter(|entry| matches!(entry.ty, wgpu::BindingType::Texture { .. }))
            .count() as u32
    }

    /// The documented counts are the layouts' counts, within the portable
    /// limit of 16 sampled textures per fragment stage.
    #[test]
    fn each_entry_point_fits_the_portable_texture_limit() {
        let base = sampled_textures(&[base_group_1_layout(), base_group_2_layout()]);
        let reflection =
            sampled_textures(&[reflection_group_1_layout(), reflection_group_2_layout()]);
        assert_eq!(base, BASE_SAMPLED_TEXTURE_COUNT);
        assert_eq!(reflection, REFLECTION_SAMPLED_TEXTURE_COUNT);
        assert!(base <= 16 && reflection <= 16);
    }
}
