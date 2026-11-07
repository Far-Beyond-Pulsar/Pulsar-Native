//! The material preview: the material drawn on the preview mesh with the
//! materials' preview program (`materials_component::preview_program`), so it
//! shows what the engine's drawers draw, over the editor's sky backdrop.

use super::camera::OrbitCamera;
use super::mesh::PreviewVertex;
use materials_component::graph_preview::GraphPreview;
use materials_component::preview_program::{self, GraphDraw, PreviewUniforms};
use wgpu::*;

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

pub struct PreviewRenderer {
    pub device: Option<Device>,
    pub queue: Option<Queue>,
    /// The material's pipeline and texture bind groups.
    draw: Option<GraphDraw>,
    /// The program's uniform block (group 3) and clock (group 0).
    uniform_buffer: Option<Buffer>,
    uniform_bind_group_layout: Option<BindGroupLayout>,
    bind_group: Option<BindGroup>,
    globals_layout: Option<BindGroupLayout>,
    globals_buffer: Option<Buffer>,
    globals_bind_group: Option<BindGroup>,
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
    globals_layout: BindGroupLayout,
    target_format: TextureFormat,
}

/// The GPU objects one compiled material produces.
pub struct ShaderBuild {
    draw: Option<GraphDraw>,
    /// Why the GPU rejected the program; `None` when `draw` was built.
    pub error: Option<String>,
}

/// Build the preview program for `compiled` (the materials' compiler's
/// output). Slow (shader compilation, texture decode); safe to call from any
/// thread, so the editor never waits on it. A program the GPU rejects comes
/// back as an error, never through wgpu's panicking handler.
pub fn build_shader(inputs: &ShaderBuildInputs, compiled: &GraphPreview) -> ShaderBuild {
    let context = preview_program::GraphContext {
        device: &inputs.device,
        queue: &inputs.queue,
        target_format: inputs.target_format,
        globals_layout: &inputs.globals_layout,
        uniform_layout: &inputs.uniform_layout,
        depth_format: None,
        front_face: FrontFace::Ccw,
    };
    let layout = VertexBufferLayout {
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
    };
    match preview_program::build(&context, compiled, &[Some(layout)]) {
        Ok(draw) => ShaderBuild {
            draw: Some(draw),
            error: None,
        },
        Err(error) => {
            tracing::error!("Material preview pipeline rejected the program: {error}");
            ShaderBuild {
                draw: None,
                error: Some(error),
            }
        }
    }
}

impl PreviewRenderer {
    pub fn new() -> Self {
        Self {
            device: None,
            queue: None,
            draw: None,
            uniform_buffer: None,
            uniform_bind_group_layout: None,
            bind_group: None,
            globals_layout: None,
            globals_buffer: None,
            globals_bind_group: None,
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

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("preview bind group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_ref().unwrap().as_entire_binding(),
            }],
        });
        self.bind_group = Some(bind_group);

        // The program's clock (the frame and the time graph `time` nodes read).
        let globals_layout = preview_program::globals_layout(device);
        let globals_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("preview clock"),
            size: preview_program::GLOBALS_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.globals_bind_group = Some(device.create_bind_group(&BindGroupDescriptor {
            label: Some("preview clock bind group"),
            layout: &globals_layout,
            entries: &[BindGroupEntry {
                binding: 1,
                resource: globals_buffer.as_entire_binding(),
            }],
        }));
        self.globals_layout = Some(globals_layout);
        self.globals_buffer = Some(globals_buffer);

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
            globals_layout: self.globals_layout.clone()?,
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
    pub fn update_shader(&mut self, compiled: &GraphPreview) {
        if let Some(inputs) = self.shader_build_inputs() {
            self.apply_shader(build_shader(&inputs, compiled));
        }
        self.needs_recompile = false;
    }

    /// Install a finished [`ShaderBuild`]. A shader the GPU rejected keeps
    /// the previous pipeline on screen.
    pub fn apply_shader(&mut self, build: ShaderBuild) {
        if let Some(draw) = build.draw {
            self.draw = Some(draw);
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
        let mesh_ready = self.draw.is_some()
            && self.globals_bind_group.is_some()
            && self.uniform_buffer.is_some()
            && self.bind_group.is_some()
            && self.mesh_vertex_buffer.is_some()
            && self.mesh_index_buffer.is_some()
            && self.mesh_index_count > 0;

        if mesh_ready {
            let uniforms = PreviewUniforms {
                view_proj,
                // Lit.
                render_mode: [0; 4],
                base_color: [1.0; 4],
            };
            queue.write_buffer(
                self.uniform_buffer.as_ref().unwrap(),
                0,
                bytemuck::bytes_of(&uniforms),
            );
            preview_program::write_globals_at(
                queue,
                self.globals_buffer.as_ref().unwrap(),
                0,
                elapsed,
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
                let draw = self.draw.as_ref().unwrap();
                let vertex_buffer = self.mesh_vertex_buffer.as_ref().unwrap();
                let index_buffer = self.mesh_index_buffer.as_ref().unwrap();

                rpass.set_pipeline(&draw.pipeline);
                rpass.set_bind_group(0, self.globals_bind_group.as_ref().unwrap(), &[]);
                rpass.set_bind_group(1, &draw.textures, &[]);
                rpass.set_bind_group(2, &draw.empty, &[]);
                rpass.set_bind_group(3, self.bind_group.as_ref().unwrap(), &[]);
                rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
                rpass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
                rpass.draw_indexed(0..self.mesh_index_count, 0, 0..1);
            }
        }

        queue.submit(Some(encoder.finish()));
    }

    pub fn has_material_pipeline(&self) -> bool {
        self.draw.is_some()
    }

    pub fn resize(&mut self, width: u32, height: u32, config: &SurfaceConfiguration) {
        self.surface_config = Some(config.clone());
        self.camera.aspect = width as f32 / height as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph that compiles: the default class's surface.
    fn good() -> GraphPreview {
        GraphPreview::default()
    }

    /// A type error, as a half-edited graph can produce.
    fn bad() -> GraphPreview {
        GraphPreview {
            snippet: "/*RADIANT_GRAPH_DECLARATIONS*/\nfn broken() -> f32 { return vec4<f32>(\"x\"); }\n/*RADIANT_GRAPH_BODY*/\n".into(),
            textures: Vec::new(),
        }
    }

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
        let globals_layout = preview_program::globals_layout(&device);
        ShaderBuildInputs {
            device,
            queue,
            uniform_layout,
            globals_layout,
            target_format: TextureFormat::Bgra8Unorm,
        }
    }

    /// The build runs on a worker thread (so the editor never waits on shader
    /// compilation), and a program the GPU rejects is reported as "no
    /// pipeline" instead of reaching wgpu's panicking error handler.
    #[test]
    fn shader_builds_run_off_thread_and_rejected_shaders_do_not_panic() {
        let inputs = inputs();
        let caller = std::thread::current().id();

        let worker_inputs = inputs.clone();
        let (worker, good) = std::thread::spawn(move || {
            (
                std::thread::current().id(),
                build_shader(&worker_inputs, &good()),
            )
        })
        .join()
        .expect("a valid material builds on a worker");
        assert_ne!(worker, caller);
        assert!(
            good.draw.is_some(),
            "a valid material yields a pipeline: {:?}",
            good.error
        );

        let bad = std::thread::spawn(move || build_shader(&inputs, &bad()))
            .join()
            .expect("a rejected program must not panic the worker");
        assert!(bad.draw.is_none(), "a rejected program yields no pipeline");
        assert!(good.error.is_none());
        assert!(bad.error.is_some(), "a rejected program reports why");
    }
}
