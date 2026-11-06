//! GPU-instanced 2D sprite batcher with texture atlasing.
//!
//! Sprites live in a persistent, handle-addressed pool
//! ([`SpriteBatchPass::insert_sprite`] / [`update_sprite`](SpriteBatchPass::update_sprite) /
//! [`remove_sprite`](SpriteBatchPass::remove_sprite)) — not a per-frame push
//! list. This matters for what gets uploaded to the GPU each frame, matching
//! the dirty-range convention the rest of the engine uses for its scene
//! buffers (`helio_core::GrowableBuffer`): instance data (and the parallel
//! alive-flags buffer) is only re-uploaded for the byte range actually
//! touched by `insert_sprite`/`update_sprite`/`remove_sprite` since the last
//! `prepare()` — `O(1)` (no upload at all) if nothing changed.
//!
//! Culling and depth-sorting the pool into a draw order is **not** done
//! here, and not on the CPU at all — pair this pass with
//! `helio-pass-sprite-cull`'s `SpriteCullPass`, added to the graph *before*
//! this one, and wire its outputs in via [`SpriteBatchPass::use_gpu_culling`].
//! That pass culls + radix-sorts the whole pool on the GPU every frame
//! (regardless of pool size — 10, 10 thousand, or 10 million sprites cost
//! the CPU the same: zero), and this pass's `execute()` issues one
//! `draw_indexed_indirect` reading the GPU-computed instance count, never
//! learning the visible count on the CPU at all. See that crate's module doc
//! comment for the full design (and why it's a separate crate: no Cargo
//! dependency either way, just `Arc<wgpu::Buffer>` handles passed between
//! them, matching how `helio-pass-shadow-cull`/`helio-pass-shadow` are wired).
//!
//! The pass renders via vertex-pulling (a `var<storage, read> instances` array
//! indexed through a separate `draw_order` array), not
//! `VertexStepMode::Instance` — see the shader's module doc comment for why.
//!
//! # Scene overlay
//!
//! [`SpriteBatchPass::scene_overlay`] draws a scene's authored 2D sprites
//! (SceneDB's `"sprite_instances"`, each row's image a slot of the scene's
//! bindless texture table) over whatever the target already holds: the
//! default graphs' final 2D overlay, after post-processing. It keeps its own
//! orthographic camera, so the 3D camera does not move it, and it records
//! no draw while the scene holds no sprite.
//!
//! This pass has no dependency on `helio` / `helio-presets` — it only
//! needs `helio-core` (for the [`RenderPass`] trait and graph plumbing) and
//! `helio-core` (for [`ResourceRegistry`](helio_core::ResourceRegistry)), matching
//! every other pass crate. It never touches `PassContext::scene`, so it is
//! usable inside a `RenderGraph` that carries no 3D scene data at all.

pub mod feature;
pub mod scene_rows;

pub use feature::SpriteFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result};
use pulsar_scenedb::gpu::BufferKey;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// A stable reference to a sprite in a [`SpriteBatchPass`]'s pool. Returned by
/// [`SpriteBatchPass::insert_sprite`]; opaque, not orderable across passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpriteHandle(u32);
impl SpriteHandle {
    /// Construct a handle for a SceneDB row slot.
    pub const fn from_index(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A single sprite's transform, atlas region, and tint.
///
/// Field order/padding here is not incidental: it must byte-match the
/// `SpriteInstance` struct in `shaders/sprite.wgsl` exactly, including WGSL's
/// storage-buffer alignment rules (`vec4<f32>` requires 16-byte alignment,
/// which Rust's `[f32; 4]` does *not* impose on its own — the `_pad_*` fields
/// exist purely to reproduce that padding on the CPU side).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SpriteInstance {
    /// World-space center, in world units (1 unit == 1 pixel at zoom 1.0).
    pub position: [f32; 2],
    /// Width/height in world units, before rotation.
    pub size: [f32; 2],
    /// Rotation about the sprite's own center, radians.
    pub rotation: f32,
    /// Back-to-front sort key (larger draws later, i.e. on top). Not a
    /// hardware depth value: alpha-blended sprites can't rely on a depth test
    /// for correct compositing (blending is order-dependent regardless of
    /// what a depth buffer says), so the paired GPU cull pass
    /// (`helio-pass-sprite-cull`) radix-sorts the draw-order index list by
    /// this field instead of enabling `DepthStencilState`. A common
    /// convention is "world Y" for a top-down/2.5D look (larger Y = further
    /// back = drawn first), but any scalar works.
    pub depth: f32,
    _pad_uv: [f32; 2],
    /// Atlas UV rectangle: `[u0, v0, u1, v1]`, each in `0..1`, within
    /// `atlas_layer`.
    pub uv_rect: [f32; 4],
    /// Straight-alpha RGBA tint, multiplied against the sampled atlas texel.
    pub color: [f32; 4],
    /// Index into the atlas texture array — see
    /// [`SpriteBatchPass::add_atlas_layer`].
    pub atlas_layer: u32,
    _pad_tail: [u32; 3],
}

impl SpriteInstance {
    pub fn new(position: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            position,
            size,
            rotation: 0.0,
            depth: 0.0,
            _pad_uv: [0.0; 2],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            atlas_layer: 0,
            _pad_tail: [0; 3],
        }
    }

    pub fn with_rotation(mut self, radians: f32) -> Self {
        self.rotation = radians;
        self
    }

    pub fn with_depth(mut self, depth: f32) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_uv_rect(mut self, uv_rect: [f32; 4]) -> Self {
        self.uv_rect = uv_rect;
        self
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn with_atlas_layer(mut self, layer: u32) -> Self {
        self.atlas_layer = layer;
        self
    }
}

/// SceneDB-owned persistent sprite row. The packed ABI intentionally matches
/// [`SpriteInstance`] and `shaders/sprite.wgsl`; cull/sort/indirect buffers
/// remain renderer-derived state.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpriteRow {
    pub position_x: f32,
    pub position_y: f32,
    pub size_x: f32,
    pub size_y: f32,
    pub rotation: f32,
    pub depth: f32,
    pub _pad_uv_x: f32,
    pub _pad_uv_y: f32,
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    pub atlas_layer: u32,
    pub _pad_tail_0: u32,
    pub _pad_tail_1: u32,
    pub _pad_tail_2: u32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for SpriteRow {}

/// The buffer the renderer reads [`SpriteRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const SPRITE_INSTANCES: helio_core::BufferContract<SpriteRow> =
    helio_core::BufferContract::new("sprite_instances");

impl From<SpriteInstance> for SpriteRow {
    fn from(value: SpriteInstance) -> Self {
        Self {
            position_x: value.position[0],
            position_y: value.position[1],
            size_x: value.size[0],
            size_y: value.size[1],
            rotation: value.rotation,
            depth: value.depth,
            _pad_uv_x: value._pad_uv[0],
            _pad_uv_y: value._pad_uv[1],
            uv_rect: value.uv_rect,
            color: value.color,
            atlas_layer: value.atlas_layer,
            _pad_tail_0: value._pad_tail[0],
            _pad_tail_1: value._pad_tail[1],
            _pad_tail_2: value._pad_tail[2],
        }
    }
}

impl From<SpriteRow> for SpriteInstance {
    fn from(value: SpriteRow) -> Self {
        Self {
            position: [value.position_x, value.position_y],
            size: [value.size_x, value.size_y],
            rotation: value.rotation,
            depth: value.depth,
            _pad_uv: [value._pad_uv_x, value._pad_uv_y],
            uv_rect: value.uv_rect,
            color: value.color,
            atlas_layer: value.atlas_layer,
            _pad_tail: [value._pad_tail_0, value._pad_tail_1, value._pad_tail_2],
        }
    }
}
const INSTANCE_STRIDE: u64 = std::mem::size_of::<SpriteInstance>() as u64;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

// Y-up world space; v=0 at the top of the quad to match texture-space UVs.
const QUAD_VERTICES: [QuadVertex; 4] = [
    QuadVertex {
        pos: [-0.5, -0.5],
        uv: [0.0, 1.0],
    },
    QuadVertex {
        pos: [0.5, -0.5],
        uv: [1.0, 1.0],
    },
    QuadVertex {
        pos: [-0.5, 0.5],
        uv: [0.0, 0.0],
    },
    QuadVertex {
        pos: [0.5, 0.5],
        uv: [1.0, 0.0],
    },
];
const QUAD_INDICES: [u16; 6] = [0, 1, 2, 2, 1, 3];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpriteCamera {
    view_proj: [[f32; 4]; 4],
}

const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// GPU-instanced 2D sprite batch pass.
///
/// Owns its own orthographic camera (recomputed from the render target size
/// every frame — call [`SpriteBatchPass::set_camera`] to override
/// framing/zoom/pan) and starts with a 1×1 white fallback atlas layer, so it
/// renders solid-colored quads out of the box; call
/// [`SpriteBatchPass::add_atlas_layer`] to load real sprite sheets.
pub struct SpriteBatchPass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,

    atlas_texture: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    /// `true` until the first [`SpriteBatchPass::add_atlas_layer`] call — the
    /// pass is still serving the 1×1 white fallback and hasn't committed to
    /// a real atlas width/height yet.
    atlas_using_fallback: bool,
    atlas_width: u32,
    atlas_height: u32,
    atlas_layer_count: u32,
    atlas_layer_capacity: u32,

    quad_vertex_buf: wgpu::Buffer,
    quad_index_buf: wgpu::Buffer,

    // ── Persistent, handle-addressed instance pool (delta-uploaded) ───────
    slots: Vec<SpriteInstance>,
    slot_alive: Vec<u32>,
    free_list: Vec<u32>,
    /// `[start, end)` slot range touched since the last upload, or `None` if
    /// clean. `prepare()` uploads exactly this byte range (both instance data
    /// and alive flags) and nothing else.
    dirty_range: Option<(usize, usize)>,
    instances_buf: Arc<wgpu::Buffer>,
    instances_capacity: usize,
    /// Parallel `slot_alive` flags (0/1 per slot) mirrored on the GPU, read
    /// by the paired cull pass.
    alive_buf: Arc<wgpu::Buffer>,
    alive_capacity: usize,
    scene_instances_buf: Option<wgpu::Buffer>,
    scene_instances_epoch: Option<u64>,

    // ── GPU cull/sort wiring (provided by `helio-pass-sprite-cull`) ───────
    gpu_culling: Option<GpuCulling>,

    camera_buf: wgpu::Buffer,
    camera_dirty: bool,
    last_width: u32,
    last_height: u32,
    /// Half-extent of the orthographic view, in world units. `None` means
    /// "derive from the render target's pixel size" (1 world unit = 1 pixel).
    camera_half_extent: Option<[f32; 2]>,
    camera_center: [f32; 2],
    clear_color: Option<wgpu::Color>,
    /// Set by [`SpriteBatchPass::scene_overlay`].
    scene_overlay: Option<SceneOverlay>,
}

/// The scene overlay variant's own state: a pipeline sampling the scene's
/// texture table (group 1). The pass is a graph member only while the scene
/// holds a sprite.
struct SceneOverlay {
    pipeline: wgpu::RenderPipeline,
    textures_bgl: wgpu::BindGroupLayout,
    textures_bg: Option<wgpu::BindGroup>,
    textures_version: Option<u64>,
    material_binding: helio_mats::MaterialBindingConfig,
}

/// Outputs of a paired `helio-pass-sprite-cull` `SpriteCullPass`, wired in
/// via [`SpriteBatchPass::use_gpu_culling`]. `draw_order_buf` is the GPU-sorted
/// list of slot indices to draw; `indirect_buf` holds
/// `DrawIndexedIndirectArgs` whose `instance_count` the cull pass writes every
/// frame — the CPU never learns the visible count.
struct GpuCulling {
    draw_order_buf: Arc<wgpu::Buffer>,
    indirect_buf: Arc<wgpu::Buffer>,
}

impl SpriteBatchPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = helio_core::shader::module_with(
            device,
            "Sprite Batch Shader",
            helio_core::include_wgsl!("../shaders/sprite.wgsl"),
            &[helio_pass_sprite_cull::SPRITE_INSTANCE_SNIPPET],
        );

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Batch BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Batch PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
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
        };

        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Sprite Batch Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(vertex_layout)],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        // Straight-alpha atlas texels over an opaque or transparent
                        // target; premultiplied atlases should premultiply at import
                        // time and use `BlendState::REPLACE` via a future variant.
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Camera Uniform"),
            size: std::mem::size_of::<SpriteCamera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let quad_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite Quad Vertices"),
            contents: bytemuck::cast_slice(&QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let quad_index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite Quad Indices"),
            contents: bytemuck::cast_slice(&QUAD_INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });

        let initial_capacity = 256usize;
        let instances_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Instance Storage"),
            size: INSTANCE_STRIDE * initial_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let alive_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Alive Flags"),
            size: 4 * initial_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Sprite Atlas Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let atlas_texture = create_atlas_array_texture(device, 1, 1, 1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_texture,
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
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        Self {
            pipeline,
            bgl,
            bind_group: None,
            sampler,
            atlas_texture,
            atlas_view,
            atlas_using_fallback: true,
            atlas_width: 1,
            atlas_height: 1,
            atlas_layer_count: 1,
            atlas_layer_capacity: 1,
            quad_vertex_buf,
            quad_index_buf,
            slots: Vec::new(),
            slot_alive: Vec::new(),
            free_list: Vec::new(),
            dirty_range: None,
            instances_buf,
            instances_capacity: initial_capacity,
            alive_buf,
            alive_capacity: initial_capacity,
            scene_instances_buf: None,
            scene_instances_epoch: None,
            gpu_culling: None,
            camera_buf,
            camera_dirty: true,
            last_width: 0,
            last_height: 0,
            camera_half_extent: None,
            camera_center: [0.0, 0.0],
            clear_color: Some(wgpu::Color::BLACK),
            scene_overlay: None,
        }
    }

    /// The scene's authored 2D sprites as an overlay (see the module doc):
    /// draws SceneDB's `"sprite_instances"` over the target without
    /// clearing it, each sprite's image sampled from the scene's texture
    /// table (`material_binding` must be the renderer's). Pair it with a
    /// `SpriteCullPass` built with `follow_target`, wired in through
    /// [`use_gpu_culling`](Self::use_gpu_culling).
    pub fn scene_overlay(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        material_binding: helio_mats::MaterialBindingConfig,
    ) -> Self {
        let mut pass = Self::new(device, queue, surface_format);
        pass.clear_color = None;
        let mut entries = Vec::new();
        material_binding.append_layout_entries(&mut entries, 0, wgpu::ShaderStages::FRAGMENT);
        let textures_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Scene Textures BGL"),
            entries: &entries,
        });
        let source = scene_overlay_source(material_binding);
        let shader = helio_core::shader::module_with(
            device,
            "Sprite Scene Shader",
            &source,
            &[helio_pass_sprite_cull::SPRITE_INSTANCE_SNIPPET],
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Scene PL"),
            bind_group_layouts: &[Some(&pass.bgl), Some(&textures_bgl)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Sprite Scene Pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(quad_vertex_layout())],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        pass.scene_overlay = Some(SceneOverlay {
            pipeline,
            textures_bgl,
            textures_bg: None,
            textures_version: None,
            material_binding,
        });
        pass
    }

    fn mark_data_dirty(&mut self, slot: usize) {
        self.dirty_range = Some(match self.dirty_range {
            Some((s, e)) => (s.min(slot), e.max(slot + 1)),
            None => (slot, slot + 1),
        });
    }

    /// Adds a new sprite to the pool. `O(1)` amortized (reuses a freed slot
    /// if one exists). The returned handle stays valid until
    /// [`remove_sprite`](Self::remove_sprite).
    pub fn insert_sprite(&mut self, instance: SpriteInstance) -> SpriteHandle {
        let slot = if let Some(free) = self.free_list.pop() {
            self.slots[free as usize] = instance;
            self.slot_alive[free as usize] = 1;
            free
        } else {
            self.slots.push(instance);
            self.slot_alive.push(1);
            (self.slots.len() - 1) as u32
        };
        self.mark_data_dirty(slot as usize);
        SpriteHandle(slot)
    }

    /// Overwrites a sprite's data in place. `O(1)`; marks only this slot's
    /// byte range dirty for the next `prepare()`.
    pub fn update_sprite(&mut self, handle: SpriteHandle, instance: SpriteInstance) {
        let slot = handle.0 as usize;
        self.slots[slot] = instance;
        self.mark_data_dirty(slot);
    }

    /// Frees a sprite's slot for reuse. Its GPU instance bytes are stale but
    /// never read again (the cull pass skips slots whose alive flag is 0, and
    /// the slot is handed out to the next `insert_sprite`), so the only byte
    /// that must be re-uploaded is the alive flag itself.
    pub fn remove_sprite(&mut self, handle: SpriteHandle) {
        let slot = handle.0 as usize;
        if self.slot_alive[slot] != 0 {
            self.slot_alive[slot] = 0;
            self.free_list.push(handle.0);
            self.mark_data_dirty(slot);
        }
    }

    /// Uploads `rgba8` (must be exactly `width * height * 4` bytes, straight
    /// alpha) as a new layer in the atlas array, growing the array (and
    /// invalidating the cached bind group, rebuilt lazily on the next
    /// `execute()`) if it's out of capacity. Returns the new layer's index
    /// for [`SpriteInstance::with_atlas_layer`].
    ///
    /// Every layer in a texture array must share one width/height — the
    /// first call after construction fixes that size for the pass's
    /// lifetime; subsequent calls with a different `width`/`height` panic.
    /// Pack sprites that don't already share a sheet size into same-size
    /// atlas pages before calling this (a texture-array layer boundary is
    /// not a UV-rect boundary — `uv_rect` still addresses within one layer).
    pub fn add_atlas_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba8: &[u8],
    ) -> u32 {
        assert_eq!(
            rgba8.len() as u32,
            width * height * 4,
            "rgba8 length {} does not match width*height*4 ({width}*{height}*4 = {})",
            rgba8.len(),
            width * height * 4
        );

        if self.atlas_using_fallback {
            self.atlas_width = width;
            self.atlas_height = height;
            self.atlas_layer_capacity = 4;
            self.atlas_layer_count = 0;
            self.atlas_texture =
                create_atlas_array_texture(device, width, height, self.atlas_layer_capacity);
            self.atlas_view = self
                .atlas_texture
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                });
            self.atlas_using_fallback = false;
            self.bind_group = None;
        } else {
            assert_eq!(
                width, self.atlas_width,
                "atlas layer width {width} does not match the array's existing width {} \
                 (all layers in one SpriteBatchPass must share dimensions)",
                self.atlas_width
            );
            assert_eq!(
                height, self.atlas_height,
                "atlas layer height {height} does not match the array's existing height {} \
                 (all layers in one SpriteBatchPass must share dimensions)",
                self.atlas_height
            );
        }

        if self.atlas_layer_count >= self.atlas_layer_capacity {
            self.grow_atlas_array(device, queue);
        }

        let layer = self.atlas_layer_count;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            rgba8,
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
        self.atlas_layer_count += 1;
        layer
    }

    fn grow_atlas_array(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let new_capacity = self.atlas_layer_capacity * 2;
        let new_texture =
            create_atlas_array_texture(device, self.atlas_width, self.atlas_height, new_capacity);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Sprite Atlas Array Grow"),
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &new_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.atlas_width,
                height: self.atlas_height,
                depth_or_array_layers: self.atlas_layer_count,
            },
        );
        queue.submit([encoder.finish()]);

        self.atlas_texture = new_texture;
        self.atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        self.atlas_layer_capacity = new_capacity;
        self.bind_group = None;
    }

    /// Escape hatch: replace atlas management entirely with a caller-owned
    /// `D2Array` view (e.g. loaded via a dedicated atlas-packing tool).
    /// `view`/`sampler` must outlive every subsequent `execute()` call until
    /// the next `set_atlas_array`/`add_atlas_layer`.
    ///
    /// The bind group is only valid once the pass can draw, so GPU culling
    /// must be wired via [`use_gpu_culling`](Self::use_gpu_culling) first.
    pub fn set_atlas_array(
        &mut self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) {
        let gpu = self
            .gpu_culling
            .as_ref()
            .expect("set_atlas_array: call use_gpu_culling() first");
        self.bind_group = Some(
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Sprite Batch BG"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.camera_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self
                            .scene_instances_buf
                            .as_ref()
                            .unwrap_or(&self.instances_buf)
                            .as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: gpu.draw_order_buf.as_entire_binding(),
                    },
                ],
            }),
        );
    }

    /// Overrides the orthographic view: `center` and `half_extent` in world
    /// units. `half_extent = None` re-derives it from the render target's
    /// pixel dimensions each frame (1 world unit = 1 pixel, origin centered).
    pub fn set_camera(&mut self, center: [f32; 2], half_extent: Option<[f32; 2]>) {
        self.camera_center = center;
        self.camera_half_extent = half_extent;
        self.camera_dirty = true;
    }

    /// `None` disables the clear (loads the existing target contents);
    /// `Some(color)` clears to that color every frame. Defaults to opaque black.
    pub fn set_clear_color(&mut self, color: Option<wgpu::Color>) {
        self.clear_color = color;
    }

    /// Pre-sizes the instance pool's GPU buffers (instance data + alive
    /// flags) to `capacity` slots. The paired cull pass binds these buffers
    /// once at construction and can't follow reallocations, so call this
    /// *before* inserting all the sprites the pool will ever hold, and before
    /// wiring [`use_gpu_culling`](Self::use_gpu_culling). Inserting more
    /// sprites than a reserved capacity once culling is wired panics in
    /// `prepare()`.
    pub fn reserve(&mut self, device: &wgpu::Device, capacity: usize) {
        if capacity <= self.instances_capacity && capacity <= self.alive_capacity {
            return;
        }
        self.instances_capacity = capacity.max(self.instances_capacity);
        self.alive_capacity = capacity.max(self.alive_capacity);
        self.instances_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Instance Storage"),
            size: INSTANCE_STRIDE * self.instances_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.alive_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Alive Flags"),
            size: 4 * self.alive_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.bind_group = None;
        self.dirty_range = (!self.slots.is_empty()).then_some((0, self.slots.len()));
    }

    /// The instance-data storage buffer, for handing to
    /// `helio-pass-sprite-cull`'s `SpriteCullPass::new` before wiring its
    /// outputs in via [`use_gpu_culling`](Self::use_gpu_culling).
    pub fn instances_buffer(&self) -> Arc<wgpu::Buffer> {
        self.instances_buf.clone()
    }

    /// The parallel alive-flags storage buffer (`u32` per slot, 1 = alive),
    /// for handing to `helio-pass-sprite-cull`'s `SpriteCullPass::new`.
    pub fn alive_buffer(&self) -> Arc<wgpu::Buffer> {
        self.alive_buf.clone()
    }

    /// Hands the pass the cull/sort pass's outputs: a `draw_order_buf`
    /// (GPU-written, radix-sorted slot indices) and an `indirect_buf`
    /// (`DrawIndexedIndirectArgs` whose `instance_count` the cull pass writes
    /// each frame). After this, `prepare()` no longer does any CPU culling or
    /// sorting and `execute()` issues a single `draw_indexed_indirect` — the
    /// CPU never learns the visible count.
    pub fn use_gpu_culling(
        &mut self,
        draw_order_buf: Arc<wgpu::Buffer>,
        indirect_buf: Arc<wgpu::Buffer>,
    ) {
        self.gpu_culling = Some(GpuCulling {
            draw_order_buf,
            indirect_buf,
        });
        self.bind_group = None;
    }

    /// Sprites currently alive in the pool (inserted, not yet removed).
    pub fn sprite_count(&self) -> usize {
        self.slots.len() - self.free_list.len()
    }
}

/// The quad's vertex buffer layout (position, UV).
fn quad_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<QuadVertex>() as u64,
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
    }
}

/// The scene overlay shader, resized to this platform's texture table, as
/// the decal pass does: the 256-entry native table resized to the material
/// tier, or individual bindings where `binding_array` is unavailable.
fn scene_overlay_source(material_binding: helio_mats::MaterialBindingConfig) -> String {
    let text = helio_core::shader::source_text(
        "Sprite Scene",
        helio_core::include_wgsl!("../shaders/sprite_scene.wgsl"),
    );
    let src: &str = &text;
    if material_binding.uses_binding_arrays() {
        src.replace(
            "binding_array<texture_2d<f32>, 256>",
            &format!(
                "binding_array<texture_2d<f32>, {}>",
                material_binding.max_textures
            ),
        )
        .replace(
            "binding_array<sampler, 256>",
            &format!("binding_array<sampler, {}>", material_binding.max_textures),
        )
    } else {
        helio_mats::apply_webgpu_decal_bindings(src, material_binding.max_textures)
    }
}

fn create_atlas_array_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    layers: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Sprite Atlas Array"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl RenderPass for SpriteBatchPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["sprite_instances"]
    }

    fn name(&self) -> &'static str {
        "SpriteBatch"
    }

    /// The scene overlay is a member while some `"sprite_instances"` row is
    /// live; a pass drawing its own sprites always is.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        self.scene_overlay.is_none() || scene.holds_rows("sprite_instances")
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        // 2D sprites are alpha-blended and GPU-sorted (see `SpriteInstance::depth`)
        // — no depth attachment. `Box::leak` here matches the convention used by
        // every other executor-managed pass: the descriptor only needs to live
        // for this frame's `execute()` call, and the executor drops it before
        // the next `render_pass_descriptor()`.
        let load = match self.clear_color {
            Some(color) => wgpu::LoadOp::Clear(color),
            None => wgpu::LoadOp::Load,
        };
        let attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Sprite Batch Pass"),
            color_attachments: attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        if self.scene_overlay.is_some() {
            builder.read(helio_mats::MATERIAL_TEXTURES);
        }
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> Result<()> {
        if ctx.width != self.last_width || ctx.height != self.last_height {
            self.last_width = ctx.width;
            self.last_height = ctx.height;
            self.camera_dirty = true;
        }

        if self.camera_dirty {
            self.camera_dirty = false;
            let half_extent = self
                .camera_half_extent
                .unwrap_or([ctx.width as f32 * 0.5, ctx.height as f32 * 0.5]);
            let [cx, cy] = self.camera_center;
            let [hx, hy] = half_extent;
            // Y-up world space; a Y-down convention would swap `cy - hy`/`cy + hy`.
            let view_proj =
                glam::Mat4::orthographic_rh(cx - hx, cx + hx, cy - hy, cy + hy, -1.0, 1.0);
            let uniform = SpriteCamera {
                view_proj: view_proj.to_cols_array_2d(),
            };
            ctx.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));
        }

        let scene_instances = ctx.scene_buffers.get(BufferKey::of("sprite_instances"));
        let scene_epoch = scene_instances.map(|handle| handle.epoch);
        if self.scene_instances_epoch != scene_epoch {
            self.scene_instances_buf = scene_instances.map(|handle| handle.buffer.clone());
            self.scene_instances_epoch = scene_epoch;
            self.bind_group = None;
        }

        // SceneDB rows are authoritative and already mirrored by the frontend.
        // Do not upload or grow the obsolete renderer-local pool on this path.
        if self.scene_instances_buf.is_some() {
            return Ok(());
        }

        // ── Delta-upload instance + alive-flag data ────────────────────────
        // No CPU culling or sorting happens here (or anywhere) — the paired
        // `SpriteCullPass` does that on the GPU from these two buffers.
        if self.slots.len() > self.instances_capacity {
            if self.gpu_culling.is_some() {
                panic!(
                    "SpriteBatchPass: pool grew to {} sprites but GPU-culling buffers are reserved for {} slots. \
                     Insert all sprites and call `reserve()` before `use_gpu_culling()` (or reserve more)",
                    self.slots.len(),
                    self.instances_capacity
                );
            }
            self.instances_capacity = (self.slots.len() * 2).max(self.instances_capacity * 2);
            self.instances_buf = Arc::new(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sprite Instance Storage"),
                size: INSTANCE_STRIDE * self.instances_capacity as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.alive_buf = Arc::new(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sprite Alive Flags"),
                size: 4 * self.instances_capacity as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.bind_group = None; // buffer identity changed
            self.dirty_range = (!self.slots.is_empty()).then_some((0, self.slots.len()));
        }
        if let Some((start, end)) = self.dirty_range.take() {
            ctx.write_buffer(
                &self.instances_buf,
                start as u64 * INSTANCE_STRIDE,
                bytemuck::cast_slice(&self.slots[start..end]),
            );
            ctx.write_buffer(
                &self.alive_buf,
                start as u64 * 4,
                bytemuck::cast_slice(&self.slot_alive[start..end]),
            );
        }

        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        let Some(gpu) = self.gpu_culling.as_ref() else {
            return Ok(());
        };

        if self.bind_group.is_none() {
            self.bind_group = Some(
                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Sprite Batch BG"),
                    layout: &self.bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.camera_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&self.atlas_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self
                                .scene_instances_buf
                                .as_ref()
                                .unwrap_or(&self.instances_buf)
                                .as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: gpu.draw_order_buf.as_entire_binding(),
                        },
                    ],
                }),
            );
        }

        let Some(mut rp) = ctx.render_cmds() else {
            return Ok(());
        };
        if let Some(overlay) = self.scene_overlay.as_mut() {
            // The scene's texture table, published per frame by the renderer.
            let Some(textures) = ctx
                .registry
                .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
            else {
                return Ok(());
            };
            if overlay.textures_version != Some(textures.version) || overlay.textures_bg.is_none() {
                let mut entries = Vec::new();
                overlay.material_binding.append_bind_group_entries(
                    &mut entries,
                    0,
                    textures.texture_views,
                    textures.samplers,
                );
                overlay.textures_bg =
                    Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Sprite Scene Textures BG"),
                        layout: &overlay.textures_bgl,
                        entries: &entries,
                    }));
                overlay.textures_version = Some(textures.version);
            }
            rp.set_pipeline(&overlay.pipeline);
            rp.set_bind_group(1, overlay.textures_bg.as_ref().unwrap(), &[]);
        } else {
            rp.set_pipeline(&self.pipeline);
        }
        rp.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        rp.set_vertex_buffer(0, self.quad_vertex_buf.slice(..));
        rp.set_index_buffer(self.quad_index_buf.slice(..), wgpu::IndexFormat::Uint16);
        rp.draw_indexed_indirect(&gpu.indirect_buf, 0);

        Ok(())
    }
}
