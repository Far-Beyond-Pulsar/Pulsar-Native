//! GPU tests for shader-graph materials in the viewer.
//!
//! These render through the viewer's real graph pipeline on a real device,
//! so they need a GPU adapter; absence fails rather than skipping.

use helio_component::graph_preview::GraphPreview;
use psgc::*;

use super::graph_material::{
    build, globals_layout, write_globals_at, GraphContext, GLOBALS_SIZE,
};

fn node(id: &str, kind: &str) -> NodeInstance {
    let meta = get_shader_nodes()
        .into_iter()
        .find(|m| m.name == kind)
        .unwrap();
    let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
    for p in meta.params {
        node.inputs.push(PinInstance::new(
            &p.name,
            Pin::new(
                &p.name,
                &p.name,
                DataType::Data(TypeInfo::new(&p.param_type)),
                PinType::Input,
            ),
        ));
    }
    node
}

fn link(graph: &mut GraphDescription, from: &str, to: &str, pin: &str) {
    graph.add_connection(Connection::new(from, "result", to, pin, ConnectionType::Data));
}

/// Write `graph` as a material asset in a temp project and compile it.
fn compile(graph: &GraphDescription) -> (tempfile::TempDir, GraphPreview) {
    let dir = tempfile::tempdir().unwrap();
    let material = dir.path().join("M.material");
    std::fs::create_dir_all(&material).unwrap();
    std::fs::write(
        material.join("shader_graph_save.json"),
        serde_json::json!({ "main_graph": graph }).to_string(),
    )
    .unwrap();
    let preview = helio_component::graph_preview::compile_graph_preview(dir.path(), "M.material")
        .expect("a graph material")
        .expect("compiles");
    (dir, preview)
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("GPU adapter required");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
}

fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// A graph asset (uv -> rainbow -> base colour) builds a pipeline.
#[test]
fn a_graph_material_builds_a_viewer_pipeline() {
    let mut graph = GraphDescription::new("viewer regression");
    graph.add_node(node("output", "fragment_output"));
    graph.add_node(node("uv", "frag_uv"));
    graph.add_node(node("rainbow", "rainbow"));
    link(&mut graph, "uv", "rainbow", "uv");
    link(&mut graph, "rainbow", "output", "base_color");
    let (_dir, preview) = compile(&graph);
    assert!(preview.textures.is_empty());

    let (device, queue) = device();
    let uniform_layout = uniform_layout(&device);
    let globals_layout = globals_layout(&device);
    let ctx = GraphContext {
        device: &device,
        queue: &queue,
        target_format: wgpu::TextureFormat::Bgra8Unorm,
        globals_layout: &globals_layout,
        uniform_layout: &uniform_layout,
    };
    let layout = super::panel_render::mesh_vertex_layout();
    if let Err(error) = build(&ctx, &preview, &[Some(layout)]) {
        panic!("graph pipeline failed: {error}");
    }
}

/// Renders one full-screen triangle through the viewer's graph pipeline with
/// the clock at `time`, and returns a centre pixel (BGRA).
fn render_at(preview: &GraphPreview, time: f32) -> [u8; 4] {
    let (device, queue) = device();
    let uniform_layout = uniform_layout(&device);
    let globals_layout = globals_layout(&device);
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let ctx = GraphContext {
        device: &device,
        queue: &queue,
        target_format: format,
        globals_layout: &globals_layout,
        uniform_layout: &uniform_layout,
    };
    let draw = build(
        &ctx,
        preview,
        &[Some(super::panel_render::mesh_vertex_layout())],
    )
    .expect("pipeline");

    // View-projection identity, unlit mode.
    let mut uniforms = [0u8; 96];
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    uniforms[..64].copy_from_slice(bytemuck::cast_slice(&identity));
    uniforms[64..68].copy_from_slice(&1u32.to_le_bytes());
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, &uniforms);
    let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform_buffer,
                offset: 0,
                size: wgpu::BufferSize::new(96),
            }),
        }],
    });
    let globals = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: GLOBALS_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    write_globals_at(&queue, &globals, 1, time);
    let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &globals_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: globals.as_entire_binding(),
        }],
    });
    let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[],
    });
    let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &empty_layout,
        entries: &[],
    });

    // One clockwise triangle covering the whole target; UV (0.25, 0.25).
    let vertex = |x: f32, y: f32| [x, y, 0.5, 0.0, 0.0, 1.0, 0.25, 0.25];
    let vertices: Vec<f32> = [vertex(-1.0, -1.0), vertex(-1.0, 3.0), vertex(3.0, -1.0)]
        .into_iter()
        .flatten()
        .collect();
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (vertices.len() * 4) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));

    let size = wgpu::Extent3d {
        width: 8,
        height: 8,
        depth_or_array_layers: 1,
    };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(
        format,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = texture(
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let (color_view, depth_view) = (
        color.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    );
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 8,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
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
        pass.set_pipeline(&draw.pipeline);
        pass.set_bind_group(0, &globals_group, &[]);
        pass.set_bind_group(1, &draw.textures, &[]);
        pass.set_bind_group(2, &empty, &[]);
        pass.set_bind_group(3, &uniform_group, &[0]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(8),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().expect("mapped");
    let offset = 4 * 256 + 4 * 4;
    [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]
}

/// A `time` node drives the rainbow's shift: the picture must change with the
/// clock. A graph frozen at time zero would render both frames identically.
#[test]
fn a_graph_material_animates_with_the_clock() {
    let mut graph = GraphDescription::new("animated");
    graph.add_node(node("output", "fragment_output"));
    graph.add_node(node("uv", "frag_uv"));
    graph.add_node(node("time", "time"));
    graph.add_node(node("rainbow", "rainbow"));
    link(&mut graph, "uv", "rainbow", "uv");
    link(&mut graph, "time", "rainbow", "shift");
    link(&mut graph, "rainbow", "output", "base_color");
    let (_dir, preview) = compile(&graph);
    assert!(
        preview.snippet.contains("radiant_graph_time()"),
        "time must come from the host, not a literal"
    );

    let (at_zero, at_later) = (render_at(&preview, 0.0), render_at(&preview, 1.5));
    assert_ne!(at_zero, at_later, "a time-driven graph must change with the clock");
}
