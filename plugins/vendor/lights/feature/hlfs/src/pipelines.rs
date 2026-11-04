use crate::HlfsMode;

/// The text of one HLFS shader file. These are concatenated in Rust (shared
/// `common`/`lighting`/`shadows` plus a per-stage file), so each is fetched as
/// text rather than handed to the module builder whole: with hot reload on,
/// an edit to any part is picked up when the stage is next assembled.
/// Evaluates to a `&str` borrowed from a temporary, so use it inside one
/// expression (the concatenation).
macro_rules! wgsl {
    ($path:literal) => {
        &*wgsl_text!($path)
    };
}

/// [`wgsl!`] as an owned handle, for use across a block boundary.
macro_rules! wgsl_text {
    ($path:literal) => {
        helio_core::shader::source_text($path, helio_core::include_wgsl!($path))
    };
}

pub(crate) fn shader_source(stage: &str) -> String {
    shader_source_for_sampler(stage, false)
}

pub(crate) fn shader_source_for_sampler(stage: &str, presampled: bool) -> String {
    shader_source_for_transmission(stage, presampled, false)
}
fn shader_source_for_transmission(stage: &str, presampled: bool, transmission: bool) -> String {
    // Scalar visibility keeps the opaque sampler's register footprint unchanged.
    // Only the transmission pipelines carry three independent colour channels.
    let visibility = if transmission {
        r#"
        alias Visibility = vec3<f32>;
        alias VisibilityCache = array<vec3<f32>, 4>;
        fn visibility_nonzero(v: Visibility) -> bool { return any(v>vec3<f32>(0.0)); }
        fn visibility_missing(v: Visibility) -> bool { return v.x<0.0; }
        fn visibility_from_rgb(v: vec3<f32>) -> Visibility { return v; }
        "#
    } else {
        r#"
        alias Visibility = f32;
        alias VisibilityCache = vec4<f32>;
        fn visibility_nonzero(v: Visibility) -> bool { return v>0.0; }
        fn visibility_missing(v: Visibility) -> bool { return v<0.0; }
        fn visibility_from_rgb(v: vec3<f32>) -> Visibility { return v.x; }
        "#
    };
    let common = format!(
        "const USE_RAY_TRANSMISSION: bool = {transmission};\nconst USE_TILE_PRESAMPLING: bool = {presampled};\n{visibility}\n{}",
        wgsl!("../shaders/common.wgsl")
    );
    let common = common.as_str();
    match stage {
        "depth" => wgsl!("../shaders/depth_pyramid.wgsl").into(),
        "grid" => [common, wgsl!("../shaders/light_grid.wgsl")].concat(),
        "screen_space" => [
            common,
            wgsl!("../shaders/lighting.wgsl"),
            wgsl!("../shaders/shadows.wgsl"),
            wgsl!("../shaders/sample.wgsl"),
        ]
        .concat(),
        "spatial" => [
            common,
            wgsl!("../shaders/lighting.wgsl"),
            wgsl!("../shaders/spatial.wgsl"),
        ]
        .concat(),
        "temporal" => [common, wgsl!("../shaders/temporal.wgsl")].concat(),
        "composite" => [
            common,
            wgsl!("../shaders/lighting.wgsl"),
            wgsl!("../shaders/shadows.wgsl"),
            wgsl!("../shaders/composite.wgsl"),
        ]
        .concat(),
        "ray_traced" | "ray_traced_composite" => {
            // Owned handles: a `&str` borrowed from a temporary would not
            // outlive the `if` branch that created it.
            let shadow = if transmission {
                wgsl_text!("../shaders/ray_shadow.wgsl")
            } else {
                wgsl_text!("../shaders/ray_shadow_opaque.wgsl")
            };
            let tail = if stage == "ray_traced" {
                wgsl_text!("../shaders/sample.wgsl")
            } else {
                wgsl_text!("../shaders/composite.wgsl")
            };
            [
                "enable wgpu_ray_query;\n",
                common,
                wgsl!("../shaders/lighting.wgsl"),
                wgsl!("../shaders/ray_receiver.wgsl"),
                &*shadow,
                &*tail,
            ]
            .concat()
        }
        _ => panic!("unknown HLFS shader stage"),
    }
}

fn entry(
    binding: u32,
    ty: wgpu::BindingType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}
fn storage(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}
fn uniform() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}
fn texture(dimension: wgpu::TextureViewDimension, depth: bool) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: if depth {
            wgpu::TextureSampleType::Depth
        } else {
            wgpu::TextureSampleType::Float { filterable: false }
        },
        view_dimension: dimension,
        multisampled: false,
    }
}
fn uint_texture() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Uint,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}
fn storage_texture(format: wgpu::TextureFormat) -> wgpu::BindingType {
    wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format,
        view_dimension: wgpu::TextureViewDimension::D2,
    }
}
fn bgl(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}

fn compute(
    device: &wgpu::Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    entry: &str,
    layout: &wgpu::PipelineLayout,
) -> wgpu::ComputePipeline {
    helio_core::shader::create_compute_pipeline(
        &device,
        wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        },
    )
}

/// The visibility backend owns only its sampling pipelines. Grid construction,
/// reservoir bindings and denoising remain in the shared pass.
pub(crate) enum VisibilityPipelines {
    ScreenSpace {
        regular: wgpu::ComputePipeline,
        small: wgpu::ComputePipeline,
    },
    RayTraced {
        regular: wgpu::ComputePipeline,
        small: wgpu::ComputePipeline,
    },
}
impl VisibilityPipelines {
    fn new(
        device: &wgpu::Device,
        mode: HlfsMode,
        presampled: bool,
        common: &wgpu::BindGroupLayout,
        gbuffer: &wgpu::BindGroupLayout,
        reservoirs: &wgpu::BindGroupLayout,
        rt: Option<&wgpu::BindGroupLayout>,
        transmission: bool,
    ) -> Self {
        match mode {
            HlfsMode::ScreenSpace => {
                let shader = helio_core::shader::module(
                    device,
                    "HLFS ScreenSpace visibility",
                    &shader_source_for_transmission("screen_space", presampled, transmission),
                );
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("HLFS ScreenSpace visibility"),
                    bind_group_layouts: &[Some(common), Some(gbuffer), Some(reservoirs)],
                    immediate_size: 0,
                });
                Self::ScreenSpace {
                    regular: compute(
                        device,
                        "HLFS ScreenSpace visibility",
                        &shader,
                        "sample_lights",
                        &layout,
                    ),
                    small: compute(
                        device,
                        "HLFS ScreenSpace small population",
                        &shader,
                        "sample_small",
                        &layout,
                    ),
                }
            }
            HlfsMode::RayTraced => {
                let shader = helio_core::shader::module(
                    device,
                    "HLFS RayTraced visibility",
                    &shader_source_for_transmission("ray_traced", presampled, transmission),
                );
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("HLFS RayTraced visibility"),
                    bind_group_layouts: &[Some(common), Some(gbuffer), Some(reservoirs), rt],
                    immediate_size: 0,
                });
                Self::RayTraced {
                    regular: compute(
                        device,
                        "HLFS RayTraced visibility",
                        &shader,
                        "sample_lights",
                        &layout,
                    ),
                    small: compute(
                        device,
                        "HLFS RayTraced small population",
                        &shader,
                        "sample_small",
                        &layout,
                    ),
                }
            }
        }
    }

    pub fn pipeline(&self, small_population: bool) -> &wgpu::ComputePipeline {
        match self {
            Self::ScreenSpace { regular, small } | Self::RayTraced { regular, small } => {
                if small_population {
                    small
                } else {
                    regular
                }
            }
        }
    }
}

pub(crate) struct Pipelines {
    pub compact_bgl: wgpu::BindGroupLayout,
    pub common_bgl: wgpu::BindGroupLayout,
    pub gbuffer_bgl: wgpu::BindGroupLayout,
    pub depth_bgl: wgpu::BindGroupLayout,
    pub grid_bgl: wgpu::BindGroupLayout,
    pub sample_bgl: wgpu::BindGroupLayout,
    pub temporal_bgl: wgpu::BindGroupLayout,
    pub spatial_bgl: wgpu::BindGroupLayout,
    pub composite_bgl: wgpu::BindGroupLayout,
    pub compact: wgpu::ComputePipeline,
    /// Builds the live light row list (`compact_lights.wgsl` `list_live`).
    pub list_live: wgpu::ComputePipeline,
    pub depth_reduce: wgpu::ComputePipeline,
    pub coarse: wgpu::ComputePipeline,
    pub select_key: wgpu::ComputePipeline,
    pub fine: wgpu::ComputePipeline,
    pub visibility: VisibilityPipelines,
    pub transmission_visibility: Option<VisibilityPipelines>,
    pub temporal: wgpu::ComputePipeline,
    pub spatial: wgpu::ComputePipeline,
    pub composite: wgpu::RenderPipeline,
    pub transmission_composite: Option<wgpu::RenderPipeline>,
    pub rt_bgl: Option<wgpu::BindGroupLayout>,
    output_format: wgpu::TextureFormat,
}
impl Pipelines {
    pub fn set_mode(&mut self, device: &wgpu::Device, mode: HlfsMode, presampled: bool) {
        self.visibility = VisibilityPipelines::new(
            device,
            mode,
            presampled,
            &self.common_bgl,
            &self.gbuffer_bgl,
            &self.sample_bgl,
            self.rt_bgl.as_ref(),
            false,
        );
        self.composite = composite_pipeline(
            device,
            self.output_format,
            mode,
            presampled,
            &self.common_bgl,
            &self.gbuffer_bgl,
            &self.composite_bgl,
            self.rt_bgl.as_ref(),
            false,
        );
        self.transmission_visibility = Some(VisibilityPipelines::new(
            device,
            mode,
            presampled,
            &self.common_bgl,
            &self.gbuffer_bgl,
            &self.sample_bgl,
            self.rt_bgl.as_ref(),
            true,
        ));
        self.transmission_composite = Some(composite_pipeline(
            device,
            self.output_format,
            mode,
            presampled,
            &self.common_bgl,
            &self.gbuffer_bgl,
            &self.composite_bgl,
            self.rt_bgl.as_ref(),
            true,
        ));
    }

    pub fn new(
        device: &wgpu::Device,
        output_format: wgpu::TextureFormat,
        mode: HlfsMode,
        presampled: bool,
    ) -> Self {
        use wgpu::{ShaderStages as S, TextureFormat as F, TextureViewDimension as D};
        let all = S::COMPUTE | S::FRAGMENT;
        let compact_bgl = bgl(
            device,
            "HLFS light compaction layout",
            &[
                entry(0, storage(true), S::COMPUTE),
                entry(1, storage(false), S::COMPUTE),
                // Live row list and the globals it reads the row count from.
                entry(2, storage(false), S::COMPUTE),
                entry(3, uniform(), S::COMPUTE),
            ],
        );
        let compact_shader = helio_core::shader::module(
            device,
            "HLFS light compaction",
            helio_core::include_wgsl!("../shaders/compact_lights.wgsl"),
        );
        let compact_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HLFS light compaction"),
            bind_group_layouts: &[Some(&compact_bgl)],
            immediate_size: 0,
        });
        let compact = compute(
            device,
            "HLFS light compaction",
            &compact_shader,
            "compact",
            &compact_layout,
        );
        let list_live = compute(
            device,
            "HLFS live light list",
            &compact_shader,
            "list_live",
            &compact_layout,
        );
        let rt_bgl = device
            .features()
            .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
            .then(|| {
                bgl(
                    device,
                    "HLFS ray-query layout",
                    &[
                        entry(
                            0,
                            wgpu::BindingType::AccelerationStructure {
                                vertex_return: false,
                            },
                            all,
                        ),
                        entry(1, storage(true), all),
                    ],
                )
            });
        let common_bgl = bgl(
            device,
            "HLFS common layout",
            &[
                entry(0, uniform(), all),
                entry(1, storage(true), all),
                entry(2, storage(true), all),
                entry(3, uniform(), all),
                entry(4, texture(D::D2Array, true), all),
                entry(
                    5,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    all,
                ),
                entry(6, storage(true), all),
                entry(7, texture(D::D2Array, false), all),
                // Static-caster atlas: lighting takes the min with the dynamic one.
                entry(8, texture(D::D2Array, true), all),
                // Coloured glass transmittance (1 - T), and its filtering sampler.
                entry(
                    9,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: D::D2Array,
                        multisampled: false,
                    },
                    all,
                ),
                entry(
                    10,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    all,
                ),
                // Live light rows (Pulsar-Native#838), see common.wgsl.
                entry(11, storage(true), all),
            ],
        );
        let mut gbuffer_entries: Vec<_> = (0..10)
            .map(|i| entry(i, texture(D::D2, i == 4), all))
            .collect();
        gbuffer_entries[6].ty = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: D::D2,
            multisampled: false,
        };
        gbuffer_entries[7].ty = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        // The GI probes' irradiance at each pixel (`gi_irradiance`).
        gbuffer_entries.push(entry(10, storage(true), all));
        let gbuffer_bgl = bgl(device, "HLFS GBuffer layout", &gbuffer_entries);
        let depth_bgl = bgl(
            device,
            "HLFS depth layout",
            &[
                entry(0, texture(D::D2, false), S::COMPUTE),
                entry(1, storage_texture(F::R32Float), S::COMPUTE),
            ],
        );
        let grid_bgl = bgl(
            device,
            "HLFS light grid layout",
            &[
                entry(0, storage(false), S::COMPUTE),
                entry(1, storage(false), S::COMPUTE),
                entry(2, storage_texture(F::R32Float), S::COMPUTE),
                entry(3, storage(false), S::COMPUTE),
            ],
        );
        let sample_bgl = bgl(
            device,
            "HLFS sampling layout",
            &[
                entry(0, storage(true), S::COMPUTE),
                entry(1, storage(true), S::COMPUTE),
                entry(2, storage(false), S::COMPUTE),
                entry(3, storage_texture(F::Rg32Uint), S::COMPUTE),
                entry(4, uint_texture(), S::COMPUTE),
                entry(5, texture(D::D2, false), S::COMPUTE),
                entry(6, storage(true), S::COMPUTE),
                entry(7, uint_texture(), S::COMPUTE),
                entry(8, storage_texture(F::Rgba32Uint), S::COMPUTE),
            ],
        );
        let temporal_bgl = bgl(
            device,
            "HLFS temporal layout",
            &[
                entry(0, uint_texture(), S::COMPUTE),
                entry(1, uint_texture(), S::COMPUTE),
                entry(2, uint_texture(), S::COMPUTE),
                entry(3, storage_texture(F::Rg32Uint), S::COMPUTE),
                entry(4, storage_texture(F::Rg32Uint), S::COMPUTE),
                entry(5, storage(true), S::COMPUTE),
                entry(6, storage(true), S::COMPUTE),
            ],
        );
        let spatial_bgl = bgl(
            device,
            "HLFS spatial layout",
            &[
                entry(0, uint_texture(), S::COMPUTE),
                entry(1, uint_texture(), S::COMPUTE),
                entry(2, storage_texture(F::Rg32Uint), S::COMPUTE),
            ],
        );
        let composite_bgl = bgl(
            device,
            "HLFS composite layout",
            &(0..7)
                .map(|i| {
                    entry(
                        i,
                        if i >= 5 {
                            storage(true)
                        } else if i == 2 {
                            texture(D::D2, false)
                        } else {
                            uint_texture()
                        },
                        S::FRAGMENT,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let module = |name| helio_core::shader::module(device, name, &shader_source(name));
        let grid_shader = module("grid");
        let temporal_shader = module("temporal");
        let layout =
            |label, third: &wgpu::BindGroupLayout, fourth: Option<&wgpu::BindGroupLayout>| {
                let mut layouts = vec![Some(&common_bgl), Some(&gbuffer_bgl), Some(third)];
                if let Some(fourth) = fourth {
                    layouts.push(Some(fourth));
                }
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: &layouts,
                    immediate_size: 0,
                })
            };
        let depth_reduce = compute(
            device,
            "HLFS depth reduction",
            &module("depth"),
            "reduce_depth",
            &layout("HLFS depth", &depth_bgl, None),
        );
        let grid_layout = layout("HLFS grid", &grid_bgl, None);
        let select_key = compute(
            device,
            "HLFS dominant light",
            &grid_shader,
            "select_key",
            &grid_layout,
        );
        let coarse = compute(
            device,
            "HLFS coarse culling",
            &grid_shader,
            "coarse",
            &grid_layout,
        );
        let fine = compute(
            device,
            "HLFS fine depth culling",
            &grid_shader,
            "fine",
            &grid_layout,
        );
        let visibility = VisibilityPipelines::new(
            device,
            mode,
            presampled,
            &common_bgl,
            &gbuffer_bgl,
            &sample_bgl,
            rt_bgl.as_ref(),
            false,
        );
        let temporal = compute(
            device,
            "HLFS temporal denoising",
            &temporal_shader,
            "temporal",
            &layout("HLFS temporal", &temporal_bgl, None),
        );
        let spatial = compute(
            device,
            "HLFS spatial denoising",
            &module("spatial"),
            "spatial",
            &layout("HLFS spatial", &spatial_bgl, None),
        );
        let composite = composite_pipeline(
            device,
            output_format,
            mode,
            presampled,
            &common_bgl,
            &gbuffer_bgl,
            &composite_bgl,
            rt_bgl.as_ref(),
            false,
        );
        Self {
            compact_bgl,
            compact,
            list_live,
            transmission_visibility: Some(VisibilityPipelines::new(
                device,
                mode,
                presampled,
                &common_bgl,
                &gbuffer_bgl,
                &sample_bgl,
                rt_bgl.as_ref(),
                true,
            )),
            transmission_composite: Some(composite_pipeline(
                device,
                output_format,
                mode,
                presampled,
                &common_bgl,
                &gbuffer_bgl,
                &composite_bgl,
                rt_bgl.as_ref(),
                true,
            )),
            common_bgl,
            gbuffer_bgl,
            depth_bgl,
            grid_bgl,
            sample_bgl,
            temporal_bgl,
            spatial_bgl,
            composite_bgl,
            depth_reduce,
            coarse,
            select_key,
            fine,
            visibility,
            temporal,
            spatial,
            composite,
            rt_bgl,
            output_format,
        }
    }
}

fn composite_pipeline(
    device: &wgpu::Device,
    output_format: wgpu::TextureFormat,
    mode: HlfsMode,
    presampled: bool,
    common: &wgpu::BindGroupLayout,
    gbuffer: &wgpu::BindGroupLayout,
    composite: &wgpu::BindGroupLayout,
    rt: Option<&wgpu::BindGroupLayout>,
    transmission: bool,
) -> wgpu::RenderPipeline {
    let stage = if mode == HlfsMode::RayTraced {
        "ray_traced_composite"
    } else {
        "composite"
    };
    let composite_shader = helio_core::shader::module(
        device,
        stage,
        &shader_source_for_transmission(stage, presampled, transmission),
    );
    let mut layouts = vec![Some(common), Some(gbuffer), Some(composite)];
    if mode == HlfsMode::RayTraced {
        layouts.push(rt);
    }
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("HLFS composite"),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    helio_core::shader::create_render_pipeline(
        &device,
        wgpu::RenderPipelineDescriptor {
            label: Some("HLFS denoise and composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        },
    )
}
