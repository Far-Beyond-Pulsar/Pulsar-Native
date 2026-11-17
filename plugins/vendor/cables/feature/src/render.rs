use std::sync::Arc;

use cables_api::{CABLE_DEFINITIONS, CABLE_SPANS, CableSpans, MAX_BUNDLE_LANES};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

const SEGMENTS: u32 = 32;
const TUBE_SIDES: u32 = 12;
const RIBBON_VERTICES: u32 = SEGMENTS * 6;
const TUBE_VERTICES: u32 = SEGMENTS * TUBE_SIDES * 6;
/// Definition rows the draw searches (the join's packed capacity).
const MAX_DEFINITIONS: u32 = 256;
/// Instances a list starts with, and the most it grows to: a pair with a
/// bundle adds one instance per lane. Past the cap the rest are dropped and
/// reported once.
const MIN_INSTANCES: usize = 256;
const MAX_INSTANCES: usize = 65_536;
/// Words per instance record: from (3), lane, to (3), id.
const INSTANCE_WORDS: u32 = 8;

/// Renderer debug modes this feature lists (the next free ids after the
/// shading-cost view's 60).
pub const CABLE_DIAGNOSTICS_VIEW: u32 = 70;
pub const CABLE_SOLID_VIEW: u32 = 71;

const DEBUG_VIEWS: &[helio_core::DebugViewDescriptor] = &[
    helio_core::DebugViewDescriptor {
        name: "Cable Diagnostics",
        debug_mode: CABLE_DIAGNOSTICS_VIEW,
        description: "Bars for what each cable stage found (definition rows, texture table, \
                      ribbon and tube instances), a cross at every cable end and a line along \
                      the first cables.",
    },
    helio_core::DebugViewDescriptor {
        name: "Cable Solid",
        debug_mode: CABLE_SOLID_VIEW,
        description: "Cable geometry painted flat magenta, ignoring colour and texture.",
    },
];

/// `CableUniforms::flags` bits, shared with the shaders.
const FLAG_ROWS_LIVE: u32 = 1;
const FLAG_TEXTURES_OK: u32 = 2;
const FLAG_SOLID: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CableUniforms {
    definition_count: u32,
    frame: u32,
    flags: u32,
    ribbon_count: u32,
    tube_count: u32,
    _pad: u32,
    viewport: [f32; 2],
}

/// One instance: `from`, `lane`, `to`, `id`.
type Instance = [u32; INSTANCE_WORDS as usize];

fn instance(from: [f32; 3], to: [f32; 3], lane: u32, id: u32) -> Instance {
    [
        from[0].to_bits(),
        from[1].to_bits(),
        from[2].to_bits(),
        lane,
        to[0].to_bits(),
        to[1].to_bits(),
        to[2].to_bits(),
        id,
    ]
}

/// The WGSL the shader shares with this file, generated from the Rust
/// constants so the two languages cannot drift.
fn common_wgsl() -> String {
    format!(
        "struct CableUniforms {{\n\
         definition_count: u32, frame: u32, flags: u32, ribbon_count: u32,\n\
         tube_count: u32, _pad: u32, viewport: vec2<f32>,\n\
         }};\n\
         const FLAG_ROWS_LIVE: u32 = {FLAG_ROWS_LIVE}u;\n\
         const FLAG_TEXTURES_OK: u32 = {FLAG_TEXTURES_OK}u;\n\
         const FLAG_SOLID: u32 = {FLAG_SOLID}u;\n\
         const DEFINITION_WORDS: u32 = {}u;\n\
         const INSTANCE_WORDS: u32 = {INSTANCE_WORDS}u;\n\
         const BUNDLE_LANES: u32 = {MAX_BUNDLE_LANES}u;\n\
         const SEGMENTS: u32 = {SEGMENTS}u;\n\
         const TUBE_SIDES: u32 = {TUBE_SIDES}u;\n\
         const TAU: f32 = 6.283185307179586;\n\
         fn fword(word: u32) -> f32 {{ return bitcast<f32>(word); }}\n",
        std::mem::size_of::<cables_api::CableDefinitionRow>() / 4,
    )
}

fn is_cable_view(mode: u32) -> bool {
    mode == CABLE_DIAGNOSTICS_VIEW || mode == CABLE_SOLID_VIEW
}

/// A growable GPU list of instance records.
struct InstanceList {
    label: &'static str,
    buffer: wgpu::Buffer,
    capacity: usize,
}

impl InstanceList {
    fn new(device: &wgpu::Device, label: &'static str, capacity: usize) -> Self {
        Self {
            label,
            buffer: Self::allocate(device, label, capacity),
            capacity,
        }
    }

    fn allocate(device: &wgpu::Device, label: &'static str, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Room for `count` records; true when the buffer was replaced (the
    /// bind group must be rebuilt).
    fn reserve(&mut self, device: &wgpu::Device, count: usize) -> bool {
        if count <= self.capacity {
            return false;
        }
        self.capacity = count.next_power_of_two();
        self.buffer = Self::allocate(device, self.label, self.capacity);
        true
    }
}

fn storage_entry(
    binding: u32,
    read_only: bool,
    stage: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    size: u64,
    stage: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// Draws the frame's cables: the spans the host published
/// ([`CABLE_SPANS`]) as camera-facing ribbons and 3D tubes, each looking up
/// its definition row by ID.
pub struct CableRenderPass {
    ribbon_pipeline: wgpu::RenderPipeline,
    tube_pipeline: wgpu::RenderPipeline,
    /// Fullscreen diagnostics view; binds group 0 only.
    debug_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    texture_binding: helio_mats::MaterialBindingConfig,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_version: Option<u64>,
    uniforms: wgpu::Buffer,
    fallback_definitions: wgpu::Buffer,
    ribbons: InstanceList,
    tubes: InstanceList,
    /// The spans the lists were last built from.
    uploaded: Option<Arc<CableSpans>>,
    overflow_reported: bool,
    ribbon_count: u32,
    tube_count: u32,
    definition_count: u32,
    rows_live: bool,
    debug_mode: u32,
    bind_group: Option<wgpu::BindGroup>,
    bound_buffers: Option<[wgpu::Buffer; 4]>,
}

impl CableRenderPass {
    pub fn new(
        device: &wgpu::Device,
        texture_binding: helio_mats::MaterialBindingConfig,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        // The debug views read the rows and counts in the fragment stage too.
        let v = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cable render bindings"),
            entries: &[
                uniform_entry(0, std::mem::size_of::<CableUniforms>() as u64, v),
                storage_entry(1, true, v),
                // The WGSL camera prefix is view, projection and
                // view-projection: three 64-byte matrices.
                uniform_entry(2, 192, v),
                storage_entry(3, true, v),
                storage_entry(4, true, v),
            ],
        });
        let mut texture_entries = Vec::new();
        texture_binding.append_layout_entries(
            &mut texture_entries,
            0,
            wgpu::ShaderStages::FRAGMENT,
        );
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Cable scene texture table"),
                entries: &texture_entries,
            });
        let source = cable_shader_source(texture_binding);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GPU cable vertex generation"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cable render pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout), Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });
        let debug_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cable debug pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = |label, layout, vertex_entry, fragment_entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment_entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            ribbon_pipeline: pipeline("GPU cable ribbons", &layout, "vs_ribbon", "fs_main"),
            tube_pipeline: pipeline("GPU cable tubes", &layout, "vs_tube", "fs_main"),
            debug_pipeline: pipeline("Cable diagnostics view", &debug_layout, "vs_debug", "fs_debug"),
            bind_group_layout,
            texture_binding,
            texture_bind_group_layout,
            texture_bind_group: None,
            texture_version: None,
            uniforms: buffer(
                "Cable view uniforms",
                std::mem::size_of::<CableUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            fallback_definitions: buffer(
                "Empty cable definition rows",
                std::mem::size_of::<cables_api::CableDefinitionRow>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            ribbons: InstanceList::new(device, "Cable ribbon instances", MIN_INSTANCES),
            tubes: InstanceList::new(device, "Cable tube instances", MIN_INSTANCES),
            uploaded: None,
            overflow_reported: false,
            ribbon_count: 0,
            tube_count: 0,
            definition_count: 0,
            rows_live: false,
            debug_mode: 0,
            bind_group: None,
            bound_buffers: None,
        }
    }

    /// Rebuild both instance lists from `spans` and upload them.
    fn upload(&mut self, ctx: &PrepareContext, spans: &CableSpans) {
        let mut ribbons: Vec<Instance> = Vec::new();
        let mut tubes: Vec<Instance> = Vec::new();
        let mut dropped = false;
        for span in &spans.0 {
            if span.tube {
                for lane in 0..span.lanes.clamp(1, MAX_BUNDLE_LANES) {
                    if tubes.len() == MAX_INSTANCES {
                        dropped = true;
                        break;
                    }
                    tubes.push(instance(span.from, span.to, lane, span.id));
                }
            } else if ribbons.len() < MAX_INSTANCES {
                ribbons.push(instance(span.from, span.to, 0, span.id));
            } else {
                dropped = true;
            }
        }
        if dropped && !self.overflow_reported {
            self.overflow_reported = true;
            tracing::warn!("cables: more than {MAX_INSTANCES} strands in a list; the rest are not drawn");
        }
        let mut grew = self.ribbons.reserve(ctx.device, ribbons.len());
        grew |= self.tubes.reserve(ctx.device, tubes.len());
        if grew {
            self.bound_buffers = None;
        }
        if !ribbons.is_empty() {
            ctx.write_buffer(&self.ribbons.buffer, 0, bytemuck::cast_slice(&ribbons));
        }
        if !tubes.is_empty() {
            ctx.write_buffer(&self.tubes.buffer, 0, bytemuck::cast_slice(&tubes));
        }
        self.ribbon_count = ribbons.len() as u32;
        self.tube_count = tubes.len() as u32;
    }
}

impl RenderPass for CableRenderPass {
    fn name(&self) -> &'static str {
        "CableRenderPass"
    }

    fn scene_buffers(&self) -> &'static [&'static str] {
        &["cable_definitions"]
    }

    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        self.rows_live = scene.holds_rows("cable_definition_sources");
        // A cable debug view runs the pass without rows so it can say so.
        self.rows_live || is_cable_view(self.debug_mode)
    }

    fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
    }

    fn debug_views(&self) -> &'static [helio_core::DebugViewDescriptor] {
        DEBUG_VIEWS
    }

    fn debug_view_category(&self, _debug_mode: u32) -> &'static str {
        "Cables"
    }

    fn reads(&self) -> &'static [&'static str] {
        &[helio_frame_api::color::SCENE_COLOR]
    }

    fn writes(&self) -> &'static [&'static str] {
        &[helio_frame_api::color::SCENE_COLOR]
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read(helio_frame_api::color::SCENE_COLOR);
        builder.read(helio_mats::MATERIAL_TEXTURES);
        builder.read("cable_definitions");
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let scene_color = resources
            .get(helio_core::ResourceKey::new(helio_frame_api::color::SCENE_COLOR))
            .unwrap_or(target);
        let attachments = storage.retain_boxed_slice(Box::new([Some(
            wgpu::RenderPassColorAttachment {
                view: scene_color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            },
        )]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Cable overlay"),
            color_attachments: attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        self.definition_count = ctx.scene_buffers.get(CABLE_DEFINITIONS.key()).map_or(0, |h| {
            (h.buffer.size() / std::mem::size_of::<cables_api::CableDefinitionRow>() as u64)
                .min(u64::from(MAX_DEFINITIONS)) as u32
        });

        // The publisher replaces the payload only when it changed, so an
        // unchanged pointer means unchanged lists.
        let spans = ctx.scene_buffers.payload(&CABLE_SPANS);
        let changed = match (&self.uploaded, &spans) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };
        if changed {
            match &spans {
                Some(spans) => self.upload(ctx, spans),
                None => {
                    self.ribbon_count = 0;
                    self.tube_count = 0;
                }
            }
            self.uploaded = spans;
        }

        let textures_ok = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
            .is_some();
        let flags = u32::from(self.rows_live) * FLAG_ROWS_LIVE
            | u32::from(textures_ok) * FLAG_TEXTURES_OK
            | u32::from(self.debug_mode == CABLE_SOLID_VIEW) * FLAG_SOLID;
        ctx.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&CableUniforms {
                definition_count: self.definition_count,
                frame: ctx.frame_num as u32,
                flags,
                ribbon_count: self.ribbon_count,
                tube_count: self.tube_count,
                _pad: 0,
                viewport: [ctx.width as f32, ctx.height as f32],
            }),
        );
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let diagnostics = self.debug_mode == CABLE_DIAGNOSTICS_VIEW;
        let nothing = self.definition_count == 0 || (self.ribbon_count | self.tube_count) == 0;
        if !diagnostics && nothing {
            return Ok(());
        }
        let definitions = ctx
            .scene_buffers
            .get(CABLE_DEFINITIONS.key())
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_definitions);
        let key = [
            definitions.clone(),
            ctx.camera.clone(),
            self.ribbons.buffer.clone(),
            self.tubes.buffer.clone(),
        ];
        if self.bound_buffers.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable scene rows"),
                layout: &self.bind_group_layout,
                entries: &[
                    entry(0, &self.uniforms),
                    entry(1, definitions),
                    entry(2, ctx.camera),
                    entry(3, &self.ribbons.buffer),
                    entry(4, &self.tubes.buffer),
                ],
            }));
            self.bound_buffers = Some(key);
        }
        if diagnostics {
            let mut pass = ctx.render_cmds().ok_or_else(|| {
                helio_core::Error::ResourceNotFound("cable overlay render pass".into())
            })?;
            pass.set_pipeline(&self.debug_pipeline);
            pass.set_bind_group(0, self.bind_group.as_ref().expect("bind group built"), &[]);
            pass.draw(0..3, 0..1);
            return Ok(());
        }
        let textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures());
        let Some(textures) = textures else {
            return Ok(());
        };
        let mut pass = ctx.render_cmds().ok_or_else(|| {
            helio_core::Error::ResourceNotFound("cable overlay render pass".into())
        })?;
        if self.texture_version != Some(textures.version) || self.texture_bind_group.is_none() {
            let mut entries = Vec::new();
            self.texture_binding.append_bind_group_entries(
                &mut entries,
                0,
                textures.texture_views,
                textures.sampler,
            );
            self.texture_bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable material texture bindings"),
                layout: &self.texture_bind_group_layout,
                entries: &entries,
            }));
            self.texture_version = Some(textures.version);
        }
        pass.set_bind_group(0, self.bind_group.as_ref().expect("bind group built"), &[]);
        pass.set_bind_group(1, self.texture_bind_group.as_ref().expect("texture group built"), &[]);
        if self.ribbon_count > 0 {
            pass.set_pipeline(&self.ribbon_pipeline);
            pass.draw(0..RIBBON_VERTICES, 0..self.ribbon_count);
        }
        if self.tube_count > 0 {
            pass.set_pipeline(&self.tube_pipeline);
            pass.draw(0..TUBE_VERTICES, 0..self.tube_count);
        }
        Ok(())
    }
}

fn cable_shader_source(binding: helio_mats::MaterialBindingConfig) -> String {
    let src = include_str!("cables.wgsl").replace("// @common", &common_wgsl());
    if binding.uses_binding_arrays() {
        src.replace(
            "binding_array<texture_2d<f32>, 256>",
            &format!("binding_array<texture_2d<f32>, {}>", binding.max_textures),
        )
    } else {
        helio_mats::apply_webgpu_decal_bindings(&src, binding.max_textures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("the shader parses: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("the shader validates");
    }

    #[test]
    fn the_uniform_block_matches_its_wgsl_layout() {
        // five counters, a pad, then the viewport at offset 24.
        assert_eq!(std::mem::size_of::<CableUniforms>(), 32);
    }

    #[test]
    fn an_instance_is_eight_words() {
        assert_eq!(std::mem::size_of::<Instance>(), INSTANCE_WORDS as usize * 4);
    }

    #[test]
    fn the_draw_shader_validates_as_compiled() {
        validate(&include_str!("cables.wgsl").replace("// @common", &common_wgsl()));
    }
}
