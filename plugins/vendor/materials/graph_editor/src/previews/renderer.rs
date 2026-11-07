//! A pin's preview: the pin's graph, compiled by the materials' compiler,
//! drawn unlit on a quad with the materials' preview program
//! (`materials_component::preview_program`).

use materials_component::graph_preview::GraphPreview;
use materials_component::preview_program::{self, GraphDraw, PreviewUniforms};
use wgpu::util::DeviceExt;
use wgpu::*;

/// The quad's vertex: position, normal, UV (the preview program's inputs).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

/// A unit quad facing +Z whose positions are `uv * 2 - 1` (what a graph's
/// world position reads on a pin preview).
fn quad() -> [QuadVertex; 6] {
    let corner = |u: f32, v: f32| QuadVertex {
        position: [u * 2.0 - 1.0, v * 2.0 - 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
        uv: [u, v],
    };
    [
        corner(0.0, 1.0),
        corner(1.0, 1.0),
        corner(1.0, 0.0),
        corner(0.0, 1.0),
        corner(1.0, 0.0),
        corner(0.0, 0.0),
    ]
}

/// Quad to clip space: `uv` (0, 0) is the top-left texel.
const VIEW_PROJ: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, -1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub struct PinPreviewRenderer {
    draw: GraphDraw,
    vertices: Buffer,
    uniform_bind_group: BindGroup,
    globals_buffer: Buffer,
    globals_bind_group: BindGroup,
}

impl PinPreviewRenderer {
    /// The preview program writes display-encoded colour itself.
    pub const TEXTURE_FORMAT: TextureFormat = TextureFormat::Bgra8Unorm;

    pub fn new(device: &Device, queue: &Queue, compiled: &GraphPreview) -> Result<Self, String> {
        let uniform_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("pin_preview_uniform_layout"),
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
        let uniforms = PreviewUniforms {
            view_proj: VIEW_PROJ,
            // Unlit: the pin's value as it is.
            render_mode: [1, 0, 0, 0],
            base_color: [1.0; 4],
        };
        let uniform_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("pin_preview_uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: BufferUsages::UNIFORM,
        });
        let uniform_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("pin_preview_uniforms"),
            layout: &uniform_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let globals_layout = preview_program::globals_layout(device);
        let globals_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("pin_preview_clock"),
            size: preview_program::GLOBALS_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("pin_preview_clock"),
            layout: &globals_layout,
            entries: &[BindGroupEntry {
                binding: 1,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let context = preview_program::GraphContext {
            device,
            queue,
            target_format: Self::TEXTURE_FORMAT,
            globals_layout: &globals_layout,
            uniform_layout: &uniform_layout,
            depth_format: None,
            // `VIEW_PROJ` flips Y, which turns the quad's winding.
            front_face: FrontFace::Cw,
        };
        let layout = VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: &vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        };
        let draw = preview_program::build(&context, compiled, &[Some(layout)])?;
        let vertices = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("pin_preview_quad"),
            contents: bytemuck::cast_slice(&quad()),
            usage: BufferUsages::VERTEX,
        });
        Ok(Self {
            draw,
            vertices,
            uniform_bind_group,
            globals_buffer,
            globals_bind_group,
        })
    }

    pub fn render(&self, device: &Device, queue: &Queue, output: &TextureView, time: f32) {
        preview_program::write_globals_at(queue, &self.globals_buffer, 0, time);

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("pin_preview_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("pin_preview_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.draw.pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, &self.draw.textures, &[]);
            pass.set_bind_group(2, &self.draw.empty, &[]);
            pass.set_bind_group(3, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..6, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}
