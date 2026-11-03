//! Scene-linear lens response: image-based scattering of everything bright in
//! the frame plus analytic lens sources from scene lights. See README.md for
//! the optical model and the `postprocess_uniforms` -> `lens_output` contract.

pub mod feature;
pub use feature::LensFlareFeature;

use helio_core::graph::ResourceBuilder;
use helio_core::{CommandRecorder, PassContext, RenderPass, ResourceKey, Result as HelioResult};
use helio_pass_lens::{ResolvedLens, RESOLVED_KEY};
use wgpu::util::DeviceExt;

pub mod gpu_types;
pub use gpu_types::*;

pub const OUTPUT_KEY: &str = "lens_output";
/// Optional graph texture replacing the built-in procedural lens dirt.
pub const DIRT_KEY: &str = "lens_dirt";
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Mip levels of the extracted-light pyramid; must equal `LEVELS` in the WGSL.
const LEVELS: u32 = 5;
const DIRT_SIZE: u32 = 512;

struct Image {
    texture: wgpu::Texture,
    /// All mip levels, for sampling.
    view: wgpu::TextureView,
    /// One single-level view per mip, for storage writes and downsampling.
    levels: Vec<wgpu::TextureView>,
}

impl Image {
    fn new(device: &wgpu::Device, width: u32, height: u32, label: &str) -> Self {
        Self::with_levels(device, width, height, 1, label)
    }

    fn with_levels(device: &wgpu::Device, width: u32, height: u32, mips: u32, label: &str) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let levels = (0..mips)
            .map(|level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        Self {
            texture,
            view,
            levels,
        }
    }
}

/// Scene inputs for analytic lens sources. Every field is optional: missing
/// producers fall back to neutral stand-ins (no lights, no occluders).
#[derive(Default, Clone, Copy)]
pub struct OpticsInputs<'a> {
    pub camera: Option<&'a wgpu::Buffer>,
    pub lights: Option<&'a wgpu::Buffer>,
    pub depth: Option<&'a wgpu::TextureView>,
    pub shadow_matrices: Option<&'a wgpu::Buffer>,
    pub shadow_atlas: Option<&'a wgpu::TextureView>,
    pub dirt: Option<&'a wgpu::TextureView>,
    /// The camera's optics: the lens pass's `ResolvedLens`.
    pub lens: Option<&'a wgpu::Buffer>,
}

type BindingKey = [Option<wgpu::TextureView>; 5];

/// All persistent state is derived GPU machinery; no CPU copy of settings or lights.
pub struct LensFlarePass {
    input_key: &'static str,
    /// The image the input may be a view of (`helio_frame_api::color::alias`).
    input_alias: Option<&'static str>,
    control: wgpu::ComputePipeline,
    extract: wgpu::ComputePipeline,
    downsample: wgpu::ComputePipeline,
    sources_pipeline: wgpu::ComputePipeline,
    response: wgpu::ComputePipeline,
    temporal: wgpu::ComputePipeline,
    control_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    optics_layout: wgpu::BindGroupLayout,
    temporal_layout: wgpu::BindGroupLayout,
    dispatch: wgpu::Buffer,
    sources: wgpu::Buffer,
    bright: Image,
    output: Image,
    /// This frame's response before temporal filtering.
    raw: Image,
    /// Last frame's filtered output (copied after the temporal pass).
    history: wgpu::Texture,
    history_view: wgpu::TextureView,
    /// dt seconds and history validity, written in prepare().
    temporal_params: wgpu::Buffer,
    dirt: wgpu::TextureView,
    dirt_texture: wgpu::Texture,
    /// Procedural dirt pixels, uploaded by the first recorded frame (the
    /// constructor has no queue).
    dirt_upload: Option<wgpu::Buffer>,
    dirt_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    fallback_camera: wgpu::Buffer,
    fallback_lights: wgpu::Buffer,
    fallback_matrices: wgpu::Buffer,
    /// The neutral 50 mm f/2.8 lens, for graphs without a lens pass.
    fallback_lens: wgpu::Buffer,
    fallback_depth: wgpu::TextureView,
    fallback_shadow: wgpu::TextureView,
    bindings: Option<(
        (BindingKey, [Option<wgpu::Buffer>; 5]),
        wgpu::BindGroup,
        Vec<wgpu::BindGroup>,
        wgpu::BindGroup,
        wgpu::BindGroup,
    )>,
}

impl LensFlarePass {
    /// Compatibility constructor: lights, queue and display format are unused.
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _lights_buf: &wgpu::Buffer,
        width: u32,
        height: u32,
        _surface_format: wgpu::TextureFormat,
    ) -> Self {
        Self::new_hdr(device, width, height)
    }

    pub fn new_hdr(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(POSTPROCESS_BINDING_SIZE),
            },
            count: None,
        };
        let sampled = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: compute,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lens image layout"),
            entries: &[
                uniform,
                sampled,
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: OUTPUT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let control_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lens dispatch layout"),
            entries: &[
                uniform,
                sampled,
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(12 * LEVELS as u64),
                    },
                    count: None,
                },
            ],
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let optics_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lens optics layout"),
            entries: &[
                storage(0, true),
                storage(1, true),
                texture(
                    2,
                    wgpu::TextureSampleType::Depth,
                    wgpu::TextureViewDimension::D2,
                ),
                storage(3, true),
                texture(
                    4,
                    wgpu::TextureSampleType::Depth,
                    wgpu::TextureViewDimension::D2Array,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                storage(6, false),
                texture(
                    7,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                storage(9, true),
            ],
        });
        let temporal_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lens temporal layout"),
            entries: &[
                uniform,
                sampled,
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: OUTPUT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                texture(
                    4,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = helio_core::shader::module(
            device,
            "Physical lens response",
            helio_core::include_wgsl!("../shaders/lens_response.wgsl"),
        );
        let pipeline = |entry, layouts: &[Option<&wgpu::BindGroupLayout>]| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let depth_texture = |label, layers: bool| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(if layers {
                        wgpu::TextureViewDimension::D2Array
                    } else {
                        wgpu::TextureViewDimension::D2
                    }),
                    ..Default::default()
                })
        };
        let (dirt_texture, dirt_upload) = procedural_dirt(device);
        let (w, h) = reduced_size(width, height);
        let history = history_texture(device, w, h);
        Self {
            input_key: "pre_aa",
            input_alias: None,
            control: pipeline("cs_control", &[Some(&control_layout)]),
            extract: pipeline("cs_extract", &[Some(&image_layout), Some(&optics_layout)]),
            downsample: pipeline("cs_downsample", &[Some(&image_layout)]),
            sources_pipeline: pipeline("cs_sources", &[Some(&image_layout), Some(&optics_layout)]),
            response: pipeline("cs_response", &[Some(&image_layout), Some(&optics_layout)]),
            temporal: pipeline(
                "cs_temporal",
                &[Some(&temporal_layout), Some(&optics_layout)],
            ),
            control_layout,
            image_layout,
            optics_layout,
            temporal_layout,
            dispatch: buffer(
                "Lens indirect dispatch",
                12 * LEVELS as u64,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            sources: buffer(
                "Lens analytic sources",
                LENS_SOURCES_SIZE,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            bright: Image::with_levels(device, w, h, LEVELS, "Lens reduced radiance"),
            output: Image::new(device, w, h, "Lens response HDR"),
            raw: Image::new(device, w, h, "Lens response raw"),
            history: history.0,
            history_view: history.1,
            temporal_params: buffer(
                "Lens temporal params",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            dirt: dirt_texture.create_view(&Default::default()),
            dirt_texture,
            dirt_upload: Some(dirt_upload),
            dirt_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lens dirt sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lens shadow sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            }),
            // Zero cameras/lights: the source scan finds no light and exits.
            fallback_camera: buffer(
                "Lens neutral camera",
                2 * std::mem::size_of::<helio_core::GpuCameraUniforms>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_lights: buffer("Lens no lights", 128, wgpu::BufferUsages::STORAGE),
            fallback_matrices: buffer("Lens no shadow matrices", 96, wgpu::BufferUsages::STORAGE),
            fallback_lens: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Lens neutral optics"),
                contents: bytemuck::bytes_of(&ResolvedLens::default()),
                usage: wgpu::BufferUsages::STORAGE,
            }),
            fallback_depth: depth_texture("Lens neutral depth", false),
            fallback_shadow: depth_texture("Lens empty shadow atlas", true),
            bindings: None,
        }
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
        &self.output.view
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (w, h) = reduced_size(width, height);
        if (self.output.texture.width(), self.output.texture.height()) != (w, h) {
            self.bright = Image::with_levels(device, w, h, LEVELS, "Lens reduced radiance");
            self.output = Image::new(device, w, h, "Lens response HDR");
            self.raw = Image::new(device, w, h, "Lens response raw");
            (self.history, self.history_view) = history_texture(device, w, h);
            self.bindings = None;
        }
    }

    /// This frame's filtered source image (pyramid level 0) becomes next
    /// frame's reprojected history.
    fn store_history(&self, cmds: &mut CommandRecorder<'_>) {
        cmds.copy_texture_to_texture(
            self.bright.texture.as_image_copy(),
            self.history.as_image_copy(),
            self.history.size(),
        );
    }

    fn clear(&self, cmds: &mut CommandRecorder<'_>) {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &self.output.view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        // Fast clear guarantees no stale response when disabled or resources vanish.
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
        pp: Option<&wgpu::Buffer>,
        optics: OpticsInputs<'_>,
    ) {
        if let Some(upload) = self.dirt_upload.take() {
            cmds.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &upload,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(DIRT_SIZE * 4),
                        rows_per_image: Some(DIRT_SIZE),
                    },
                },
                self.dirt_texture.as_image_copy(),
                wgpu::Extent3d {
                    width: DIRT_SIZE,
                    height: DIRT_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }
        if let Some(input) = input {
            self.resize(device, input.texture().width(), input.texture().height());
        }
        self.clear(cmds);
        // Old PP buffers fail closed without an out-of-bounds uniform binding.
        let (Some(input), Some(pp)) = (
            input,
            pp.filter(|b| {
                b.size() >= POSTPROCESS_BINDING_SIZE
                    && b.usage().contains(wgpu::BufferUsages::UNIFORM)
            }),
        ) else {
            return;
        };
        let camera = optics.camera.unwrap_or(&self.fallback_camera);
        let lights = optics.lights.unwrap_or(&self.fallback_lights);
        let matrices = optics.shadow_matrices.unwrap_or(&self.fallback_matrices);
        let depth = optics.depth.unwrap_or(&self.fallback_depth);
        let shadow = optics.shadow_atlas.unwrap_or(&self.fallback_shadow);
        let dirt = optics.dirt.unwrap_or(&self.dirt);
        let lens = optics.lens.unwrap_or(&self.fallback_lens);
        // Bind groups are cached per GPU handle; comparing handles, not
        // pointers, is safe across TSR's rotating outputs and resize.
        let key = (
            [
                Some(input.clone()),
                Some(self.output.view.clone()),
                Some(depth.clone()),
                Some(shadow.clone()),
                Some(dirt.clone()),
            ],
            [
                Some(pp.clone()),
                Some(camera.clone()),
                Some(lights.clone()),
                Some(matrices.clone()),
                Some(lens.clone()),
            ],
        );
        if self.bindings.as_ref().map(|(k, ..)| k) != Some(&key) {
            let uniform = || {
                wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: pp,
                    offset: 0,
                    size: wgpu::BufferSize::new(POSTPROCESS_BINDING_SIZE),
                })
            };
            let control = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lens control bindings"),
                layout: &self.control_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.bright.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.dispatch.as_entire_binding(),
                    },
                ],
            });
            let images = |src, dst| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Lens image bindings"),
                    layout: &self.image_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(src),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(dst),
                        },
                    ],
                })
            };
            // Extract, blend with reprojected history into level 0, build the
            // pyramid, then respond from it.
            let mut steps = vec![images(input, &self.raw.view)];
            for level in 1..LEVELS as usize {
                steps.push(images(
                    &self.bright.levels[level - 1],
                    &self.bright.levels[level],
                ));
            }
            steps.push(images(&self.bright.view, &self.output.view));
            let optics_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lens optics bindings"),
                layout: &self.optics_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: lights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: matrices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(shadow),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.sources.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(dirt),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&self.dirt_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: lens.as_entire_binding(),
                    },
                ],
            });
            let temporal_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lens temporal bindings"),
                layout: &self.temporal_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.raw.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.bright.levels[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&self.history_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.temporal_params.as_entire_binding(),
                    },
                ],
            });
            self.bindings = Some((key, control, steps, optics_group, temporal_group));
        }
        let (_, control, steps, optics_group, temporal_group) = self.bindings.as_ref().unwrap();
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Lens enable dispatch"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.control);
            pass.set_bind_group(0, control, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        let response_step = steps.len() - 1;
        {
            // Classify scene lights into analytic lens sources. One workgroup;
            // it writes an empty list when the lens or light sources are off.
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Lens light sources"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.sources_pipeline);
            pass.set_bind_group(0, &steps[response_step], &[]);
            pass.set_bind_group(1, optics_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Every dispatch is GPU-sized: zero groups when the lens is off.
        for (step, bindings) in steps.iter().enumerate() {
            let (pipeline, offset, label) = if step == 0 {
                (&self.extract, 0, "Lens HDR extraction")
            } else if step < response_step {
                (&self.downsample, 12 * step as u64, "Lens pyramid")
            } else {
                (&self.response, 0, "Lens optical response")
            };
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bindings, &[]);
            if step == 0 || step == response_step {
                pass.set_bind_group(1, optics_group, &[]);
            }
            pass.dispatch_workgroups_indirect(&self.dispatch, offset);
            drop(pass);
            if step == 0 {
                // Blend this frame's extracted light with the reprojected
                // history into pyramid level 0, then keep it as next history.
                let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Lens temporal source"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.temporal);
                pass.set_bind_group(0, temporal_group, &[]);
                pass.set_bind_group(1, optics_group, &[]);
                pass.dispatch_workgroups_indirect(&self.dispatch, 0);
                drop(pass);
                self.store_history(cmds);
            }
        }
    }
}

/// Zero-initialised, so the first frame fades in from black.
fn history_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lens response history"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// Quarter resolution, but never below what the `LEVELS`-deep radiance
/// pyramid needs (16 px per side): tiny targets otherwise request more mips
/// than the texture can hold, which is a validation error.
fn reduced_size(width: u32, height: u32) -> (u32, u32) {
    let min_side = 1 << (LEVELS - 1);
    (
        width.max(1).div_ceil(4).max(min_side),
        height.max(1).div_ceil(4).max(min_side),
    )
}

/// Deterministic procedural front-element dirt: soft smudges and fine specks
/// on a faint haze, luminance roughly 0..1. Replace with `DIRT_KEY`.
fn procedural_dirt(device: &wgpu::Device) -> (wgpu::Texture, wgpu::Buffer) {
    let size = DIRT_SIZE as usize;
    let mut value = vec![0.04f32; size * size];
    let mut state: u64 = 0x5eed_1e45_c0ff_ee01;
    let mut random = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as f32) / (1u64 << 31) as f32
    };
    // (count, min radius, max radius, strength): smudges, then specks.
    for (count, r_min, r_max, strength) in [(28, 18.0, 70.0, 0.35), (140, 1.5, 5.0, 0.6)] {
        for _ in 0..count {
            let (cx, cy) = (random() * size as f32, random() * size as f32);
            let radius = r_min + (r_max - r_min) * random();
            let amount = strength * (0.4 + 0.6 * random());
            let r = radius.ceil() as i32 + 1;
            for dy in -r..=r {
                for dx in -r..=r {
                    let d = ((dx * dx + dy * dy) as f32).sqrt() / radius;
                    if d >= 1.0 {
                        continue;
                    }
                    let x = (cx as i32 + dx).rem_euclid(size as i32) as usize;
                    let y = (cy as i32 + dy).rem_euclid(size as i32) as usize;
                    let falloff = (1.0 - d * d).powi(2);
                    value[y * size + x] += amount * falloff;
                }
            }
        }
    }
    let texels: Vec<u8> = value
        .iter()
        .flat_map(|v| {
            let c = (v.min(1.0) * 255.0) as u8;
            [c, c, c, 255]
        })
        .collect();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lens procedural dirt"),
        size: wgpu::Extent3d {
            width: DIRT_SIZE,
            height: DIRT_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let upload = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Lens dirt upload"),
        size: texels.len() as u64,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    upload
        .slice(..)
        .get_mapped_range_mut()
        .expect("mapped at creation")
        .copy_from_slice(&texels);
    upload.unmap();
    (texture, upload)
}

impl RenderPass for LensFlarePass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["scene_lights"]
    }

    fn name(&self) -> &'static str {
        "LensFlare"
    }
    fn writes(&self) -> &'static [&'static str] {
        &[OUTPUT_KEY]
    }
    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("postprocess_uniforms");
        builder.read(self.input_key);
        if let Some(alias) = self.input_alias {
            builder.read(alias);
        }
        // Analytic light sources: current-frame shadows for lens visibility.
        builder.read("shadow_atlas");
        builder.read("shadow_matrices");
        builder.read(DIRT_KEY);
        builder.read(RESOLVED_KEY);
        // A manually owned resource: this declaration tracks its dependency,
        // while publish routes the actual sampled view without a second texture.
        builder.write_buffer(OUTPUT_KEY);
    }
    fn prepare(&mut self, ctx: &helio_core::PrepareContext) -> HelioResult<()> {
        // History starts black and stays valid, so the lens fades in rather
        // than popping on the first frame or after being enabled.
        let params = [ctx.delta_time.max(0.0), 1.0, 0.0, 0.0];
        ctx.queue
            .write_buffer(&self.temporal_params, 0, bytemuck::cast_slice(&params));
        Ok(())
    }
    fn on_resize(&mut self, device: &wgpu::Device, size: helio_core::RenderSize) {
        let [width, height] = size.internal;
        self.resize(device, width, height);
    }
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        frame.route_named_texture(OUTPUT_KEY, &self.output.view, self.name());
    }
    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let input = ctx
            .registry
            .get(ResourceKey::new(self.input_key))
            .or_else(|| {
                self.input_alias
                    .and_then(|alias| ctx.registry.get(ResourceKey::new(alias)))
            });
        let pp = ctx.registry.get(ResourceKey::new("postprocess_uniforms"));
        let optics = OpticsInputs {
            camera: Some(ctx.camera),
            lights: ctx
                .scene_buffers
                .get(helio_core::BufferKey::of("scene_lights"))
                .map(|handle| &handle.buffer),
            depth: Some(ctx.depth),
            shadow_matrices: ctx
                .registry
                .get::<helio_pass_shadow_matrix::ShadowMatricesFrameData<'_>>(
                    helio_pass_shadow_matrix::shadow_matrices(),
                )
                .map(|s| s.shadow_matrices),
            shadow_atlas: ctx
                .registry
                .get::<&wgpu::TextureView>(ResourceKey::new("shadow_atlas")),
            dirt: ctx
                .registry
                .get::<&wgpu::TextureView>(ResourceKey::new(DIRT_KEY)),
            lens: ctx
                .registry
                .get::<&wgpu::Buffer>(ResourceKey::new(RESOLVED_KEY)),
        };
        // Must follow fog/TSR on the graphics encoder. The separate compute
        // encoder is submitted BEFORE graphics and would sample stale HDR.
        self.record(ctx.device, &mut ctx.graphics_cmds(), input, pp, optics);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
