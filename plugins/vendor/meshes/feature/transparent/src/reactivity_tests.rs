use super::*;

#[test]
fn graph_routed_coverage_is_attached_and_cleared_without_draws() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let pass = TransparentPass::new(&device, wgpu::TextureFormat::Rgba16Float)
            .with_pre_aa_target()
            .with_reactive_mask();
        for width in [32, 64] {
            let extent = wgpu::Extent3d {
                width,
                height: 8,
                depth_or_array_layers: 1,
            };
            let texture = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = texture(
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let depth = texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let coverage = texture(
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
            );
            let color_view = color.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            let coverage_view = coverage.create_view(&Default::default());
            queue.write_texture(
                coverage.as_image_copy(),
                &vec![255; (width * 8) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width),
                    rows_per_image: Some(8),
                },
                extent,
            );
            let mut resources = helio_core::ResourceRegistry::empty();
            // Graph-owned attachments are bindings, not typed resource slots.
            resources.write_texture_binding("transparency_reactivity", &coverage_view, "test");
            resources.write_texture_view(
                helio_core::ResourceKey::new("pre_aa"),
                &color_view,
                "test",
            );
            let mut storage = helio_core::RenderFrameStorage::new();
            let descriptor = pass
                .render_pass_descriptor_with_storage(
                    &color_view,
                    &depth_view,
                    &resources,
                    &mut storage,
                )
                .expect("graph-owned coverage must not suppress the transparent pass");
            assert_eq!(descriptor.color_attachments.len(), 2);
            assert_eq!(
                descriptor.color_attachments[1].as_ref().unwrap().view,
                &coverage_view
            );
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let _clear = encoder.begin_render_pass(&descriptor);
            }
            encoder.copy_texture_to_buffer(
                coverage.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(8),
                    },
                },
                extent,
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            read.slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let data = read.slice(..).get_mapped_range().unwrap();
            for row in data.chunks_exact(256) {
                assert!(row[..width as usize].iter().all(|v| *v == 0));
            }
        }
    });
}

#[test]
fn production_pipelines_accept_coverage_and_legacy_templates() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, _) = adapter.request_device(&Default::default()).await.unwrap();
        for coverage in [false, true] {
            for legacy in [false, true] {
                let mut pass = TransparentPass::new(&device, wgpu::TextureFormat::Rgba16Float);
                pass.reactive_mask = coverage;
                if legacy {
                    // A registered template written before coverage support has
                    // one color output and must remain usable with either graph.
                    let source = pass.local_class0.wgsl_source
                        .replace("HELIO_TRANSPARENT_REACTIVITY", "LEGACY_TEMPLATE")
                        .replace("-> TransparentOutput", "-> @location(0) vec4<f32>")
                        .replace("return TransparentOutput(surface, vec4<f32>(clamp(surface.a, 0.0, 1.0)));", "return surface;");
                    pass.local_class0.wgsl_source = source.into();
                }
                pass.get_or_create_pipeline(
                    &device,
                    RadiantShaderKey {
                        template_id: 0,
                        graph_hash: 0,
                        feature_flags: 0,
                    },
                    "",
                );
            }
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    });
}
