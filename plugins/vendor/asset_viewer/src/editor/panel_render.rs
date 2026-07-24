use gpui::prelude::*;
use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::h_flex;

use super::panel::AssetViewerPanel;

static WIRE_VERTEX_SRC: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};
@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = uniforms.view_proj * vec4(position, 1.0);
    return out;
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4(0.0, 0.74, 0.83, 1.0);
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
    fn rebuild_surface(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if !self.needs_rebuild {
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

        self.setup_wireframe_pipeline(&device, &config);
        self.setup_quad_pipeline(&device, &queue, &config);
        self.upload_wireframe_mesh(&device, &queue);
        self.upload_image_texture(&device, &queue);

        self.needs_rebuild = false;
    }

    fn setup_wireframe_pipeline(
        &mut self,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire uniform buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.wire_uniform_buffer = Some(uniform_buffer);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wire bind group layout"),
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
        self.bind_group_layout = Some(bind_group_layout);

        let uniform_buffer_ref = self.wire_uniform_buffer.as_ref().unwrap();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wire bind group"),
            layout: self.bind_group_layout.as_ref().unwrap(),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer_ref.as_entire_binding(),
            }],
        });
        self.wire_bind_group = Some(bind_group);

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire vertex"),
            source: wgpu::ShaderSource::Wgsl(WIRE_VERTEX_SRC.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wire pipeline layout"),
            bind_group_layouts: &[Some(self.bind_group_layout.as_ref().unwrap())],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
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
                topology: wgpu::PrimitiveTopology::LineList,
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
        self.wire_pipeline = Some(pipeline);
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

        let vertices: [[f32; 4]; 4] = [
            [-1.0, -1.0, 0.0, 1.0],
            [1.0, -1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 0.0],
        ];
        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad vertex buffer"),
            size: std::mem::size_of_val(&vertices) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vb, 0, bytemuck::cast_slice(&vertices));
        self.quad_vertex_buffer = Some(vb);

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad vertex"),
            source: wgpu::ShaderSource::Wgsl(QUAD_VERTEX_SRC.into()),
        });
        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad fragment"),
            source: wgpu::ShaderSource::Wgsl(QUAD_FRAGMENT_SRC.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        self.quad_bind_group_layout = Some(bind_group_layout);

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
                    blend: Some(wgpu::BlendState::REPLACE),
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

    fn upload_wireframe_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let edges: [[f32; 3]; 24] = [
            [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0], [1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0],
            [-1.0, -1.0, 1.0], [-1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0], [1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0], [1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0], [-1.0, 1.0, -1.0],
            [-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0],
            [1.0, -1.0, -1.0], [1.0, 1.0, -1.0],
            [1.0, -1.0, 1.0], [1.0, 1.0, 1.0],
            [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0],
        ];
        let vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire vertex buffer"),
            size: std::mem::size_of_val(&edges) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vb, 0, bytemuck::cast_slice(&edges));
        self.wire_vertex_buffer = Some(vb);
        self.wire_index_count = 24;
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

    fn view_matrix(&self) -> [[f32; 4]; 4] {
        let (yaw_s, yaw_c) = (self.yaw.sin(), self.yaw.cos());
        let (pitch_s, pitch_c) = (self.pitch.sin(), self.pitch.cos());
        let eye = [
            self.distance * pitch_c * yaw_s,
            self.distance * pitch_s,
            self.distance * pitch_c * yaw_c,
        ];
        let forward = [-eye[0], -eye[1], -eye[2]];
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

    fn render_wireframe(&mut self) {
        let Some(device) = &self.device else { return };
        let Some(queue) = &self.queue else { return };
        let Some(surface) = &self.surface_handle else { return };
        let (width, height) = surface.size();
        if height == 0 {
            return;
        }

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

        if let Some(buf) = &self.wire_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&view_proj));
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("wire encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wire pass"),
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some(pipeline), Some(bg), Some(vb)) = (
                &self.wire_pipeline,
                &self.wire_bind_group,
                &self.wire_vertex_buffer,
            ) {
                rpass.set_pipeline(pipeline);
                rpass.set_bind_group(0, bg, &[]);
                rpass.set_vertex_buffer(0, vb.slice(..));
                rpass.draw(0..self.wire_index_count, 0..1);
            }
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

        let view = match surface.back_buffer_view() {
            Some(v) => v,
            None => return,
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("quad encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("quad pass"),
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bg, &[]);
            rpass.set_vertex_buffer(0, vb.slice(..));
            rpass.draw(0..4, 0..1);
        }

        queue.submit(Some(encoder.finish()));
        surface.swap_buffers();
    }

    fn render_content(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.needs_rebuild {
            return;
        }
        if self.surface_handle.is_none() {
            return;
        }
        if self.is_3d {
            self.render_wireframe();
        } else {
            self.render_quad();
        }
    }
}

impl Panel for AssetViewerPanel {
    fn panel_name(&self) -> &'static str {
        "Asset Viewer"
    }

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.current_path.clone()
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Asset Viewer")
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
}

impl Focusable for AssetViewerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for AssetViewerPanel {}

impl AssetViewerPanel {
    fn on_orbit_mouse_down(
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

    fn on_orbit_mouse_move(
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

    fn on_orbit_mouse_up(
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

    fn on_orbit_scroll(
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
}

impl Render for AssetViewerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_rebuild {
            self.rebuild_surface(window, cx);
        }

        self.render_content(window, cx);

        if self.is_3d {
            div()
                .size_full()
                .min_h(px(200.0))
                .bg(gpui::rgb(0x1a1a1a))
                .on_mouse_down(MouseButton::Right, Self::on_orbit_mouse_down(cx))
                .on_mouse_move(Self::on_orbit_mouse_move(cx))
                .on_mouse_up(MouseButton::Right, Self::on_orbit_mouse_up(cx))
                .on_mouse_up_out(MouseButton::Right, Self::on_orbit_mouse_up(cx))
                .on_scroll_wheel(Self::on_orbit_scroll(cx))
                .child(self.surface_element())
        } else {
            div()
                .size_full()
                .bg(gpui::rgb(0x1a1a1a))
                .child(self.surface_element())
        }
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
                        .child("Loading..."),
                )
                .into_any_element()
        }
    }
}
