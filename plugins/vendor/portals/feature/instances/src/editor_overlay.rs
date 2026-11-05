//! Editor-only checkerboard indicator over each portal's opening. See
//! `shaders/portal_editor_overlay.wgsl` for the full rationale. Disabled
//! (zero draws, zero cost) unless [`PortalEditorOverlayPass::set_editor_mode`]
//! has been called with `true` — call it from wherever your application
//! already tracks editor vs. game mode (mirrors `helio::Renderer`'s own
//! `is_editor_mode()` / `set_editor_mode()`, which this pass does not read
//! automatically since plain render passes have no access to the `Renderer`
//! that owns them).

use helio_core::graph::ResourceBuilder;
use helio_core::{
    DebugViewDescriptor, PassContext, PayloadContract, PrepareContext, RenderPass,
    Result as HelioResult,
};
use pulsar_scenedb::gpu::BufferKey;

/// Short world-anchored text rendered by the portal editor overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalDebugText {
    /// Absolute world coordinates. The renderer rebases these against its
    /// camera-relative frame origin before uploading them to the GPU.
    pub world_position: [f64; 3],
    pub text: String,
    pub color: [f32; 4],
}

/// The labels an editor shows over its portals: it publishes them for its
/// viewport (a view payload); the overlay draws the latest.
pub const PORTAL_DEBUG_TEXTS: PayloadContract<Vec<PortalDebugText>> =
    PayloadContract::new("portal_debug_texts");

/// The geometry an editor shows mapped through the portal it selected:
/// triangles, three absolute world positions each (a view payload, as
/// [`PORTAL_DEBUG_TEXTS`]).
pub const PORTAL_DEBUG_GEOMETRY: PayloadContract<Vec<[f64; 3]>> =
    PayloadContract::new("portal_debug_geometry");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphInstance {
    world_position: [f32; 4],
    color: [f32; 4],
    bitmap: [u32; 2],
    screen_offset: [f32; 2],
}

pub struct PortalEditorOverlayPass {
    pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_key: Option<[wgpu::Buffer; 2]>,
    portal_count: u32,
    /// Live portal views: the frame's `PORTAL_COUNTS`, or what a host set
    /// by hand ([`Self::set_active_portal_count`]) when it publishes none.
    active_portal_count: Option<u32>,
    editor_mode: bool,
    debug_texts: Vec<PortalDebugText>,
    glyph_buffer: wgpu::Buffer,
    glyph_count: u32,
    ghost_pipeline: wgpu::RenderPipeline,
    ghost_buffer: wgpu::Buffer,
    ghost_vertex_count: u32,
    ghost_vertices: Vec<[f64; 3]>,
    ghost_geometry_dirty: bool,
    ghost_world_origin: Option<[f64; 3]>,
    fallback_portal_views: wgpu::Buffer,
}

impl PortalEditorOverlayPass {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = helio_core::shader::module(
            device,
            "PortalEditorOverlay Shader",
            helio_core::include_wgsl!("../shaders/portal_editor_overlay.wgsl"),
        );

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PortalEditorOverlay BGL"),
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
                }, // cameras
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }, // portal_views
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }, // debug_glyphs
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }, // ghost_vertices
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PortalEditorOverlay PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("PortalEditorOverlay Pipeline"),
                layout: Some(&pl),
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
                        format: surface_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            // Leave pre_aa's alpha channel alone — later passes
                            // in the chain (LensFlare/PostProcess) use it for
                            // more than plain transparency, and blending our own
                            // alpha into it (as helio-pass-corona's particles
                            // legitimately do, being the actual scene content)
                            // corrupted whatever they read from it into visible
                            // ray/streak artifacts. This overlay only ever wants
                            // to darken RGB, so just keep destination alpha.
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // Read-only: respect real occluders in front of the portal
                    // (same reasoning as helio-pass-portal-mask's stamp draw) —
                    // don't paint the indicator through a wall.
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
        let glyph_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("PortalEditorGlyph Pipeline"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_glyph_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_glyph_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
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
        let ghost_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Portal Editor Ghost Geometry Pipeline"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_ghost_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_ghost_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
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
            glyph_pipeline,
            bgl,
            bind_group: None,
            bind_group_key: None,
            portal_count: 0,
            active_portal_count: None,
            editor_mode: false,
            debug_texts: Vec::new(),
            glyph_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Portal Editor Glyphs"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            glyph_count: 0,
            ghost_pipeline,
            ghost_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Portal Editor Ghost Geometry"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ghost_vertex_count: 0,
            ghost_vertices: Vec::new(),
            ghost_geometry_dirty: true,
            ghost_world_origin: None,
            fallback_portal_views: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Portal Editor Empty Views"),
                size: 144,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
        }
    }

    /// Set the active dense portal-view row count for editor visualization.
    pub fn set_active_portal_count(&mut self, count: u32) {
        self.active_portal_count = Some(count);
    }

    /// Enable or disable the checkerboard indicator. Mirrors whatever
    /// game/editor-mode toggle the host application already has — call this
    /// whenever that flips (e.g. alongside `Renderer::set_editor_mode`).
    pub fn set_editor_mode(&mut self, enabled: bool) {
        self.editor_mode = enabled;
    }

    pub fn is_editor_mode(&self) -> bool {
        self.editor_mode
    }

    /// Replace the world-anchored labels shown alongside the portal checker.
    pub fn set_debug_texts(&mut self, texts: Vec<PortalDebugText>) -> bool {
        if self.debug_texts == texts {
            return false;
        }
        self.debug_texts = texts;
        true
    }

    /// Replace the transient triangles shown through the selected portal.
    pub fn set_ghost_vertices(&mut self, vertices: Vec<[f64; 3]>) -> bool {
        if self.ghost_vertices == vertices {
            return false;
        }
        self.ghost_vertex_count = vertices.len().min(u32::MAX as usize) as u32;
        self.ghost_vertices = vertices;
        self.ghost_geometry_dirty = true;
        true
    }
}

impl RenderPass for PortalEditorOverlayPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["portal_views"]
    }

    fn name(&self) -> &'static str {
        "PortalEditorOverlay"
    }

    /// A member in editor mode while it has something to draw: live portals
    /// (`PORTAL_COUNTS`) or what the editor published ([`PORTAL_DEBUG_TEXTS`],
    /// [`PORTAL_DEBUG_GEOMETRY`]).
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        if let Some(buffers) = scene.scene_buffers() {
            self.active_portal_count = Some(helio_pass_portal_cull::portal_counts(buffers).views);
            let texts = buffers.payload(&PORTAL_DEBUG_TEXTS);
            if texts
                .as_deref()
                .map_or(!self.debug_texts.is_empty(), |texts| {
                    *texts != self.debug_texts
                })
            {
                self.set_debug_texts(texts.as_deref().cloned().unwrap_or_default());
            }
            let ghost = buffers.payload(&PORTAL_DEBUG_GEOMETRY);
            if ghost
                .as_deref()
                .map_or(!self.ghost_vertices.is_empty(), |ghost| {
                    *ghost != self.ghost_vertices
                })
            {
                self.set_ghost_vertices(ghost.as_deref().cloned().unwrap_or_default());
            }
        }
        self.editor_mode
            && (self.active_portal_count.unwrap_or(0) > 0
                || !self.debug_texts.is_empty()
                || self.ghost_vertex_count > 0)
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        static VIEWS: &[DebugViewDescriptor] = &[DebugViewDescriptor {
            name: "Portal Diagnostics",
            debug_mode: 53,
            description: "Portal link labels, checker overlays, and pairing diagnostics",
        }];
        VIEWS
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_frame_api::color::SCENE_COLOR,
            helio_frame_api::gbuffer::DEPTH,
        ]
    }

    fn writes(&self) -> &'static [&'static str] {
        // Draws (LoadOp::Load) directly onto pre_aa — see render_pass_descriptor.
        &[helio_frame_api::color::SCENE_COLOR]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(helio_frame_api::color::SCENE_COLOR);
    }

    fn set_editor_mode(&mut self, enabled: bool) {
        self.editor_mode = enabled;
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        // Always structurally participates in the pre_aa fusion chain,
        // regardless of `editor_mode` — that flag is runtime, mutable state,
        // while chain fusion is decided from attachment identity once (see
        // PortalInstancePass's own doc comment on why). Gating on it here
        // would make this pass flicker in and out of the chain and could
        // break fusion for passes chained through it. `execute()` is where
        // editor_mode actually matters: no draw call, zero cost, when off.
        let pre_aa = resources.get(helio_core::ResourceKey::new(
            helio_frame_api::color::SCENE_COLOR,
        ));
        let target_view = pre_aa.unwrap_or(target);
        // `pre_aa` is internal-resolution (render-scaled); the raw `target`
        // fallback is full output resolution. Depth must track whichever one
        // color actually resolved to, or wgpu rejects the pass for mismatched
        // attachment extents whenever render_scale < 1.0.
        let depth_view = if pre_aa.is_some() {
            depth
        } else {
            resources
                .get(helio_core::ResourceKey::new(
                    helio_frame_api::depth::FULL_RES_DEPTH,
                ))
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
            label: Some("PortalEditorOverlay"),
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

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // Growable SceneDB storage exposes capacity, not live portal rows.
        // An absent active count therefore means no authored/projected
        // portals, not "draw the whole reserve".
        self.portal_count = self.active_portal_count.unwrap_or(0);
        let origin = ctx.world_origin.to_array();
        let glyphs = self
            .debug_texts
            .iter()
            .flat_map(|text| make_glyphs(text, origin))
            .collect::<Vec<_>>();
        self.glyph_count = glyphs.len().min(u32::MAX as usize) as u32;
        if !glyphs.is_empty() {
            let bytes = bytemuck::cast_slice(&glyphs);
            if bytes.len() as u64 > self.glyph_buffer.size() {
                self.glyph_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Portal Editor Glyphs"),
                    size: (bytes.len() as u64).next_power_of_two(),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.bind_group = None;
                self.bind_group_key = None;
            }
            ctx.write_buffer(&self.glyph_buffer, 0, bytes);
        }
        if self.ghost_geometry_dirty || self.ghost_world_origin != Some(origin) {
            let gpu_vertices: Vec<[f32; 4]> = self
                .ghost_vertices
                .iter()
                .map(|point| {
                    [
                        (point[0] - origin[0]) as f32,
                        (point[1] - origin[1]) as f32,
                        (point[2] - origin[2]) as f32,
                        1.0,
                    ]
                })
                .collect();
            let bytes = bytemuck::cast_slice(&gpu_vertices);
            self.ghost_vertex_count = gpu_vertices.len().min(u32::MAX as usize) as u32;
            if bytes.len() as u64 > self.ghost_buffer.size() {
                self.ghost_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Portal Editor Ghost Geometry"),
                    size: (bytes.len() as u64).next_power_of_two(),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.bind_group = None;
                self.bind_group_key = None;
            }
            if !bytes.is_empty() {
                ctx.write_buffer(&self.ghost_buffer, 0, bytes);
            }
            self.ghost_geometry_dirty = false;
            self.ghost_world_origin = Some(origin);
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.editor_mode
            || (self.portal_count == 0 && self.glyph_count == 0 && self.ghost_vertex_count == 0)
        {
            return Ok(());
        }
        let Some(mut pass) = ctx.render_cmds() else {
            return Ok(());
        };
        let portal_views = ctx.scene_buffers.get(BufferKey::of("portal_views"));
        if self.portal_count > 0 && portal_views.is_none() {
            return Ok(());
        }

        let portal_view_buffer = portal_views
            .map(|views| &views.buffer)
            .unwrap_or(&self.fallback_portal_views);
        let key = [ctx.camera.clone(), portal_view_buffer.clone()];
        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("PortalEditorOverlay BG"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: portal_view_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.glyph_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.ghost_buffer.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        if self.portal_count > 0 {
            pass.draw(0..6, 0..self.portal_count);
        }
        if self.glyph_count > 0 {
            pass.set_pipeline(&self.glyph_pipeline);
            pass.draw(0..6, 0..self.glyph_count);
        }
        if self.ghost_vertex_count > 0 {
            pass.set_pipeline(&self.ghost_pipeline);
            pass.draw(0..self.ghost_vertex_count, 0..1);
        }

        Ok(())
    }
}

fn make_glyphs(
    text: &PortalDebugText,
    origin: [f64; 3],
) -> impl Iterator<Item = GlyphInstance> + '_ {
    text.text
        .chars()
        .enumerate()
        .map(move |(index, character)| {
            let rows = glyph_rows(character.to_ascii_uppercase());
            let mut bitmap = [0u32; 2];
            for (y, row) in rows.into_iter().enumerate() {
                for x in 0..5 {
                    if row & (1 << (4 - x)) != 0 {
                        let bit = y * 5 + x;
                        bitmap[bit / 32] |= 1 << (bit % 32);
                    }
                }
            }
            GlyphInstance {
                world_position: [
                    (text.world_position[0] - origin[0]) as f32,
                    (text.world_position[1] - origin[1]) as f32,
                    (text.world_position[2] - origin[2]) as f32,
                    1.0,
                ],
                color: text.color,
                bitmap,
                screen_offset: [index as f32 * 0.022, 0.0],
            }
        })
}

fn glyph_rows(character: char) -> [u8; 7] {
    match character {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'N' => [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        ' ' => [0; 7],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        _ => [
            0b11111, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
    }
}
