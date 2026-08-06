use gpui::prelude::*;
use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::{h_flex, v_flex, ActiveTheme};

use solid_rs::registry::Registry;
use solid_fbx::FbxLoader;

use super::panel::{AssetViewerPanel, MeshProps, SceneStats};

static MESH_VERTEX_SRC: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
};
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = uniforms.view_proj * vec4(input.position, 1.0);
    out.world_normal = input.normal;
    return out;
}

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>) -> @location(0) vec4<f32> {
    let n = normalize(world_normal);
    let light_dir = normalize(vec3(0.5, 1.0, 0.8));
    let diffuse = max(dot(n, light_dir), 0.0);
    let ambient = 0.3;
    let intensity = ambient + diffuse * 0.7;
    return vec4(vec3(intensity * 0.74, intensity * 0.83, intensity), 1.0);
}
"#;

static CHECKER_VERTEX_SRC: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let ix = i32(vi);
    let x = f32(ix & 1) * 2.0 - 1.0;
    let y = f32(ix >> 1) * 2.0 - 1.0;
    return vec4<f32>(x, -y, 0.0, 1.0);
}
"#;

static CHECKER_FRAGMENT_SRC: &str = r#"
struct Uniforms {
    viewport_size: vec2<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@fragment
fn fs_main(@builtin(position) coord: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = 8.0;
    let cx = floor(coord.x / cell);
    let cy = floor(coord.y / cell);
    let c = (cx + cy) % 2.0;
    let gray = mix(0.15, 0.25, c);
    return vec4(gray, gray, gray, 1.0);
}
"#;

static QUAD_VERTEX_SRC: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}
"#;

static QUAD_FRAGMENT_SRC: &str = r#"
@group(0) @binding(0) var texture: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(texture, tex_sampler, uv);
}
"#;

impl AssetViewerPanel {
    pub(crate) fn init_surface(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if self.surface_handle.is_some() {
            return;
        }

        let size = window.bounds().size;
        let width = (size.width.to_f64() as u32).max(1);
        let height = (size.height.to_f64() as u32).max(1);

        let Some(surface) =
            window.create_wgpu_surface(width, height, wgpu::TextureFormat::Bgra8Unorm)
        else {
            return;
        };

        let device = surface.device().clone();
        let queue = surface.queue().clone();

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };

        self.device = Some(device.clone());
        self.queue = Some(queue.clone());
        self.surface_config = Some(config.clone());
        self.surface_handle = Some(surface);

        if !self.is_3d {
            if let Some((img_w, img_h, _)) = self.image_data {
                let fit = (width as f32 / img_w as f32).min(height as f32 / img_h as f32);
                self.zoom = 1.0;
                self.pan_x = ((width as f32 - img_w as f32 * fit) * 0.5).max(0.0);
                self.pan_y = ((height as f32 - img_h as f32 * fit) * 0.5).max(0.0);
            }
            self.setup_quad_pipeline(&device, &queue, &config);
            self.setup_checker_pipeline(&device, &config);
            self.upload_image_texture(&device, &queue);
        } else {
            let (sw, sh) = self.surface_handle.as_ref().unwrap().size();
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth buffer"),
                size: wgpu::Extent3d { width: sw.max(1), height: sh.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let dv = depth.create_view(&wgpu::TextureViewDescriptor::default());
            self.depth_texture = Some(depth);
            self.depth_view = Some(dv);

            self.setup_mesh_pipeline(&device, &config);
            self.load_and_upload_mesh(&device, &queue);
        }
        self.needs_rebuild = false;
    }

    fn setup_mesh_pipeline(
        &mut self,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh uniform buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.mesh_uniform_buffer = Some(uniform_buffer);

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_buf = self.mesh_uniform_buffer.as_ref().unwrap();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh bind group"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        self.mesh_bind_group = Some(bind_group);

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh vertex"),
            source: wgpu::ShaderSource::Wgsl(MESH_VERTEX_SRC.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &vs_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.mesh_pipeline = Some(pipeline);
    }

    fn setup_quad_pipeline(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("quad sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.quad_sampler = Some(sampler);

        let dummy: [[f32; 4]; 6] = [[0.0; 4]; 6];
        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad vertex buffer"),
            size: std::mem::size_of_val(&dummy) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vb, 0, bytemuck::cast_slice(&dummy));
        self.quad_vertex_buffer = Some(vb);

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad vertex"),
            source: wgpu::ShaderSource::Wgsl(QUAD_VERTEX_SRC.into()),
        });
        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad fragment"),
            source: wgpu::ShaderSource::Wgsl(QUAD_FRAGMENT_SRC.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        self.quad_bind_group_layout = Some(bgl);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad pipeline layout"),
            bind_group_layouts: &[Some(self.quad_bind_group_layout.as_ref().unwrap())],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.quad_pipeline = Some(pipeline);
    }

    fn setup_checker_pipeline(
        &mut self,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("checker uniform buffer"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.checker_uniform_buffer = Some(uniform_buffer);

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("checker bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_buf = self.checker_uniform_buffer.as_ref().unwrap();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("checker bind group"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        self.checker_bind_group = Some(bind_group);

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("checker vertex"),
            source: wgpu::ShaderSource::Wgsl(CHECKER_VERTEX_SRC.into()),
        });
        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("checker fragment"),
            source: wgpu::ShaderSource::Wgsl(CHECKER_FRAGMENT_SRC.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("checker pipeline layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        self.checker_bind_group_layout = Some(bgl);

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("checker pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.checker_pipeline = Some(pipeline);
    }

    fn load_and_upload_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(ref path) = self.current_path else { return };

        let mut registry = solid_rs::registry::Registry::new();
        registry.register_loader(solid_fbx::FbxLoader);

        let solid_scene = match registry.load_file(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to load FBX {:?}: {}", path, e);
                return;
            }
        };

        // Walk the node DAG to get the world-space transform for each mesh.
        let mut mesh_world = Vec::<(usize, [f32; 16])>::new();
        {
            let mut stack: Vec<(solid_rs::scene::NodeId, [f32; 16])> = solid_scene
                .roots
                .iter()
                .map(|&id| (id, glam::Mat4::IDENTITY.to_cols_array()))
                .collect();
            while let Some((node_id, parent_cols)) = stack.pop() {
                let Some(node) = solid_scene.node(node_id) else { continue };
                let node_mat = node.transform.to_matrix().to_cols_array();
                let mut prod = [0.0f32; 16];
                for col in 0..4 {
                    for row in 0..4 {
                        let mut sum = 0.0;
                        for k in 0..4 {
                            sum += node_mat[k * 4 + col] * parent_cols[row * 4 + k];
                        }
                        prod[row * 4 + col] = sum;
                    }
                }
                if let Some(mesh_idx) = node.mesh {
                    mesh_world.push((mesh_idx, prod));
                }
                for &child_id in &node.children {
                    stack.push((child_id, prod));
                }
            }
        }

        // First pass: collect all transformed vertices + compute bounding box
        struct MeshVert { px: f32, py: f32, pz: f32, nx: f32, ny: f32, nz: f32 }
        let mut all_verts: Vec<MeshVert> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut bbox_min = [f32::MAX; 3];
        let mut bbox_max = [f32::MIN; 3];

        for (mesh_idx, mesh) in solid_scene.meshes.iter().enumerate() {
            let world_mat = mesh_world
                .iter()
                .find(|(idx, _)| *idx == mesh_idx)
                .map(|(_, m)| *m)
                .unwrap_or(glam::Mat4::IDENTITY.to_cols_array());

            let rot: [[f32; 3]; 3] = [
                [world_mat[0], world_mat[4], world_mat[8]],
                [world_mat[1], world_mat[5], world_mat[9]],
                [world_mat[2], world_mat[6], world_mat[10]],
            ];

            let base = all_verts.len() as u32;
            for v in &mesh.vertices {
                let px = v.position.x * rot[0][0] + v.position.y * rot[0][1] + v.position.z * rot[0][2] + world_mat[12];
                let py = v.position.x * rot[1][0] + v.position.y * rot[1][1] + v.position.z * rot[1][2] + world_mat[13];
                let pz = v.position.x * rot[2][0] + v.position.y * rot[2][1] + v.position.z * rot[2][2] + world_mat[14];
                let (nx, ny, nz) = match v.normal {
                    Some(n) => (
                        n.x * rot[0][0] + n.y * rot[0][1] + n.z * rot[0][2],
                        n.x * rot[1][0] + n.y * rot[1][1] + n.z * rot[1][2],
                        n.x * rot[2][0] + n.y * rot[2][1] + n.z * rot[2][2],
                    ),
                    None => (0.0, 1.0, 0.0),
                };
                bbox_min[0] = bbox_min[0].min(px);
                bbox_min[1] = bbox_min[1].min(py);
                bbox_min[2] = bbox_min[2].min(pz);
                bbox_max[0] = bbox_max[0].max(px);
                bbox_max[1] = bbox_max[1].max(py);
                bbox_max[2] = bbox_max[2].max(pz);
                all_verts.push(MeshVert { px, py, pz, nx, ny, nz });
            }

            for prim in &mesh.primitives {
                if prim.topology != solid_rs::geometry::Topology::TriangleList {
                    continue;
                }
                for i in &prim.indices {
                    indices.push(base + i);
                }
            }
        }

        // Compute center and scale to normalize mesh to unit size
        let center = [
            (bbox_min[0] + bbox_max[0]) * 0.5,
            (bbox_min[1] + bbox_max[1]) * 0.5,
            (bbox_min[2] + bbox_max[2]) * 0.5,
        ];
        let extent = [
            bbox_max[0] - bbox_min[0],
            bbox_max[1] - bbox_min[1],
            bbox_max[2] - bbox_min[2],
        ];
        let max_extent = extent[0].max(extent[1]).max(extent[2]).max(1e-6);

        let scale = 1.0 / max_extent;
        self.orbit_target = [0.0, 0.0, 0.0];
        self.distance = 2.0;
        let mut verts: Vec<f32> = Vec::with_capacity(all_verts.len() * 6);
        for mv in &all_verts {
            verts.push((mv.px - center[0]) * scale);
            verts.push((mv.py - center[1]) * scale);
            verts.push((mv.pz - center[2]) * scale);
            verts.push(mv.nx);
            verts.push(mv.ny);
            verts.push(mv.nz);
        }

        // Build per-mesh properties and scene stats
        let mut total_verts: u32 = 0;
        let mut total_indices_scene: u32 = 0;
        let mut morph_total = 0;

        self.mesh_props = solid_scene.meshes.iter().map(|mesh| {
            let prim_indices: u32 = mesh.primitives.iter()
                .filter(|p| p.topology == solid_rs::geometry::Topology::TriangleList)
                .map(|p| p.indices.len() as u32)
                .sum();
            let mat_name = mesh.primitives.first()
                .and_then(|p| p.material_index)
                .and_then(|mi| solid_scene.materials.get(mi))
                .map(|m| m.name.clone())
                .unwrap_or_default();
            let bb = mesh.bounds.as_ref().map(|b| ([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]))
                .unwrap_or(([0.0; 3], [0.0; 3]));
            morph_total += mesh.morph_targets.len();

            total_verts += mesh.vertices.len() as u32;
            total_indices_scene += prim_indices;

            MeshProps {
                name: mesh.name.clone(),
                vertex_count: mesh.vertices.len() as u32,
                index_count: prim_indices,
                triangle_count: prim_indices / 3,
                primitive_count: mesh.primitives.len(),
                morph_count: mesh.morph_targets.len(),
                has_normals: mesh.vertices.iter().any(|v| v.normal.is_some()),
                has_tangents: mesh.vertices.iter().any(|v| v.tangent.is_some()),
                has_uvs: mesh.vertices.iter().any(|v| v.uvs[0].is_some()),
                has_vertex_colors: mesh.vertices.iter().any(|v| v.colors.iter().any(|c| c.is_some())),
                has_skin: mesh.vertices.iter().any(|v| v.skin_weights.is_some()),
                material_name: mat_name,
                bounds_min: bb.0,
                bounds_max: bb.1,
            }
        }).collect();

        self.scene_stats = SceneStats {
            name: solid_scene.name.clone(),
            generator: solid_scene.metadata.generator.clone().unwrap_or_default(),
            mesh_count: solid_scene.meshes.len(),
            total_vertices: total_verts,
            total_indices: total_indices_scene,
            material_count: solid_scene.materials.len(),
            texture_count: solid_scene.textures.len(),
            image_count: solid_scene.images.len(),
            light_count: solid_scene.lights.len(),
            camera_count: solid_scene.cameras.len(),
            animation_count: solid_scene.animations.len(),
            skin_count: solid_scene.skins.len(),
            morph_target_count: morph_total,
            has_skin: solid_scene.skins.len() > 0,
            has_animations: solid_scene.animations.len() > 0,
            total_joints: solid_scene.skins.iter().map(|s| s.joints.len()).sum(),
            meshes: self.mesh_props.clone(),
        };

        if verts.is_empty() || indices.is_empty() {
            log::error!("FBX {:?} has no renderable triangles", path);
            return;
        }

        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh vertex buffer"),
            size: (verts.len() * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vb, 0, bytemuck::cast_slice(&verts));

        let ib = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh index buffer"),
            size: (indices.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ib, 0, bytemuck::cast_slice(&indices));

        self.mesh_vertex_buffer = Some(vb);
        self.mesh_index_buffer = Some(ib);
        self.mesh_index_count = indices.len() as u32;

        log::info!(
            "Loaded FBX {:?}: {} verts, {} indices",
            path,
            verts.len() / 6,
            indices.len()
        );
    }

    pub fn reupload_texture(&mut self) {
        let Some(device) = &self.device else { return };
        let Some(queue) = &self.queue else { return };
        let Some((width, height, ref pixels)) = self.image_data.clone() else { return };
        let Some(bgl) = self.quad_bind_group_layout.as_ref() else { return };
        let Some(sampler) = self.quad_sampler.as_ref() else { return };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.quad_texture = Some(texture);
        if let (Some(tex), Some(sampler)) = (self.quad_texture.as_ref(), self.quad_sampler.as_ref()) {
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("quad bind group"),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            });
            self.quad_bind_group = Some(bind_group);
        }
        self.needs_rebuild = false;
    }

    fn upload_image_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some((width, height, ref pixels)) = self.image_data else {
            return;
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.quad_texture = Some(texture);

        if let (Some(bgl), Some(tex), Some(sampler)) = (
            self.quad_bind_group_layout.as_ref(),
            self.quad_texture.as_ref(),
            self.quad_sampler.as_ref(),
        ) {
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("quad bind group"),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            });
            self.quad_bind_group = Some(bind_group);
        }
    }

    fn update_camera(&mut self, dt: f32) {
        let (yaw_s, yaw_c) = (self.yaw.sin(), self.yaw.cos());
        let (pitch_s, pitch_c) = (self.pitch.sin(), self.pitch.cos());
        let fwd = [-pitch_c * yaw_s, -pitch_s, -pitch_c * yaw_c];
        let world_up = [0.0, 1.0, 0.0];
        let r = [
            world_up[1] * fwd[2] - world_up[2] * fwd[1],
            world_up[2] * fwd[0] - world_up[0] * fwd[2],
            world_up[0] * fwd[1] - world_up[1] * fwd[0],
        ];
        let rl = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        let right = [r[0] / rl, r[1] / rl, r[2] / rl];

        let speed = self.move_speed * dt;
        let k = &self.keys;
        if k[0] { // W - forward
            self.orbit_target[0] += fwd[0] * speed;
            self.orbit_target[1] += fwd[1] * speed;
            self.orbit_target[2] += fwd[2] * speed;
        }
        if k[2] { // S - backward
            self.orbit_target[0] -= fwd[0] * speed;
            self.orbit_target[1] -= fwd[1] * speed;
            self.orbit_target[2] -= fwd[2] * speed;
        }
        if k[1] { // A - left
            self.orbit_target[0] -= right[0] * speed;
            self.orbit_target[1] -= right[1] * speed;
            self.orbit_target[2] -= right[2] * speed;
        }
        if k[3] { // D - right
            self.orbit_target[0] += right[0] * speed;
            self.orbit_target[1] += right[1] * speed;
            self.orbit_target[2] += right[2] * speed;
        }
        if k[4] { // Space - up
            self.orbit_target[1] += speed;
        }
        if k[5] { // Ctrl - down
            self.orbit_target[1] -= speed;
        }
    }

    fn view_matrix(&self) -> [[f32; 4]; 4] {
        let (yaw_s, yaw_c) = (self.yaw.sin(), self.yaw.cos());
        let (pitch_s, pitch_c) = (self.pitch.sin(), self.pitch.cos());
        let eye = [
            self.orbit_target[0] + self.distance * pitch_c * yaw_s,
            self.orbit_target[1] + self.distance * pitch_s,
            self.orbit_target[2] + self.distance * pitch_c * yaw_c,
        ];
        let target = self.orbit_target;
        let forward = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
        let fwd_len =
            (forward[0] * forward[0] + forward[1] * forward[1] + forward[2] * forward[2]).sqrt();
        let fwd = [forward[0] / fwd_len, forward[1] / fwd_len, forward[2] / fwd_len];
        let world_up = [0.0, 1.0, 0.0];
        let right = [
            world_up[1] * fwd[2] - world_up[2] * fwd[1],
            world_up[2] * fwd[0] - world_up[0] * fwd[2],
            world_up[0] * fwd[1] - world_up[1] * fwd[0],
        ];
        let right_len =
            (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let r = [right[0] / right_len, right[1] / right_len, right[2] / right_len];
        let up = [
            fwd[1] * r[2] - fwd[2] * r[1],
            fwd[2] * r[0] - fwd[0] * r[2],
            fwd[0] * r[1] - fwd[1] * r[0],
        ];
        [
            [r[0], up[0], fwd[0], 0.0],
            [r[1], up[1], fwd[1], 0.0],
            [r[2], up[2], fwd[2], 0.0],
            [
                -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                -(fwd[0] * eye[0] + fwd[1] * eye[1] + fwd[2] * eye[2]),
                1.0,
            ],
        ]
    }

    fn projection_matrix(&self, aspect: f32) -> [[f32; 4]; 4] {
        let fov_y: f32 = 45.0_f32.to_radians();
        let f = 1.0 / (fov_y * 0.5).tan();
        let near = 0.1;
        let far = 100.0;
        let range_inv = 1.0 / (far - near);
        [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, (far + near) * range_inv, 1.0],
            [0.0, 0.0, -(far * near * 2.0) * range_inv, 0.0],
        ]
    }

    fn render_mesh(&mut self) {
        self.update_camera(1.0 / 60.0);
        let Some(device) = &self.device else { return };
        let Some(queue) = &self.queue else { return };
        let Some(surface) = &self.surface_handle else { return };
        let (width, height) = surface.size();
        if height == 0 {
            return;
        }
        let Some(vb) = &self.mesh_vertex_buffer else { return };
        let Some(ib) = &self.mesh_index_buffer else { return };
        if self.mesh_index_count == 0 { return; }
        let Some(pipeline) = &self.mesh_pipeline else { return };
        let Some(bg) = &self.mesh_bind_group else { return };

        let view = match surface.back_buffer_view() {
            Some(v) => v,
            None => return,
        };

        let aspect = width as f32 / height as f32;
        let view_mat = self.view_matrix();
        let proj_mat = self.projection_matrix(aspect);

        let mut view_proj = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    view_proj[i][j] += view_mat[i][k] * proj_mat[k][j];
                }
            }
        }

        if let Some(buf) = &self.mesh_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&view_proj));
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mesh encoder"),
        });

        let (cw, ch) = surface.size();
        let needs_depth = match &self.depth_texture {
            Some(t) => t.size().width != cw || t.size().height != ch,
            None => true,
        };
        if needs_depth {
            if let Some(device) = &self.device {
                let depth = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("depth buffer"),
                    size: wgpu::Extent3d { width: cw.max(1), height: ch.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth32Float,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let dv = depth.create_view(&wgpu::TextureViewDescriptor::default());
                self.depth_texture = Some(depth);
                self.depth_view = Some(dv);
            }
        }
        let depth_view = self.depth_view.as_ref();
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.1,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: depth_view.map(|dv| wgpu::RenderPassDepthStencilAttachment {
                    view: dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bg, &[]);
            rpass.set_vertex_buffer(0, vb.slice(..));
            rpass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rpass.draw_indexed(0..self.mesh_index_count, 0, 0..1);
        }

        queue.submit(Some(encoder.finish()));
        surface.swap_buffers();
    }

    fn render_quad(&mut self) {
        let Some(device) = &self.device else { return };
        let Some(queue) = &self.queue else { return };
        let Some(surface) = &self.surface_handle else { return };
        let Some(pipeline) = &self.quad_pipeline else { return };
        let Some(bg) = &self.quad_bind_group else { return };
        let Some(vb) = &self.quad_vertex_buffer else { return };
        let Some((img_w, img_h, _)) = self.image_data else { return };

        let (vp_w, vp_h) = surface.size();
        if vp_w == 0 || vp_h == 0 || img_w == 0 || img_h == 0 {
            return;
        }

        let iw = img_w as f32;
        let ih = img_h as f32;
        let vw = vp_w as f32;
        let vh = vp_h as f32;

        let fit = (vw / iw).min(vh / ih);
        let dw = iw * fit * self.zoom;  // displayed width in screen pixels
        let dh = ih * fit * self.zoom;  // displayed height in screen pixels

        // Quad corners in screen pixel space
        let l = self.pan_x;
        let t = self.pan_y;
        let r = self.pan_x + dw;
        let b = self.pan_y + dh;

        // Screen pixel → NDC
        let sx = |x: f32| (x / vw) * 2.0 - 1.0;
        let sy = |y: f32| -((y / vh) * 2.0 - 1.0);

        let verts: [[f32; 4]; 6] = [
            [sx(l), sy(b), 0.0, 1.0],  // bottom-left
            [sx(r), sy(b), 1.0, 1.0],  // bottom-right
            [sx(r), sy(t), 1.0, 0.0],  // top-right
            [sx(l), sy(b), 0.0, 1.0],  // bottom-left
            [sx(r), sy(t), 1.0, 0.0],  // top-right
            [sx(l), sy(t), 0.0, 0.0],  // top-left
        ];

        queue.write_buffer(vb, 0, bytemuck::cast_slice(&verts));

        let view = match surface.back_buffer_view() {
            Some(v) => v,
            None => return,
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("quad encoder"),
        });

        let checker_pipeline = self.checker_pipeline.as_ref();
        let checker_bg = self.checker_bind_group.as_ref();

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("quad pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Draw checkerboard background first
            if let (Some(cp), Some(cbg)) = (checker_pipeline, checker_bg) {
                if let Some(buf) = &self.checker_uniform_buffer {
                    let viewport_size: [f32; 2] = [vp_w as f32, vp_h as f32];
                    queue.write_buffer(buf, 0, bytemuck::bytes_of(&viewport_size));
                }
                rpass.set_pipeline(cp);
                rpass.set_bind_group(0, cbg, &[]);
                rpass.draw(0..4, 0..1);
            }

            // Draw image quad on top with alpha blending
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bg, &[]);
            rpass.set_vertex_buffer(0, vb.slice(..));
            rpass.draw(0..6, 0..1);
        }

        queue.submit(Some(encoder.finish()));
        surface.swap_buffers();
    }

    pub(crate) fn render_content(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.needs_rebuild {
            return;
        }
        if self.surface_handle.is_none() {
            return;
        }
        if self.is_3d {
            self.render_mesh();
        } else {
            self.render_quad();
        }
    }
}

impl Panel for AssetViewerPanel {
    fn panel_name(&self) -> &'static str {
        t!("AssetViewer.Title")
    }

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.current_path.clone()
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let name = self
            .tab_title
            .as_deref()
            .or_else(|| {
                self.current_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
            })
            .unwrap_or(t!("AssetViewer.Title"))
            .to_string();
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().child(name))
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut App) {}
}

impl Focusable for AssetViewerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for AssetViewerPanel {}

impl AssetViewerPanel {
    pub fn on_orbit_mouse_down(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.orbiting = true;
                panel.last_drag_pos = Some(event.position);
                cx.notify();
            });
        }
    }

    pub fn on_orbit_mouse_move(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseMoveEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                if !panel.orbiting {
                    return;
                }
                let Some(last) = panel.last_drag_pos else {
                    return;
                };
                let dx = (event.position.x - last.x).to_f64() as f32;
                let dy = (event.position.y - last.y).to_f64() as f32;
                panel.last_drag_pos = Some(event.position);
                panel.yaw += dx * 0.01;
                panel.pitch = (panel.pitch + dy * 0.01).clamp(-1.5, 1.5);
                cx.notify();
            });
        }
    }

    pub fn on_orbit_mouse_up(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |_event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.orbiting = false;
                panel.last_drag_pos = None;
                cx.notify();
            });
        }
    }

    pub fn on_pan_mouse_down(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.panning = true;
                panel.last_pan_pos = Some(event.position);
                cx.notify();
            });
        }
    }

    pub fn on_pan_mouse_move(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseMoveEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                if !panel.panning {
                    return;
                }
                let Some(last) = panel.last_pan_pos else { return };
                let dx = (event.position.x - last.x).to_f64() as f32;
                let dy = (event.position.y - last.y).to_f64() as f32;
                panel.pan_x += dx;
                panel.pan_y += dy;
                panel.last_pan_pos = Some(event.position);
                cx.notify();
            });
        }
    }

    pub fn on_pan_mouse_up(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |_event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.panning = false;
                panel.last_pan_pos = None;
                cx.notify();
            });
        }
    }

    pub fn on_image_scroll(
        cx: &mut Context<Self>,
    ) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                let delta_y = match event.delta {
                    ScrollDelta::Pixels(p) => p.y.to_f64() as f32,
                    ScrollDelta::Lines(l) => l.y * 20.0,
                };
                let factor = (1.0 - delta_y * 0.002).clamp(0.5, 1.5);
                let old_zoom = panel.zoom;
                let new_zoom = (old_zoom * factor).clamp(0.01, 100.0);

                let Some((img_w, img_h, _)) = panel.image_data else {
                    panel.zoom = new_zoom;
                    cx.notify();
                    return;
                };
                let (vp_w, vp_h) = panel
                    .surface_handle
                    .as_ref()
                    .map(|s| s.size())
                    .unwrap_or((1, 1));
                if vp_w > 0 && vp_h > 0 && img_w > 0 && img_h > 0 {
                    let mx = event.position.x.to_f64() as f32;
                    let my = event.position.y.to_f64() as f32;
                    let ratio = new_zoom / old_zoom;
                    panel.pan_x = mx + (panel.pan_x - mx) * ratio;
                    panel.pan_y = my + (panel.pan_y - my) * ratio;
                }
                panel.zoom = new_zoom;
                cx.notify();
            });
        }
    }

    pub fn on_orbit_scroll(
        cx: &mut Context<Self>,
    ) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                let delta_y = match event.delta {
                    ScrollDelta::Pixels(p) => p.y.to_f64() as f32,
                    ScrollDelta::Lines(l) => l.y * 20.0,
                };
                let factor = (1.0 - delta_y * 0.002).clamp(0.5, 1.5);
                panel.distance = (panel.distance * factor).clamp(0.2, 100.0);
                cx.notify();
            });
        }
    }

    pub fn on_key_down(
        cx: &mut Context<Self>,
    ) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                match event.keystroke.key.as_str() {
                    "w" | "W" => panel.keys[0] = true,
                    "a" | "A" => panel.keys[1] = true,
                    "s" | "S" => panel.keys[2] = true,
                    "d" | "D" => panel.keys[3] = true,
                    " " => panel.keys[4] = true,
                    "control" => panel.keys[5] = true,
                    _ => {}
                }
                cx.notify();
            });
        }
    }

    pub fn on_key_up(
        cx: &mut Context<Self>,
    ) -> impl Fn(&KeyUpEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                match event.keystroke.key.as_str() {
                    "w" | "W" => panel.keys[0] = false,
                    "a" | "A" => panel.keys[1] = false,
                    "s" | "S" => panel.keys[2] = false,
                    "d" | "D" => panel.keys[3] = false,
                    " " => panel.keys[4] = false,
                    "control" => panel.keys[5] = false,
                    _ => {}
                }
                cx.notify();
            });
        }
    }
}

impl Render for AssetViewerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.surface_handle.is_none() {
            self.init_surface(window, cx);
        }

        if self.workspace.is_none() {
            self.initialize_workspace(window, cx);
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(div().flex_1().min_h_0().map(|el| {
                if let Some(workspace) = &self.workspace {
                    el.child(workspace.clone())
                } else {
                    el.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("Initializing..."),
                    )
                }
            }))
    }
}

impl AssetViewerPanel {
    fn surface_element(&self) -> AnyElement {
        if let Some(surface) = &self.surface_handle {
            wgpu_surface(surface.clone())
                .defer_resize_until_mouse_up(true)
                .size_full()
                .into_any_element()
        } else {
            div()
                .size_full()
                .bg(gpui::rgb(0x1a1a1a))
                .child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(gpui::rgb(0x888888))
                        .child(t!("AssetViewer.Loading")),
                )
                .into_any_element()
        }
    }
}
