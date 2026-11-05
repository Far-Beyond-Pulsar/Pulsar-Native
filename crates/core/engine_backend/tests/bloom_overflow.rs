use helio_pass_postprocess::{gpu_types::GpuPostProcessUniforms, with_post_settings};
use wgpu::util::DeviceExt;

#[test]
#[ignore = "requires a GPU"]
fn bloom_bright_sources_remain_finite_in_half_float_targets() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let shader = helio_core::shader::module(
            &device, "Bloom overflow regression", &with_post_settings(include_str!(
                "../../../renderer/helio/crates/passes/3d/helio-pass-postprocess/shaders/postprocess.wgsl")));
        let texture = |usage| device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float, usage, view_formats: &[],
        });
        let source = texture(wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let destination = texture(wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let src = source.create_view(&Default::default());
        let dst = destination.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = GpuPostProcessUniforms {
            bloom_intensity: 4.0, bloom_threshold: 1.0, bloom_knee: 0.5,
            bloom_tint: [1.0; 3], blend_weight_bloom: 1.0, ..Default::default()
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::bytes_of(&uniforms), usage: wgpu::BufferUsages::UNIFORM,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 512, mapped_at_creation: false,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        });
        // Finite HDR overflows when bloom amplifies/adds it; infinity models
        // an already-overflowed scene target (e.g. a near-field specular).
        for half in [0x7bffu16, 0x7c00u16] {
            let pixels = [half; 16];
            queue.write_texture(source.as_image_copy(), bytemuck::cast_slice(&pixels),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(2) },
                source.size());
            for entry in ["cs_bloom_down_extract", "cs_bloom_down", "cs_bloom_upsample"] {
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry), layout: None, module: &shader, entry_point: Some(entry),
                    compilation_options: Default::default(), cache: None,
                });
                let mut entries0 = vec![];
                let (group_index, entries) = if entry == "cs_bloom_upsample" {
                    entries0.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) });
                    (2, vec![
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&src) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&dst) },
                    ])
                } else {
                    if entry == "cs_bloom_down_extract" {
                        entries0.push(wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() });
                        entries0.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&src) });
                    } else {
                        entries0.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) });
                    }
                    let mut entries = vec![wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&dst) }];
                    if entry == "cs_bloom_down" {
                        entries.push(wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) });
                    }
                    (1, entries)
                };
                let bg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None, layout: &pipeline.get_bind_group_layout(0), entries: &entries0,
                });
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None, layout: &pipeline.get_bind_group_layout(group_index), entries: &entries,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &bg0, &[]);
                    pass.set_bind_group(group_index, &bg, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                encoder.copy_texture_to_buffer(destination.as_image_copy(), wgpu::TexelCopyBufferInfo {
                    buffer: &readback, layout: wgpu::TexelCopyBufferLayout {
                        offset: 0, bytes_per_row: Some(256), rows_per_image: Some(2),
                    },
                }, destination.size());
                queue.submit([encoder.finish()]);
                readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let mapped = readback.slice(..).get_mapped_range().unwrap();
                let result: &[u16] = bytemuck::cast_slice(&mapped);
                for channel in &result[..3] {
                    assert_ne!(channel & 0x7c00, 0x7c00, "{entry}, input {half:x}: nonfinite HDR output");
                    assert!(*channel > 0, "bright source became black in {entry}");
                }
                drop(mapped);
                readback.unmap();
            }
        }
    });
}
