use glam::{Mat4, Vec3};
use helio_core::RenderGraph;
mod scene;
use helio_pass_hlfs::{HlfsConfig, HlfsPass};
use scene::TestScene;
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub struct Fixture {
    pub ambient: [f32; 3],
    pub publish_ray_frame: bool,
    pub transmission: Option<wgpu::Buffer>,
    /// A host projection's ray-traced scene for the next frame only, in
    /// place of the fixture's own TLAS (`helio_frame_api::rays::SceneRays`).
    pub rays: Option<helio_frame_api::rays::SceneRays>,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub scene: TestScene,
    pub graph: RenderGraph,
    pub width: u32,
    pub height: u32,
    views: Vec<wgpu::TextureView>,
    depth: wgpu::TextureView,
    target: wgpu::TextureView,
    timestamps: Option<wgpu::QuerySet>,
    shadow: Option<wgpu::TextureView>,
    velocity: Option<wgpu::TextureView>,
}

impl Fixture {
    pub async fn new(width: u32, height: u32) -> Self {
        Self::new_with_ray_queries(width, height, false).await
    }
    pub async fn new_rt(width: u32, height: u32) -> Self {
        Self::new_with_ray_queries(width, height, true).await
    }
    async fn new_with_ray_queries(width: u32, height: u32, rt: bool) -> Self {
        let mut descriptor = helio_core::wgpu_instance_descriptor().with_env();
        if rt {
            descriptor.backends = wgpu::Backends::VULKAN;
        }
        let instance = wgpu::Instance::new(descriptor);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("GPU adapter required");
        eprintln!("HLFS adapter: {:?}", adapter.get_info());
        let timing_features =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let timing = adapter.features().contains(timing_features);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                required_features: if timing {
                    timing_features
                } else {
                    wgpu::Features::empty()
                } | (adapter.features()
                    & wgpu::Features::RG11B10UFLOAT_RENDERABLE)
                    | if rt {
                        wgpu::Features::EXPERIMENTAL_RAY_QUERY
                    } else {
                        wgpu::Features::empty()
                    },
                experimental_features: if rt {
                    // Explicit GPU tests acknowledge the experimental ray API.
                    unsafe { wgpu::ExperimentalFeatures::enabled() }
                } else {
                    wgpu::ExperimentalFeatures::disabled()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let mut scene = TestScene::new(device.clone(), queue.clone());
        scene.width = width;
        scene.height = height;
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
        let proj = helio_core::depth::orthographic(-2.0, 2.0, -2.0, 2.0, 0.1, 10.0);
        scene.camera.update(helio_core::GpuCameraUniforms::new(
            view,
            proj,
            Vec3::new(0.0, 0.0, 3.0),
            0.1,
            10.0,
            0,
            [0.0; 2],
            proj * view,
        ));
        let make = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("HLFS fixture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let mut views = Vec::new();
        for color in [
            [0.7f32, 0.4, 0.2, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 0.6, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
        ] {
            let texture = make(
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let values: Vec<u16> = color
                .iter()
                .map(|v| half::f16::from_f32(*v).to_bits())
                .cycle()
                .take((width * height * 4) as usize)
                .collect();
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 8),
                    rows_per_image: Some(height),
                },
                texture.size(),
            );
            views.push(texture.create_view(&Default::default()));
        }
        let depth_texture = make(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let depth = depth_texture.create_view(&Default::default());
        let target = make(
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fixture plane depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(ortho_depth(3.0)),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        }
        queue.submit([encoder.finish()]);
        let mut graph = RenderGraph::new(&device, &queue);
        let timestamps = timing.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("HLFS frame timing"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        if let Some(q) = &timestamps {
            graph.add_pass(Box::new(Timestamp {
                query: q.clone(),
                index: 0,
            }));
        }
        graph.add_pass(Box::new(HlfsPass::new(
            &device,
            &queue,
            width,
            height,
            wgpu::TextureFormat::Rgba16Float,
        )));
        if let Some(q) = &timestamps {
            graph.add_pass(Box::new(Timestamp {
                query: q.clone(),
                index: 1,
            }));
        }
        graph.lock(width, height);
        Self {
            ambient: [0.03; 3],
            publish_ray_frame: true,
            rays: None,
            transmission: None,
            device,
            queue,
            scene,
            graph,
            width,
            height,
            views,
            depth,
            target,
            timestamps,
            shadow: None,
            velocity: None,
        }
    }
    pub fn material(&mut self, albedo: [f32; 4], orm: [f32; 4]) {
        for (index, color) in [(0, albedo), (2, orm)] {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("HLFS audit material"),
                size: wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let values: Vec<u16> = color
                .into_iter()
                .map(|value| half::f16::from_f32(value).to_bits())
                .cycle()
                .take((self.width * self.height * 4) as usize)
                .collect();
            self.queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.width * 8),
                    rows_per_image: Some(self.height),
                },
                texture.size(),
            );
            self.views[index] = texture.create_view(&Default::default());
        }
    }
    pub fn config(&mut self, config: HlfsConfig) {
        self.graph
            .find_pass_mut::<HlfsPass>()
            .unwrap()
            .set_config(&self.device, config);
    }
    pub fn lights(&mut self, lights: Vec<helio_light_api::GpuLight>) {
        self.scene.movable_light_count = lights.len() as u32;
        self.scene.movable_lights_generation += 1;
        self.scene.lights.set_data(lights);
    }
    pub fn frame(&mut self) {
        self.try_frame().unwrap();
    }
    pub fn try_frame(&mut self) -> helio_core::Result<()> {
        self.scene.flush();
        // The frame's ray-traced scene (`helio_frame_api::rays`): a host
        // projection's for this frame, or the fixture's own TLAS with its
        // transmission rows.
        let rays = self.rays.take();
        let (tlas, transmission) = match &rays {
            Some(rays) => (
                Some(&rays.tlas),
                rays.buffer(helio_frame_api::rays::RAY_TRANSMISSION),
            ),
            None => {
                let tlas = self
                    .publish_ray_frame
                    .then(|| self.scene.tlas_manager.tlas())
                    .flatten();
                (tlas, tlas.and(self.transmission.as_ref()))
            }
        };
        let mut resources = helio_core::ResourceRegistry::empty();
        resources.write(
            helio_core::resource_keys::render_environment(),
            helio_core::RenderEnvironment {
                clear_color: [0.0; 4],
                ambient_color: self.ambient,
                ambient_intensity: 1.0,
                tlas,
            },
            "Fixture",
        );
        if let Some(buffer) = transmission {
            resources.write(
                helio_core::ResourceKey::new(helio_frame_api::rays::RAY_TRANSMISSION),
                buffer,
                "Fixture",
            );
        }
        resources.write(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::GROUP),
            helio_core::ViewGroup::<4> {
                views: [
                    &self.views[0],
                    &self.views[1],
                    &self.views[2],
                    &self.views[3],
                ],
            },
            "Fixture",
        );
        resources.write(
            helio_core::ResourceKey::new(helio_frame_api::color::SCENE_COLOR),
            &self.views[4],
            "Fixture",
        );
        resources.write(
            helio_light_api::shadow_matrices(),
            helio_light_api::ShadowMatricesFrameData {
                desired_matrices: None,
                residency: None,
                budget: Default::default(),
                shadow_matrices: &self.scene.shadow_matrices.buffer,
                shadow_count: 6,
                per_caster_dirty_gen: [0; helio_light_api::MAX_SHADOW_CASTERS],
                movable_objects_generation: 0,
                caster_layout: None,
            },
            "Fixture",
        );
        if let Some(shadow) = &self.shadow {
            resources.write(
                helio_core::ResourceKey::new(helio_light_api::SHADOW_ATLAS),
                shadow,
                "Fixture",
            );
        }
        if let Some(velocity) = &self.velocity {
            resources.write(
                helio_core::ResourceKey::new(helio_frame_api::gbuffer::VELOCITY),
                velocity,
                "Fixture",
            );
        }
        self.graph
            .execute_with_registry(&self.scene, &self.target, &self.depth, &mut resources)?;

        self.scene.frame_count += 1;
        Ok(())
    }
    pub fn compact_output(&mut self) {
        let mut pass = HlfsPass::new(
            &self.device,
            &self.queue,
            self.width,
            self.height,
            HlfsPass::preferred_output_format(&self.device),
        );
        pass.set_config(&self.device, HlfsConfig::performance());
        self.graph = RenderGraph::new(&self.device, &self.queue);
        self.graph.add_pass(Box::new(pass));
        self.graph.lock(self.width, self.height);
    }
    pub fn read(&self) -> Vec<[f32; 3]> {
        let texture = self.graph.find_pass::<HlfsPass>().unwrap().output_texture();
        let pixel_bytes = texture.format().block_copy_size(None).unwrap();
        let stride = (self.width * pixel_bytes).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HLFS capture"),
            size: u64::from(stride * self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(self.height),
                },
            },
            texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let mut pixels = Vec::new();
        for row in data.chunks(stride as usize) {
            for pixel in row[..(self.width * pixel_bytes) as usize].chunks(pixel_bytes as usize) {
                if pixel_bytes == 4 {
                    let bits = u32::from_le_bytes(pixel.try_into().unwrap());
                    let decode = |v: u32, m: u32| {
                        let e = v >> m;
                        let fraction = v & ((1 << m) - 1);
                        if e == 0 {
                            fraction as f32 * 2.0f32.powi(-14 - m as i32)
                        } else {
                            (1.0 + fraction as f32 / (1 << m) as f32) * 2.0f32.powi(e as i32 - 15)
                        }
                    };
                    pixels.push([
                        decode(bits & 2047, 6),
                        decode((bits >> 11) & 2047, 6),
                        decode(bits >> 22, 5),
                    ]);
                    continue;
                }
                pixels.push(std::array::from_fn(|i| {
                    half::f16::from_bits(u16::from_le_bytes([pixel[2 * i], pixel[2 * i + 1]]))
                        .to_f32()
                }));
            }
        }
        pixels
    }
    pub fn milliseconds(&self) -> f64 {
        let query = self
            .timestamps
            .as_ref()
            .expect("GPU timestamps required for benchmark");
        let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.resolve_query_set(query, 0..2, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, 16);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = read.slice(..).get_mapped_range().unwrap();
        let start = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let end = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        (end - start) as f64 * self.queue.get_timestamp_period() as f64 / 1e6
    }
    pub fn stage_milliseconds(&self) -> [f64; 6] {
        let query = self
            .graph
            .find_pass::<HlfsPass>()
            .unwrap()
            .timing_query()
            .unwrap();
        let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 56,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 56,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.resolve_query_set(query, 0..7, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, 56);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = read.slice(..).get_mapped_range().unwrap();
        let ticks: &[u64] = bytemuck::cast_slice(&bytes);
        std::array::from_fn(|i| {
            (ticks[i + 1] - ticks[i]) as f64 * self.queue.get_timestamp_period() as f64 / 1e6
        })
    }
    pub fn constant_shadow(&mut self, depth: f32) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fixture shadow atlas"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for layer in 0..6 {
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(depth),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        }
        self.queue.submit([encoder.finish()]);
        self.shadow = Some(texture.create_view(&Default::default()));
        let matrix = Mat4::from_cols_array(&[
            0.1, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
        ]);
        self.scene.shadow_matrices.set_data(vec![
            helio_light_api::GpuShadowMatrix {
                atlas: [0.0; 4],
                policy: [0; 4],
                light_view_proj: matrix.to_cols_array()
            };
            6
        ]);
    }
    /// Rasterize a real world-space plane instead of supplying ideal CPU depth.
    /// This includes vertex arithmetic, clipping and hardware interpolation.
    pub fn raster_plane_depth(&mut self, view: glam::Mat4, proj: glam::Mat4, z: f32) {
        let source = r#"
            struct Camera { view:mat4x4<f32>, proj:mat4x4<f32> }
            @group(0) @binding(0) var<uniform> camera:Camera;
            struct Vertex { @invariant @builtin(position) position:vec4<f32>, @location(0) view_z:f32 }
            @vertex fn vs(@builtin(vertex_index) i:u32)->Vertex {
                let p=array<vec2<f32>,3>(vec2<f32>(-500.0,-500.0),vec2<f32>(500.0,-500.0),vec2<f32>(0.0,500.0));
                let view=camera.view*vec4<f32>(p[i],PLANE_Z,1.0);
                return Vertex(camera.proj*view,view.z);
            }
            @fragment fn fs(v:Vertex)->@location(0) vec4<f32> {
                let reconstructed=camera.proj[3].z/(camera.proj[2].w*v.position.z-camera.proj[2].z);
                return vec4<f32>(0.0,0.0,v.view_z-reconstructed,2.0);
            }
        "#.replace("PLANE_Z", &format!("{z:.9}"));
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Rasterized receiver depth"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[view.to_cols_array(), proj.to_cols_array()]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Rasterized receiver depth"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let correction = self
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Raster depth residual"),
                size: wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &correction,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.velocity = Some(correction);
    }

    pub fn depth_values(&mut self, values: &[f32]) {
        assert_eq!(values.len(), (self.width * self.height) as usize);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fixture depth source"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            texture.size(),
        );
        let shader=self.device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("Fixture depth write"),source:wgpu::ShaderSource::Wgsl(r#"
            @group(0) @binding(0) var depths:texture_2d<f32>;
            @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
                let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
                return vec4<f32>(p[i],0.0,1.0);
            }
            @fragment fn fs(@builtin(position) p:vec4<f32>)->@builtin(frag_depth) f32 { return textureLoad(depths,vec2<i32>(p.xy),0).r; }
        "#.into())});
        let bgl = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        let view = texture.create_view(&Default::default());
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }
}

struct Timestamp {
    query: wgpu::QuerySet,
    index: u32,
}
impl helio_core::RenderPass for Timestamp {
    fn name(&self) -> &'static str {
        if self.index == 0 {
            "HLFS timestamp start"
        } else {
            "HLFS timestamp end"
        }
    }
    fn render_pass_descriptor<'a>(
        &'a self,
        _: &'a wgpu::TextureView,
        _: &'a wgpu::TextureView,
        _: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }
    fn execute(&mut self, ctx: &mut helio_core::PassContext) -> helio_core::Result<()> {
        // The lighting dispatches run on the render encoder after the GBuffer.
        ctx.graphics_cmds().write_timestamp(&self.query, self.index);
        Ok(())
    }
}

/// Depth of a surface `distance` in front of the fixture's orthographic
/// camera (near 0.1, far 10, reversed: the near plane is 1).
pub fn ortho_depth(distance: f32) -> f32 {
    (10.0 - distance) / (10.0 - 0.1)
}

pub fn point(position: [f32; 3], color: [f32; 3], intensity: f32) -> helio_light_api::GpuLight {
    helio_light_api::GpuLight {
        position_range: [position[0], position[1], position[2], 20.0],
        color_intensity: [color[0], color[1], color[2], intensity],
        ..Default::default()
    }
}
pub fn mean(pixels: &[[f32; 3]]) -> f32 {
    pixels.iter().flatten().sum::<f32>() / (pixels.len() * 3) as f32
}
