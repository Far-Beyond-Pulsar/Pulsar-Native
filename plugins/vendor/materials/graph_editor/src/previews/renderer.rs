use wgpu::*;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    time: f32,
    _padding: [f32; 3],
}

const PIN_PREVIEW_VERTEX_SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    time: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );

    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );

    var out: VertexOutput;
    let pos = positions[vertex_index];
    let uv = uvs[vertex_index];
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.world_pos = vec3<f32>(uv * 2.0 - vec2<f32>(1.0, 1.0), 0.0);
    return out;
}
"#;

pub struct PinPreviewRenderer {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    bind_group: BindGroup,
    texture_bind_group: Option<BindGroup>,
    _textures: Vec<Texture>,
}

impl PinPreviewRenderer {
    pub const TEXTURE_FORMAT: TextureFormat = TextureFormat::Bgra8UnormSrgb;

    pub fn new(device: &Device, queue: &Queue, wgsl_source: &str) -> Self {
        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("pin_preview_uniforms"),
            size: std::mem::size_of::<PreviewUniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("pin_preview_bind_group_layout"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("pin_preview_bind_group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let texture_sources = texture_source_paths(wgsl_source);
        let (texture_bind_group_layout, texture_bind_group, textures) =
            if texture_sources.is_empty() {
                (None, None, Vec::new())
            } else {
                let (layout, bind_group, textures) =
                    create_texture_bindings(device, queue, &texture_sources);
                (Some(layout), Some(bind_group), textures)
            };

        let mut bind_group_layouts = vec![Some(&bind_group_layout)];
        if let Some(texture_layout) = texture_bind_group_layout.as_ref() {
            bind_group_layouts.push(Some(texture_layout));
        }
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("pin_preview_pipeline_layout"),
            bind_group_layouts: &bind_group_layouts,
            immediate_size: 0,
        });

        let vs_module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("pin_preview_vertex_shader"),
            source: ShaderSource::Wgsl(PIN_PREVIEW_VERTEX_SHADER.into()),
        });
        let fs_module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("pin_preview_fragment_shader"),
            source: ShaderSource::Wgsl(wgsl_source.into()),
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("pin_preview_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &vs_module,
                entry_point: Some("vertex_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(FragmentState {
                module: &fs_module,
                entry_point: Some("fragment_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format: Self::TEXTURE_FORMAT,
                    blend: Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            texture_bind_group,
            _textures: textures,
        }
    }

    pub fn render(&self, device: &Device, queue: &Queue, output: &TextureView, time: f32) {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let uniforms = PreviewUniforms {
            view_proj: identity,
            model: identity,
            time,
            _padding: [0.0; 3],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("pin_preview_encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("pin_preview_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            if let Some(texture_bind_group) = &self.texture_bind_group {
                pass.set_bind_group(1, texture_bind_group, &[]);
            }
            pass.draw(0..6, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

fn texture_source_paths(wgsl: &str) -> Vec<String> {
    wgsl.lines()
        .filter_map(|line| line.trim().strip_prefix("// TextureSrc:"))
        .filter_map(|path| serde_json::from_str::<String>(path.trim()).ok())
        .collect()
}

fn create_texture_bindings(
    device: &Device,
    queue: &Queue,
    asset_paths: &[String],
) -> (BindGroupLayout, BindGroup, Vec<Texture>) {
    let mut layout_entries = Vec::with_capacity(asset_paths.len() + 1);
    layout_entries.push(BindGroupLayoutEntry {
        binding: 0,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    });
    for index in 0..asset_paths.len() {
        layout_entries.push(BindGroupLayoutEntry {
            binding: index as u32 + 1,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("pin_preview_texture_bind_group_layout"),
        entries: &layout_entries,
    });

    let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
    let mut textures = Vec::with_capacity(asset_paths.len());
    let mut views = Vec::with_capacity(asset_paths.len());
    for (index, asset_path) in asset_paths.iter().enumerate() {
        let relative_path = std::path::Path::new(asset_path);
        let resolved_path = if relative_path.is_absolute() {
            relative_path.to_path_buf()
        } else {
            project_root
                .as_ref()
                .map(|root| root.join(relative_path))
                .unwrap_or_else(|| relative_path.to_path_buf())
        };
        let (width, height, pixels) = match image::open(&resolved_path) {
            Ok(image) => {
                let image = image.to_rgba8();
                (
                    image.width().max(1),
                    image.height().max(1),
                    image.into_raw(),
                )
            }
            Err(error) => {
                tracing::warn!(
                    "Could not load pin preview texture '{}': {}",
                    resolved_path.display(),
                    error
                );
                (1, 1, vec![255, 255, 255, 255])
            }
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("pin_preview_source_texture"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        views.push(texture.create_view(&TextureViewDescriptor::default()));
        textures.push(texture);
    }

    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("pin_preview_texture_sampler"),
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        address_mode_w: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..Default::default()
    });
    let mut entries = Vec::with_capacity(views.len() + 1);
    entries.push(BindGroupEntry {
        binding: 0,
        resource: BindingResource::Sampler(&sampler),
    });
    for (index, view) in views.iter().enumerate() {
        entries.push(BindGroupEntry {
            binding: index as u32 + 1,
            resource: BindingResource::TextureView(view),
        });
    }
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("pin_preview_texture_bind_group"),
        layout: &layout,
        entries: &entries,
    });
    (layout, bind_group, textures)
}
