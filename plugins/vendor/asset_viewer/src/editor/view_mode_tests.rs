//! GPU tests for the viewer's diagnostic modes (normals, UV 1, UV 2).
//!
//! Each renders one full-screen triangle through the viewer's real mesh
//! pipeline and checks the centre pixel, so the modes show the data they
//! claim to. They need a GPU adapter; absence fails rather than skipping.

use super::panel_render::{
    create_mesh_pipeline, mesh_vertex_layout, uv_grid_layout, MESH_VERTEX_SRC,
};

/// The UV modes' test texture, row by row (RGBA): red, green / blue, white.
/// UV (0.25, 0.25) falls in the red texel, (0.75, 0.25) in the green one.
const GRID: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]];

const NORMALS: u32 = 3;
const UV0: u32 = 4;
const UV1: u32 = 5;

/// Centre pixel (B, G, R, A) of the triangle rendered in `mode`. The vertices
/// carry normal +Z, UV channel 1 = (0.25, 0.25) and UV channel 2 = (0.75, 0.25).
fn render_mode(mode: u32) -> [u8; 4] {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("GPU adapter required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let format = wgpu::TextureFormat::Bgra8Unorm;

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    });
    let grid_layout = uv_grid_layout(&device);
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout), Some(&grid_layout)],
        immediate_size: 0,
    });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(MESH_VERTEX_SRC.into()),
    });
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: 8,
        height: 8,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    let pipeline = create_mesh_pipeline(
        &device,
        &config,
        &pipeline_layout,
        &module,
        wgpu::PrimitiveTopology::TriangleList,
        Some(wgpu::Face::Back),
        "view mode test",
    );
    if let Some(error) = pollster::block_on(scope.pop()) {
        panic!("viewer mesh shader failed to build: {error}");
    }

    // View-projection identity; the mode goes in the render-mode word.
    let mut uniforms = [0u8; 96];
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    uniforms[..64].copy_from_slice(bytemuck::cast_slice(&identity));
    uniforms[64..68].copy_from_slice(&mode.to_le_bytes());
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, &uniforms);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform_buffer,
                offset: 0,
                size: wgpu::BufferSize::new(96),
            }),
        }],
    });

    // The UV modes sample a 2x2 grid, one colour per quadrant (see `GRID`),
    // without filtering: the colour tells which UV the mode used.
    let grid = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 2,
            height: 2,
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
        grid.as_image_copy(),
        bytemuck::cast_slice(&GRID),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        grid.size(),
    );
    let grid_view = grid.create_view(&Default::default());
    let grid_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let grid_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &grid_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&grid_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&grid_sampler),
            },
        ],
    });

    // position, normal, uv0, uv1; one clockwise triangle covering the target.
    let vertex = |x: f32, y: f32| [x, y, 0.5, 0.0, 0.0, 1.0, 0.25, 0.25, 0.75, 0.25];
    let vertices: Vec<f32> = [vertex(-1.0, -1.0), vertex(-1.0, 3.0), vertex(3.0, -1.0)]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(vertices.len() % 3, 0);
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (vertices.len() * 4) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));
    assert_eq!(mesh_vertex_layout().array_stride, (vertices.len() / 3 * 4) as u64);

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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[0]);
        pass.set_bind_group(1, &grid_group, &[]);
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

fn near(actual: u8, expected: u8) -> bool {
    actual.abs_diff(expected) <= 2
}

/// Normal +Z maps to (0.5, 0.5, 1.0): B 255, G 128, R 128 in BGRA.
#[test]
fn normals_mode_shows_the_vertex_normal() {
    let [b, g, r, a] = render_mode(NORMALS);
    assert!(near(b, 255) && near(g, 128) && near(r, 128) && a == 255, "{b} {g} {r} {a}");
}

/// UV channel 1 is (0.25, 0.25): the grid's red texel.
#[test]
fn uv_one_mode_shows_the_first_channel() {
    let [b, g, r, _] = render_mode(UV0);
    assert!(near(r, 255) && near(g, 0) && near(b, 0), "{b} {g} {r}");
}

/// UV channel 2 is (0.75, 0.25): the green texel. Different from channel 1,
/// so the two modes cannot be showing the same data.
#[test]
fn uv_two_mode_shows_the_second_channel() {
    let [b, g, r, _] = render_mode(UV1);
    assert!(near(r, 0) && near(g, 255) && near(b, 0), "{b} {g} {r}");
}
