//! Shader-graph materials in the mesh viewer.
//!
//! The graph is compiled by the same code the renderer uses
//! (`materials_component::graph_preview`) and composed into the default material
//! class's template exactly as a drawer composes it
//! (`RadiantTemplate::compose`), so `radiant_eval_surface` produces the same
//! albedo, emissive and alpha the engine would. The viewer's program is a
//! drawer's: a vertex stage (position, normal, UV) and a small fragment
//! stage that lights the surface.

use materials_component::graph_preview::GraphPreview;

/// The viewer's program. The material table's bindings are declared as every
/// drawer declares them (group 1; the viewer binds only the textures, which
/// is all its stages read); the uniforms live in group 3.
const VIEWER_PROGRAM: &str = r#"
enable wgpu_binding_array;
//!use pbr_eval
//!use material_row
//!use specular_aa
//!use material_surface

// The G-buffer's `Globals` layout: graph `time` nodes read `time`.
struct Globals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: vec4<f32>,
    csm_splits: vec4<f32>,
    debug_mode: u32,
    screen_width: f32,
    screen_height: f32,
    time: f32, // RADIANT_GLOBALS_TIME
}
@group(0) @binding(1) var<uniform> globals: Globals;

@group(1) @binding(0) var<storage, read> materials: array<GpuMaterial>;
@group(1) @binding(1) var<storage, read> material_textures: array<MaterialTextureData>;
@group(1) @binding(2) var scene_textures: binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var scene_samplers: binding_array<sampler, 256>;

struct ViewerUniforms {
    view_proj: mat4x4<f32>,
    render_mode: vec4<u32>,
    base_color: vec4<f32>,
};
@group(3) @binding(0) var<uniform> viewer_uniforms: ViewerUniforms;

struct ViewerVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) tex_coords: vec2<f32>,
};

@vertex fn viewer_vs(v: ViewerVertex) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = viewer_uniforms.view_proj * vec4<f32>(v.position, 1.0);
    out.world_position = v.position;
    out.world_normal = v.normal;
    out.tex_coords = v.uv;
    return out;
}

@fragment fn viewer_fs(input: VertexOutput) -> @location(0) vec4<f32> {
    var material: GpuMaterial;
    material.base_color = vec4<f32>(1.0);
    material.roughness_metallic = vec4<f32>(0.5, 0.0, 1.5, 1.0);
    var textures: MaterialTextureData;
    textures.base_color.texture_index = MATERIAL_NO_TEXTURE;
    textures.normal.texture_index = MATERIAL_NO_TEXTURE;
    textures.roughness_metallic.texture_index = MATERIAL_NO_TEXTURE;
    textures.emissive.texture_index = MATERIAL_NO_TEXTURE;
    textures.occlusion.texture_index = MATERIAL_NO_TEXTURE;
    textures.specular_color.texture_index = MATERIAL_NO_TEXTURE;
    textures.specular_weight.texture_index = MATERIAL_NO_TEXTURE;
    let surface = radiant_eval_surface(material, textures, MaterialInput(
        input.clip_position, input.world_position, input.world_normal,
        vec3<f32>(1.0, 0.0, 0.0), 1.0, input.tex_coords,
        vec3<f32>(0.0), globals.frame,
    ));

    var rgb = surface.albedo.rgb;
    if viewer_uniforms.render_mode.x == 0u {
        let n = normalize(input.world_normal);
        let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.8));
        rgb = rgb * (0.3 + 0.7 * max(dot(n, light_dir), 0.0));
    }
    rgb = clamp(rgb + surface.emissive, vec3<f32>(0.0), vec3<f32>(1.0));
    // Surface values are linear; the viewer's target is not sRGB.
    return vec4<f32>(pow(rgb, vec3<f32>(1.0 / 2.2)), 1.0);
}
"#;

/// Byte offsets inside the viewer program's `Globals` (the G-buffer's layout):
/// `frame: u32` is first and `time: f32` follows `screen_height`.
const GLOBALS_FRAME_OFFSET: u64 = 0;
const GLOBALS_TIME_OFFSET: u64 = 60;
/// Room for the whole `Globals` block.
pub const GLOBALS_SIZE: u64 = 256;

/// Layout of the `Globals` buffer the program reads at group 0, binding 1.
/// Graph `time` nodes reach the clock through it.
pub fn globals_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("graph material globals layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// Advance the clock the graph's `time` node reads. The preview is always
/// live, so its clock is wall time since the first preview frame (the
/// renderer's passes read their host's frame clock instead).
pub fn write_globals(queue: &wgpu::Queue, buffer: &wgpu::Buffer, frame: u32) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let time = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f32();
    write_globals_at(queue, buffer, frame, time);
}

/// [`write_globals`] with an explicit clock value.
pub fn write_globals_at(queue: &wgpu::Queue, buffer: &wgpu::Buffer, frame: u32, time: f32) {
    queue.write_buffer(buffer, GLOBALS_FRAME_OFFSET, &frame.to_le_bytes());
    queue.write_buffer(buffer, GLOBALS_TIME_OFFSET, &time.to_le_bytes());
}

/// A compiled graph material: its pipeline and its texture bind group.
pub struct GraphDraw {
    pub pipeline: wgpu::RenderPipeline,
    pub textures: wgpu::BindGroup,
}

/// Everything a graph pipeline shares with the viewer's own mesh pipeline.
pub struct GraphContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub target_format: wgpu::TextureFormat,
    /// Layout of the template's `Globals` buffer (group 0, binding 1).
    pub globals_layout: &'a wgpu::BindGroupLayout,
    /// Layout of the per-draw uniform buffer (bound at group 3).
    pub uniform_layout: &'a wgpu::BindGroupLayout,
}

fn load_texture(
    ctx: &GraphContext<'_>,
    path: &std::path::Path,
) -> Result<wgpu::TextureView, String> {
    let rgba = image::open(path)
        .map_err(|error| format!("texture {}: {error}", path.display()))?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(upload_rgba(ctx, width, height, rgba.as_raw()))
}

fn upload_rgba(ctx: &GraphContext<'_>, width: u32, height: u32, data: &[u8]) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("graph material texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Build the pipeline for `preview`. Shader or pipeline errors come back as
/// `Err` instead of reaching wgpu's panicking default handler, so a broken
/// graph leaves the viewer showing the slot's plain colour.
pub fn build(
    ctx: &GraphContext<'_>,
    preview: &GraphPreview,
    vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
) -> Result<GraphDraw, String> {
    let device = ctx.device;
    // At least one slot: the template's binding table is never empty.
    let slots = preview.textures.len().max(1);

    let mut views = Vec::with_capacity(slots);
    for path in &preview.textures {
        views.push(load_texture(ctx, path)?);
    }
    if views.is_empty() {
        views.push(upload_rgba(ctx, 1, 1, &[255, 255, 255, 255]));
    }
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let binding = helio_mats::MaterialBindingConfig {
        mode: helio_mats::MaterialBindingMode::Expanded,
        max_textures: slots,
    };
    let mut layout_entries = Vec::new();
    binding.append_layout_entries(&mut layout_entries, 2, wgpu::ShaderStages::FRAGMENT);
    let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("graph material textures layout"),
        entries: &layout_entries,
    });
    let view_refs: Vec<&wgpu::TextureView> = views.iter().collect();
    let sampler_refs: Vec<&wgpu::Sampler> = (0..slots).map(|_| &sampler).collect();
    let mut entries = Vec::new();
    binding.append_bind_group_entries(&mut entries, 2, &view_refs, &sampler_refs);
    let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("graph material textures"),
        layout: &textures_layout,
        entries: &entries,
    });

    let template = helio_mats::RadiantTemplateRegistry::new();
    let template = template
        .get(helio_mats::MATERIAL_CLASS_DEFAULT)
        .ok_or("the default Radiant template is missing")?;
    let source = helio_mats::apply_webgpu_material_bindings(
        &template.compose(VIEWER_PROGRAM, &preview.snippet, slots, &[]),
        slots,
    );

    let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("graph material empty layout"),
        entries: &[],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("graph material pipeline layout"),
        bind_group_layouts: &[
            Some(ctx.globals_layout),
            Some(&textures_layout),
            Some(&empty_layout),
            Some(ctx.uniform_layout),
        ],
        immediate_size: 0,
    });

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("graph material shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("graph material pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("viewer_vs"),
            compilation_options: Default::default(),
            buffers: vertex_buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("viewer_fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: ctx.target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
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
    if let Some(error) = pollster::block_on(scope.pop()) {
        return Err(error.to_string());
    }
    Ok(GraphDraw { pipeline, textures })
}
