//! Billboard pass — camera-facing instanced quads.
//!
//! Each billboard is a 6-vertex quad (2 triangles) rendered with alpha blending.
//! Instance buffer is GPU-side; CPU uploads billboard data once per frame and issues
//! a single instanced draw call. O(1) CPU.

pub mod feature;
pub use feature::BillboardFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;

pub mod components;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub use components::{BillboardRow, BILLBOARD_INSTANCES};

const MAX_BILLBOARDS: u32 = 65536;

/// Per-billboard instance data uploaded to the GPU.
/// Compatibility name for the pass's SceneDB-owned instance record.
pub type BillboardInstance = BillboardRow;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BillboardGlobals {
    frame: u32,
    delta_time: f32,
    ambient_intensity: f32,
    _pad: f32,
}

pub struct BillboardPass {
    pipeline: wgpu::RenderPipeline,
    #[allow(dead_code)]
    bgl_0: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    bgl_1: wgpu::BindGroupLayout,
    bind_group_0: wgpu::BindGroup,
    bind_group_1: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    /// A one-instance binding for bind group 0 until it binds SceneDB's
    /// `"billboard_instances"` buffer; never drawn from (the pass is a graph
    /// member only while that buffer holds a live row).
    pub instance_buf: wgpu::Buffer,
    /// Epoch of the last-bound `"billboard_instances"` `BufferHandle` — a
    /// changed epoch means the SceneDB buffer reallocated (grew) and bind
    /// group 0 must be rebuilt to point at the new one.
    scene_binding_epoch: Option<u64>,
    quad_vertex_buf: wgpu::Buffer,
    pub instance_count: u32,
    occluded_by_geometry: bool,
    #[allow(dead_code)]
    white_texture: wgpu::Texture,
    #[allow(dead_code)]
    white_view: wgpu::TextureView,
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
}

impl BillboardPass {
    /// Create the billboard pass.
    ///
    /// - `camera_buf`    — camera uniform (must match `Camera` struct in billboard.wgsl)
    /// - `target_format` — colour attachment format (e.g. `Rgba16Float`)
    ///
    /// The pass resolves `BillboardRow`'s generated `"billboard_instances"`
    /// GPU buffer by key every frame (`ctx.scene_buffers`, see `prepare()`);
    /// there is no constructor argument or setter for a SceneDB handle — a
    /// frontend authors billboards purely by inserting `BillboardRow`
    /// rows into its `World`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_buf: &wgpu::Buffer,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = helio_core::shader::module(
            device,
            "Billboard Shader",
            helio_core::include_wgsl!("../shaders/billboard.wgsl"),
        );

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Billboard Globals"),
            size: std::mem::size_of::<BillboardGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ── Group 0: camera (b0) + globals (b1) ──────────────────────────────
        let bgl_0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Billboard BGL0"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // ── Group 1: sprite_tex (b0) + sprite_sampler (b1) ───────────────────
        let bgl_1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Billboard BGL1"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
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

        // ── 1×1 opaque-white default sprite ──────────────────────────────────
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Billboard White Texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        helio_core::upload::write_texture(
            queue,
            wgpu::TexelCopyTextureInfo {
                texture: &white_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
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
        let white_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Billboard Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Billboard BG1"),
            layout: &bgl_1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&white_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard PL"),
            bind_group_layouts: &[Some(&bgl_0), Some(&bgl_1)],
            immediate_size: 0,
        });

        // ── Quad vertex buffer (6 vertices: 2 triangles CCW) ─────────────────
        // Each vertex: [pos.x, pos.y, uv.x, uv.y]
        let quad_verts: [[f32; 4]; 6] = [
            [-0.5, -0.5, 0.0, 0.0],
            [0.5, -0.5, 1.0, 0.0],
            [-0.5, 0.5, 0.0, 1.0],
            [-0.5, 0.5, 0.0, 1.0],
            [0.5, -0.5, 1.0, 0.0],
            [0.5, 0.5, 1.0, 1.0],
        ];
        let quad_vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Billboard Quad VB"),
            size: std::mem::size_of_val(&quad_verts) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        helio_core::upload::write_buffer(
            queue,
            &quad_vertex_buf,
            0,
            bytemuck::cast_slice(&quad_verts),
        );

        // ── Instance buffer ───────────────────────────────────────────────────
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Billboard Instances"),
            size: std::mem::size_of::<BillboardInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Billboard BG0"),
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
                    binding: 2,
                    resource: instance_buf.as_entire_binding(),
                },
            ],
        });

        // ── Render pipeline ───────────────────────────────────────────────────
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Billboard Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    // Slot 0 only: the quad's per-vertex position/uv. Billboard
                    // instance data (world_pos/scale_flags/color) is read from
                    // the storage buffer bound at bind_group_0 binding 2
                    // (`instances[instance_index]` in the shader) -- there is no
                    // `@location`-based per-instance vertex input in `vs_main`
                    // (only `@builtin(vertex_index)`/`@builtin(instance_index)`),
                    // so a second, `Instance`-stepped vertex buffer slot here was
                    // stale: it made this pipeline require a vertex buffer at
                    // index 1 that `execute()` correctly never had a reason to
                    // bind, since nothing reads it.
                    buffers: &[
                        // Slot 0: per-vertex quad data  (stride 16)
                        Some(wgpu::VertexBufferLayout {
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
                        }),
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent::OVER,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None, // billboards are always camera-facing
                    ..Default::default()
                },
                // Depth read-only: billboards depth-test against scene geometry but don't write depth.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(helio_core::depth::NEARER_EQUAL),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        Self {
            pipeline,
            bgl_0,
            bgl_1,
            bind_group_0,
            bind_group_1,
            globals_buf,
            instance_buf,
            scene_binding_epoch: None,
            quad_vertex_buf,
            instance_count: 0,
            occluded_by_geometry: true,
            white_texture,
            white_view,
            sampler,
        }
    }

    pub fn set_occluded_by_geometry(&mut self, value: bool) {
        self.occluded_by_geometry = value;
    }

    /// Create the billboard pass with a custom sprite texture decoded from raw RGBA8 bytes.
    ///
    /// `rgba` must be `width * height * 4` bytes in row-major order.  If the slice is
    /// empty or dimensions are zero the pass falls back to the default white texture.
    ///
    /// In the standard editor pipeline `rgba` is decoded from `spotlight.png`.
    pub fn new_with_sprite_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_buf: &wgpu::Buffer,
        target_format: wgpu::TextureFormat,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        let mut pass = Self::new(device, queue, camera_buf, target_format);
        let expected = (width as usize) * (height as usize) * 4;
        if expected > 0 && rgba.len() >= expected && width > 0 && height > 0 {
            let sprite_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Billboard Sprite Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            helio_core::upload::write_texture(
                queue,
                wgpu::TexelCopyTextureInfo {
                    texture: &sprite_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba[..expected],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            let sprite_view = sprite_texture.create_view(&wgpu::TextureViewDescriptor::default());
            // Rebuild bind_group_1 pointing to the new sprite texture.
            pass.bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Billboard BG1 (Sprite)"),
                layout: &pass.bgl_1,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&sprite_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&pass.sampler),
                    },
                ],
            });
            // Replace the white texture/view with the sprite so it stays alive.
            pass.white_texture = sprite_texture;
            pass.white_view = sprite_view;
        }
        pass
    }
}

impl RenderPass for BillboardPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["billboard_instances"]
    }

    fn name(&self) -> &'static str {
        "Billboard"
    }

    /// A member while some `"billboard_instances"` row is live. Hosts
    /// register the column up front, so its presence alone says nothing.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("billboard_instances")
    }

    fn reads(&self) -> &'static [&'static str] {
        &["pre_aa", "full_res_depth", "billboards", "depth"]
    }

    fn writes(&self) -> &'static [&'static str] {
        // Draws (LoadOp::Load) directly onto pre_aa when occluded_by_geometry —
        // see render_pass_descriptor() below. Declaring this lets the render
        // graph see the real dependency for subpass fusion.
        &["pre_aa"]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("pre_aa");
        builder.read("full_res_depth");
        builder.read("billboards");
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // Resolve `BillboardRow`'s `"billboard_instances"` buffer by
        // key, generically, every frame — no renderer method, no stored
        // SceneDB handle. `None` until the first `BillboardRow` is
        // ever inserted anywhere in the frontend's World.
        match ctx.scene_buffers.get(BufferKey::of("billboard_instances")) {
            Some(handle) => {
                // Capacity, not a live count: unused rows are `Zeroable`
                // (`scale_flags` all zero), a zero-area billboard that
                // contributes nothing, so drawing every row needs no
                // per-frame CPU query. Draw the rows the buffer actually has
                // (instances past its end would read out of bounds); the
                // graph runs the pass only while some row is live.
                self.instance_count = match handle.row_capacity() {
                    0 => MAX_BILLBOARDS,
                    rows => rows.min(MAX_BILLBOARDS),
                };
                if self.scene_binding_epoch != Some(handle.epoch) {
                    self.bind_group_0 = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Billboard SceneDB BG0"),
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
                                binding: 2,
                                resource: handle.buffer.as_entire_binding(),
                            },
                        ],
                    });
                    self.scene_binding_epoch = Some(handle.epoch);
                }
            }
            None => {
                self.instance_count = 0;
            }
        }
        let globals = BillboardGlobals {
            frame: ctx.frame_num as u32,
            delta_time: 0.0,
            ambient_intensity: 1.0,
            _pad: 0.0,
        };
        ctx.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        // Always returns Some, even with zero instances (execute() then draws
        // nothing) — a pass that conditionally returns None based on per-frame
        // state can never safely participate in subpass-chain fusion, since the
        // executor decides chain membership once via a lock-time probe.
        let pre_aa = resources.get(helio_core::ResourceKey::new("pre_aa"));
        let target_view = if self.occluded_by_geometry {
            pre_aa.unwrap_or(target)
        } else {
            target
        };

        // `pre_aa` is internal-resolution (render-scaled); `target` is full
        // output resolution. Depth must track whichever one color actually
        // resolved to, or wgpu rejects the pass for mismatched attachment
        // extents whenever render_scale < 1.0.
        let depth_view = if self.occluded_by_geometry && pre_aa.is_some() {
            depth
        } else {
            resources
                .get(helio_core::ResourceKey::new("full_res_depth"))
                .unwrap_or(depth)
        };
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Billboard"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.instance_count == 0 {
            return Ok(());
        }
        let mut rp = ctx.render_cmds().unwrap();
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group_0, &[]);
        rp.set_bind_group(1, &self.bind_group_1, &[]);
        rp.set_vertex_buffer(0, self.quad_vertex_buf.slice(..));
        rp.draw(0..6, 0..self.instance_count);
        Ok(())
    }
}
