//! Rasterize the production shadow shader and read actual depth pixels.
use helio_components::MaterialComponent;
use wgpu::util::DeviceExt;

#[test]
fn leaf_opacity_casts_identical_cutout_shadows_from_both_sides() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("GPU required");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let bindings = helio_pass_gbuffer::depth_material::DepthMaterialBindings::new(&device);
        let source = bindings.shader_source(include_str!("../shaders/shadow.wgsl"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Production shadow cutout test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let buffer = |bytes: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::COPY_DST,
            })
        };
        let identity = glam::Mat4::IDENTITY.to_cols_array();
        let mut face = [0f32; 24];
        face[..16].copy_from_slice(&identity);
        let mut inst = [0u32; 56];
        inst[..16].copy_from_slice(bytemuck::cast_slice(&identity));
        let buffers = [
            buffer(bytemuck::cast_slice(&face)),
            buffer(bytemuck::cast_slice(&inst)),
            buffer(&[0; 16]),
            buffer(bytemuck::cast_slice(&identity)),
            buffer(&[0; 4]),
            buffer(&[0; 32]),
            buffer(bytemuck::cast_slice(&identity)),
            buffer(&[0; 8]),
        ];
        let entries: Vec<_> = (0..8)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: if i == 2 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout0,
            entries: &entries,
        });
        let mut material = MaterialComponent::from(helio_mats::MaterialRow::new(
            [1.0; 4], 0.8, 0.0, [0.0; 3], 0.0,
        ));
        material.flags = 5; // double-sided + alpha-test
        material.tex_base_color = 0;
        let material_buffer = buffer(bytemuck::bytes_of(&material));
        let mut metadata = [0u32; 88];
        metadata[87] = (-1.0f32).to_bits();
        let metadata_buffer = buffer(bytemuck::cast_slice(&metadata));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let pixels: Vec<u8> = (0..64)
            .flat_map(|i| [40, 120, 30, if i % 8 < 4 { 255 } else { 0 }])
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(32),
                rows_per_image: Some(8),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&Default::default());
        // Baseline binding layout reserves five sampled slots; this test's
        // default device has sixteen sampled slots, leaving eleven materials.
        let count = (device.limits().max_sampled_textures_per_shader_stage as usize)
            .saturating_sub(5)
            .min(device.limits().max_samplers_per_shader_stage as usize)
            .min((device.limits().max_bindings_per_bind_group as usize).saturating_sub(2) / 2)
            .min(
                if cfg!(any(
                    target_arch = "wasm32",
                    target_os = "macos",
                    target_os = "ios",
                    target_os = "android"
                )) {
                    16
                } else {
                    256
                },
            );
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: material_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: metadata_buffer.as_entire_binding(),
            },
        ];
        for i in 0..count {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: wgpu::BindingResource::TextureView(&view),
            });
        }
        for i in 0..count {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + count as u32 + i as u32,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bindings.layout,
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout0), Some(&bindings.layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 40,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 16,
                                shader_location: 1,
                            },
                        ],
                    }),
                    Some(helio_pass_gbuffer::DRAW_SLOT_LAYOUT),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let vertices: Vec<_> = [
            ([-1., -1., 0.5], [0., 1.]),
            ([1., -1., 0.5], [1., 1.]),
            ([1., 1., 0.5], [1., 0.]),
            ([-1., 1., 0.5], [0., 0.]),
        ]
        .into_iter()
        .map(|(position, tex_coords0)| helio_mesh_api::PackedVertex {
            position,
            tex_coords0,
            ..Default::default()
        })
        .collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let slots = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 4],
            usage: wgpu::BufferUsages::VERTEX,
        });
        for indices in [[0u32, 1, 2, 0, 2, 3], [2u32, 1, 0, 3, 2, 0]] {
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let depth_view = depth.create_view(&Default::default());
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 8 * 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[],
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
                pass.set_bind_group(0, &group0, &[]);
                pass.set_bind_group(1, &group1, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_vertex_buffer(1, slots.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..6, 0, 0..1);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(8),
                    },
                },
                depth.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().unwrap();
            for y in 0..8 {
                for x in 0..8 {
                    let offset = y * 256 + x * 4;
                    let d = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
                    assert_eq!(
                        d,
                        if x < 4 { 0.5 } else { 1.0 },
                        "wrong leaf shadow at ({x},{y})"
                    );
                }
            }
        }
    });
}
