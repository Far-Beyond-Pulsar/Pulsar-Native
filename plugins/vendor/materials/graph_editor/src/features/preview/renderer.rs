//! WGPU-based 3D preview renderer

use super::camera::OrbitCamera;
use super::mesh::PreviewVertex;
use wgpu::*;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    time: f32,
    _padding: [f32; 3],
}

const VERTEX_SHADER_SRC: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
};

struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    time: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// WGSL has no built-in matrix inverse. This computes `transpose(inverse(m))`
// directly via the adjugate/determinant method — i.e. the normal matrix for
// model matrix `m` — since that's the only thing it's used for here.
fn normal_matrix3x3(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0];
    let b = m[1];
    let c = m[2];

    let cross_bc = cross(b, c);
    let cross_ca = cross(c, a);
    let cross_ab = cross(a, b);

    let det = dot(a, cross_bc);
    let inv_det = 1.0 / det;

    return mat3x3<f32>(
        cross_bc * inv_det,
        cross_ca * inv_det,
        cross_ab * inv_det,
    );
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    let world_pos = (uniforms.model * vec4(input.position, 1.0)).xyz;
    let model3 = mat3x3<f32>(
        uniforms.model[0].xyz,
        uniforms.model[1].xyz,
        uniforms.model[2].xyz,
    );
    let normal_mat = normal_matrix3x3(model3);

    var output: VertexOutput;
    output.position = uniforms.view_proj * vec4(world_pos, 1.0);
    output.uv = input.uv;
    output.normal = normalize(normal_mat * input.normal);
    output.world_pos = world_pos;
    return output;
}
"#;

/// Fullscreen sky/horizon gradient, drawn behind the preview mesh.
///
/// Reconstructs a view ray per-pixel from the camera basis vectors (no
/// matrix inversion needed) and shades it with a zenith→horizon→ground
/// gradient, a warm horizon glow band, and a slowly orbiting sun — handy as
/// a stable visual reference for checking the camera's orientation.
const SKY_SHADER_SRC: &str = r#"
struct SkyUniforms {
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    camera_forward: vec4<f32>,
    params: vec4<f32>, // tan(fov_y / 2), aspect, time, unused
};

@group(0) @binding(0) var<uniform> sky: SkyUniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );

    var out: VertexOutput;
    let p = positions[vertex_index];
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tan_half_fovy = sky.params.x;
    let aspect = sky.params.y;
    let time = sky.params.z;

    let right = sky.camera_right.xyz;
    let up = sky.camera_up.xyz;
    let forward = sky.camera_forward.xyz;

    let dir = normalize(
        forward
            + in.ndc.x * tan_half_fovy * aspect * right
            + in.ndc.y * tan_half_fovy * up
    );

    let zenith = vec3<f32>(0.10, 0.30, 0.70);
    let horizon_sky = vec3<f32>(0.75, 0.86, 0.97);
    let horizon_ground = vec3<f32>(0.55, 0.50, 0.45);
    let ground = vec3<f32>(0.16, 0.15, 0.14);

    let h = dir.y;
    var color: vec3<f32>;
    if (h >= 0.0) {
        color = mix(horizon_sky, zenith, pow(h, 0.45));
    } else {
        color = mix(horizon_ground, ground, pow(clamp(-h, 0.0, 1.0), 0.5));
    }

    // Warm horizon glow band.
    let glow = exp(-abs(h) * 12.0) * 0.5;
    color = color + vec3<f32>(1.0, 0.85, 0.6) * glow;

    // Slowly orbiting sun: bright core plus a soft halo.
    let sun_angle = time * 0.1;
    let sun_dir = normalize(vec3<f32>(cos(sun_angle), 0.35, sin(sun_angle)));
    let sun_dot = dot(dir, sun_dir);
    let sun_disc = smoothstep(0.9995, 0.9999, sun_dot);
    let sun_halo = pow(max(sun_dot, 0.0), 64.0) * 0.6;
    color = color + vec3<f32>(1.0, 0.95, 0.8) * (sun_disc + sun_halo);

    return vec4<f32>(color, 1.0);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniforms {
    camera_right: [f32; 4],
    camera_up: [f32; 4],
    camera_forward: [f32; 4],
    params: [f32; 4],
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
    let mut entries = Vec::with_capacity(asset_paths.len() + 1);
    entries.push(BindGroupLayoutEntry {
        binding: 0,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    });
    for index in 0..asset_paths.len() {
        entries.push(BindGroupLayoutEntry {
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
        label: Some("material texture bindings"),
        entries: &entries,
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
        // A preview sphere never needs a 4K texture: scale the larger side
        // down before the RGBA conversion, so a material graph sampling
        // several 4K images does not put hundreds of MiB on the GPU for a
        // few hundred pixels of preview.
        let decoded = image::open(&resolved_path)
            .map(|image| fit_preview_texture(image).to_rgba8());
        let (width, height, pixels) = match decoded {
            Ok(image) => (
                image.width().max(1),
                image.height().max(1),
                image.into_raw(),
            ),
            Err(error) => {
                tracing::warn!(
                    "Could not load material preview texture '{}': {}",
                    resolved_path.display(),
                    error
                );
                (1, 1, vec![255, 255, 255, 255])
            }
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(&format!("material preview texture {index}")),
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
        label: Some("material preview sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..Default::default()
    });
    let mut bind_entries = Vec::with_capacity(views.len() + 1);
    bind_entries.push(BindGroupEntry {
        binding: 0,
        resource: BindingResource::Sampler(&sampler),
    });
    for (index, view) in views.iter().enumerate() {
        bind_entries.push(BindGroupEntry {
            binding: index as u32 + 1,
            resource: BindingResource::TextureView(view),
        });
    }
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("material preview texture bind group"),
        layout: &layout,
        entries: &bind_entries,
    });

    (layout, bind_group, textures)
}

pub struct PreviewRenderer {
    pub device: Option<Device>,
    pub queue: Option<Queue>,
    pipeline: Option<RenderPipeline>,
    pipeline_layout: Option<PipelineLayout>,
    uniform_buffer: Option<Buffer>,
    uniform_bind_group_layout: Option<BindGroupLayout>,
    bind_group: Option<BindGroup>,
    texture_bind_group_layout: Option<BindGroupLayout>,
    texture_bind_group: Option<BindGroup>,
    _texture_assets: Vec<Texture>,
    pub mesh_vertex_buffer: Option<Buffer>,
    pub mesh_index_buffer: Option<Buffer>,
    pub mesh_index_count: u32,
    sky_pipeline: Option<RenderPipeline>,
    sky_uniform_buffer: Option<Buffer>,
    sky_bind_group: Option<BindGroup>,
    plain_black_background: bool,
    pub camera: OrbitCamera,
    shader_module: Option<ShaderModule>,
    surface_config: Option<SurfaceConfiguration>,
    needs_recompile: bool,
    needs_upload: bool,
    start_time: std::time::Instant,
}


/// Handles a shader build needs; see [`PreviewRenderer::shader_build_inputs`].
#[derive(Clone)]
pub struct ShaderBuildInputs {
    device: Device,
    queue: Queue,
    uniform_layout: BindGroupLayout,
    target_format: TextureFormat,
}

/// The GPU objects one compiled preview shader produces.
pub struct ShaderBuild {
    pipeline: Option<RenderPipeline>,
    pipeline_layout: PipelineLayout,
    texture_bind_group_layout: Option<BindGroupLayout>,
    texture_bind_group: Option<BindGroup>,
    texture_assets: Vec<Texture>,
    /// Why the GPU rejected the shader; `None` when `pipeline` was built.
    pub error: Option<String>,
}

/// Compile `wgsl_source` into a pipeline. Slow (shader compilation, texture
/// decode); safe to call from any thread, so the editor never waits on it.
///
/// PSGC compiles each shader graph to a complete, self-contained
/// `@fragment fn fragment_main(...)` module, used directly as the fragment
/// shader rather than wrapped inside another function.
pub fn build_shader(inputs: &ShaderBuildInputs, wgsl_source: &str) -> ShaderBuild {
    let ShaderBuildInputs {
        device,
        queue,
        uniform_layout,
        target_format,
    } = inputs;
    // User-authored graphs must not route validation failures through
    // WGPU's uncaptured-error handler, which panics the editor.
    let error_scope = device.push_error_scope(ErrorFilter::Validation);

    let texture_sources = texture_source_paths(wgsl_source);
    let texture_resources = if texture_sources.is_empty() {
        None
    } else {
        Some(create_texture_bindings(device, queue, &texture_sources))
    };
    let texture_assets = texture_resources
        .as_ref()
        .map(|(_, _, textures)| textures.iter().map(Texture::clone).collect())
        .unwrap_or_default();
    let texture_bind_group_layout = texture_resources
        .as_ref()
        .map(|(layout, _, _)| layout.clone());
    let texture_bind_group = texture_resources
        .as_ref()
        .map(|(_, bind_group, _)| bind_group.clone());

    let mut layouts = vec![Some(uniform_layout)];
    if let Some((texture_layout, _, _)) = texture_resources.as_ref() {
        layouts.push(Some(texture_layout));
    }
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("material preview pipeline layout"),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });

    let vs_module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("preview vertex shader"),
        source: ShaderSource::Wgsl(VERTEX_SHADER_SRC.into()),
    });
    let fs_module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("preview fragment shader"),
        source: ShaderSource::Wgsl(wgsl_source.into()),
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("preview pipeline"),
        // Use the bind group layout created in `initialize()` (and shared
        // with the renderer's bind group) explicitly: `layout: None` would
        // derive its own internal layout via shader reflection, a *different*
        // layout object, incompatible with that bind group at draw time.
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &vs_module,
            entry_point: Some("vertex_main"),
            compilation_options: PipelineCompilationOptions::default(),
            buffers: &[Some(VertexBufferLayout {
                array_stride: std::mem::size_of::<PreviewVertex>() as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute {
                        format: VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    VertexAttribute {
                        format: VertexFormat::Float32x3,
                        offset: 12,
                        shader_location: 1,
                    },
                    VertexAttribute {
                        format: VertexFormat::Float32x2,
                        offset: 24,
                        shader_location: 2,
                    },
                ],
            })],
        },
        fragment: Some(FragmentState {
            module: &fs_module,
            entry_point: Some("fragment_main"),
            compilation_options: PipelineCompilationOptions::default(),
            targets: &[Some(ColorTargetState {
                format: *target_format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: Some(Face::Back),
            unclipped_depth: false,
            polygon_mode: PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let (pipeline, error) = match smol::block_on(error_scope.pop()) {
        Some(error) => {
            tracing::error!("Material preview pipeline rejected shader: {error}");
            (None, Some(error.to_string()))
        }
        None => (Some(pipeline), None),
    };
    ShaderBuild {
        pipeline,
        error,
        pipeline_layout,
        texture_bind_group_layout,
        texture_bind_group,
        texture_assets,
    }
}

impl PreviewRenderer {
    pub fn new() -> Self {
        Self {
            device: None,
            queue: None,
            pipeline: None,
            pipeline_layout: None,
            uniform_buffer: None,
            uniform_bind_group_layout: None,
            bind_group: None,
            texture_bind_group_layout: None,
            texture_bind_group: None,
            _texture_assets: Vec::new(),
            mesh_vertex_buffer: None,
            mesh_index_buffer: None,
            mesh_index_count: 0,
            sky_pipeline: None,
            sky_uniform_buffer: None,
            sky_bind_group: None,
            plain_black_background: false,
            camera: OrbitCamera {
                yaw: 0.0,
                pitch: 0.4,
                distance: 3.0,
                target: [0.0, 0.0, 0.0],
                fov_y: 45.0_f32.to_radians(),
                aspect: 1.0,
                near: 0.1,
                far: 100.0,
            },
            shader_module: None,
            surface_config: None,
            needs_recompile: true,
            needs_upload: true,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn initialize(&mut self, device: &Device, queue: &Queue, config: &SurfaceConfiguration) {
        self.device = Some(device.clone());
        self.queue = Some(queue.clone());
        self.surface_config = Some(config.clone());
        self.camera.aspect = config.width as f32 / config.height as f32;

        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("preview uniforms"),
            size: std::mem::size_of::<PreviewUniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.uniform_buffer = Some(uniform_buffer);

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("preview bind group layout"),
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
        self.uniform_bind_group_layout = Some(bind_group_layout.clone());

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("preview pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("preview bind group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_ref().unwrap().as_entire_binding(),
            }],
        });
        self.bind_group = Some(bind_group);
        self.pipeline_layout = Some(pipeline_layout);

        // Sky pass: its own uniform buffer/bind group/pipeline so it stays
        // independent of whatever bind group layout the compiled material
        // shader derives.
        let sky_uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("sky uniforms"),
            size: std::mem::size_of::<SkyUniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sky_bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("sky bind group layout"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let sky_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("sky bind group"),
            layout: &sky_bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: sky_uniform_buffer.as_entire_binding(),
            }],
        });

        let sky_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("sky pipeline layout"),
            bind_group_layouts: &[Some(&sky_bind_group_layout)],
            immediate_size: 0,
        });

        let sky_module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: ShaderSource::Wgsl(SKY_SHADER_SRC.into()),
        });

        let sky_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("sky pipeline"),
            layout: Some(&sky_pipeline_layout),
            vertex: VertexState {
                module: &sky_module,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(FragmentState {
                module: &sky_module,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format: config.format,
                    blend: Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        self.sky_uniform_buffer = Some(sky_uniform_buffer);
        self.sky_bind_group = Some(sky_bind_group);
        self.sky_pipeline = Some(sky_pipeline);
    }

    /// What a shader build needs from the renderer. Everything here is a
    /// cheap, `Send` handle, so the build can run on a worker thread while
    /// the UI keeps drawing the previous pipeline.
    pub fn shader_build_inputs(&self) -> Option<ShaderBuildInputs> {
        Some(ShaderBuildInputs {
            device: self.device.clone()?,
            queue: self.queue.clone()?,
            uniform_layout: self.uniform_bind_group_layout.clone()?,
            target_format: self
                .surface_config
                .as_ref()
                .map(|c| c.format)
                .unwrap_or(TextureFormat::Bgra8Unorm),
        })
    }

    /// Build and install a shader on the calling thread. Used where blocking
    /// is fine (thumbnails); the interactive preview builds with
    /// [`build_shader`] on a worker and installs with [`Self::apply_shader`].
    pub fn update_shader(&mut self, wgsl_source: &str) {
        if let Some(inputs) = self.shader_build_inputs() {
            self.apply_shader(build_shader(&inputs, wgsl_source));
        }
        self.needs_recompile = false;
    }

    /// Install a finished [`ShaderBuild`]. A shader the GPU rejected keeps
    /// the previous pipeline on screen.
    pub fn apply_shader(&mut self, build: ShaderBuild) {
        self._texture_assets = build.texture_assets;
        self.texture_bind_group_layout = build.texture_bind_group_layout;
        self.texture_bind_group = build.texture_bind_group;
        self.pipeline_layout = Some(build.pipeline_layout);
        if let Some(pipeline) = build.pipeline {
            self.pipeline = Some(pipeline);
        }
        self.needs_recompile = false;
    }

    pub fn update_mesh(&mut self, vertices: &[PreviewVertex], indices: &[u32], index_count: u32) {
        if let (Some(device), Some(queue)) = (&self.device, &self.queue) {
            let vertex_size = std::mem::size_of_val(vertices) as u64;
            let index_size = std::mem::size_of_val(indices) as u64;

            let vertex_buffer = device.create_buffer(&BufferDescriptor {
                label: Some("preview vertex buffer"),
                size: vertex_size.max(1),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(vertices));

            let index_buffer = device.create_buffer(&BufferDescriptor {
                label: Some("preview index buffer"),
                size: index_size.max(1),
                usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(indices));

            self.mesh_vertex_buffer = Some(vertex_buffer);
            self.mesh_index_buffer = Some(index_buffer);
            self.mesh_index_count = index_count;
        }
        self.needs_upload = false;
    }

    pub fn update_camera(&mut self, aspect: f32) {
        self.camera.aspect = aspect;
    }

    /// Use a solid black backdrop for generated asset thumbnails.
    pub fn set_plain_black_background(&mut self, enabled: bool) {
        self.plain_black_background = enabled;
    }

    pub fn render(&self, output: &TextureView) {
        self.render_at_time(output, self.start_time.elapsed().as_secs_f32());
    }

    /// Render with an explicit shader time so saved thumbnails can capture a deterministic first frame.
    pub fn render_at_time(&self, output: &TextureView, elapsed: f32) {
        let Some(device) = &self.device else { return };
        let Some(queue) = &self.queue else { return };
        let sky_pipeline = self.sky_pipeline.as_ref();
        let sky_uniform_buffer = self.sky_uniform_buffer.as_ref();
        let sky_bind_group = self.sky_bind_group.as_ref();

        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix();

        // Column-major matrices (each inner array is a column, matching
        // WGSL's `mat4x4<f32>` layout): for `clip = proj * (view * world)`,
        // the combined matrix is `proj * view`, which in this storage
        // convention is computed as `view[i][k] * proj[k][j]` — swapping the
        // operands here previously produced `view * proj`, which put the
        // camera transform backwards (looked like the camera was inside the
        // mesh).
        let mut view_proj = [[0.0_f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    view_proj[i][j] += view[i][k] * proj[k][j];
                }
            }
        }

        let (right, up, forward) = self.camera.basis_vectors();
        let sky_uniforms = SkyUniforms {
            camera_right: [right[0], right[1], right[2], 0.0],
            camera_up: [up[0], up[1], up[2], 0.0],
            camera_forward: [forward[0], forward[1], forward[2], 0.0],
            params: [
                (self.camera.fov_y * 0.5).tan(),
                self.camera.aspect,
                elapsed,
                0.0,
            ],
        };
        if !self.plain_black_background {
            let (Some(sky_uniform_buffer), Some(_), Some(_)) =
                (sky_uniform_buffer, sky_pipeline, sky_bind_group)
            else {
                return;
            };
            queue.write_buffer(sky_uniform_buffer, 0, bytemuck::bytes_of(&sky_uniforms));
        }

        // Only draw the material mesh once both the compiled-shader pipeline
        // and the mesh geometry are uploaded; the sky still renders on its
        // own otherwise, so the surface is never left blank/grey.
        let mesh_ready = self.pipeline.is_some()
            && self.uniform_buffer.is_some()
            && self.bind_group.is_some()
            && self.mesh_vertex_buffer.is_some()
            && self.mesh_index_buffer.is_some()
            && self.mesh_index_count > 0;

        if mesh_ready {
            let model: [[f32; 4]; 4] = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ];

            let uniforms = PreviewUniforms {
                view_proj,
                model,
                time: elapsed,
                _padding: [0.0; 3],
            };
            queue.write_buffer(
                self.uniform_buffer.as_ref().unwrap(),
                0,
                bytemuck::bytes_of(&uniforms),
            );
        }

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("preview encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("preview render pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(if self.plain_black_background {
                            Color::BLACK
                        } else {
                            Color {
                                r: 0.1,
                                g: 0.1,
                                b: 0.1,
                                a: 1.0,
                            }
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Keep the editor preview's sky, but make generated thumbnails
            // use the same predictable solid black used by the asset browser.
            if !self.plain_black_background {
                let (Some(sky_pipeline), Some(sky_bind_group)) = (sky_pipeline, sky_bind_group)
                else {
                    return;
                };
                rpass.set_pipeline(sky_pipeline);
                rpass.set_bind_group(0, sky_bind_group, &[]);
                rpass.draw(0..3, 0..1);
            }

            if mesh_ready {
                let pipeline = self.pipeline.as_ref().unwrap();
                let bind_group = self.bind_group.as_ref().unwrap();
                let vertex_buffer = self.mesh_vertex_buffer.as_ref().unwrap();
                let index_buffer = self.mesh_index_buffer.as_ref().unwrap();

                rpass.set_pipeline(pipeline);
                rpass.set_bind_group(0, bind_group, &[]);
                if let Some(texture_bind_group) = &self.texture_bind_group {
                    rpass.set_bind_group(1, texture_bind_group, &[]);
                }
                rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
                rpass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
                rpass.draw_indexed(0..self.mesh_index_count, 0, 0..1);
            }
        }

        queue.submit(Some(encoder.finish()));
    }

    pub fn has_material_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    pub fn resize(&mut self, width: u32, height: u32, config: &SurfaceConfiguration) {
        self.surface_config = Some(config.clone());
        self.camera.aspect = width as f32 / height as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_FRAGMENT: &str = r#"
struct FragmentInput {
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
};
@fragment
fn fragment_main(input: FragmentInput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.uv, 0.0, 1.0);
}
"#;

    /// A type error, as a half-edited graph can produce.
    const BAD_FRAGMENT: &str = r#"
@fragment
fn fragment_main() -> @location(0) vec4<f32> {
    return vec4<f32>("not a number");
}
"#;

    fn inputs() -> ShaderBuildInputs {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = smol::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
            .expect("GPU adapter required");
        let (device, queue) =
            smol::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
        let uniform_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
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
        ShaderBuildInputs {
            device,
            queue,
            uniform_layout,
            target_format: TextureFormat::Bgra8Unorm,
        }
    }

    /// The build runs on a worker thread (so the editor never waits on shader
    /// compilation), and a shader the GPU rejects is reported as "no
    /// pipeline" instead of reaching wgpu's panicking error handler.
    #[test]
    fn shader_builds_run_off_thread_and_rejected_shaders_do_not_panic() {
        let inputs = inputs();
        let caller = std::thread::current().id();

        let worker_inputs = inputs.clone();
        let (worker, good) = std::thread::spawn(move || {
            (
                std::thread::current().id(),
                build_shader(&worker_inputs, GOOD_FRAGMENT),
            )
        })
        .join()
        .expect("a valid shader builds on a worker");
        assert_ne!(worker, caller);
        assert!(good.pipeline.is_some(), "a valid shader yields a pipeline");

        let bad = std::thread::spawn(move || build_shader(&inputs, BAD_FRAGMENT))
            .join()
            .expect("a rejected shader must not panic the worker");
        assert!(bad.pipeline.is_none(), "a rejected shader yields no pipeline");
        assert!(good.error.is_none());
        assert!(bad.error.is_some(), "a rejected shader reports why");
    }
}

/// The largest side, in texels, a preview texture keeps.
const PREVIEW_MAX_TEXTURE: u32 = 1024;

/// `image` scaled down (aspect kept) so its larger side is at most
/// [`PREVIEW_MAX_TEXTURE`]; smaller images are returned as they are.
fn fit_preview_texture(image: image::DynamicImage) -> image::DynamicImage {
    let (width, height) = (image.width(), image.height());
    let longest = width.max(height);
    if longest <= PREVIEW_MAX_TEXTURE {
        return image;
    }
    let scale = f64::from(PREVIEW_MAX_TEXTURE) / f64::from(longest);
    image.resize_exact(
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

#[cfg(test)]
mod preview_texture_tests {
    use super::*;

    #[test]
    fn large_preview_textures_are_scaled_down_and_small_ones_kept() {
        let big = image::DynamicImage::new_rgb8(4096, 2048);
        let fitted = fit_preview_texture(big);
        assert_eq!((fitted.width(), fitted.height()), (1024, 512));
        let small = image::DynamicImage::new_rgb8(300, 200);
        let kept = fit_preview_texture(small);
        assert_eq!((kept.width(), kept.height()), (300, 200));
    }
}
