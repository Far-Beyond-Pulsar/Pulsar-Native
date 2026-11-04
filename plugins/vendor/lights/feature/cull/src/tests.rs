use super::*;

// Execute the production kernels, including a light above the demo's two
// million object slots. Verify sparse IDs survive compaction and deletion.
#[test]
fn sparse_light_ids_reach_every_tile_and_deleted_lights_disappear() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("GPU required for light-cull regression test");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        device.on_uncaptured_error(std::sync::Arc::new(|error| panic!("{error:?}")));
        let pass = LightCullPass::new(&device, 32, 32);
        // Above the demo's two million slots where the adapter can bind that
        // many lights (128 B each); otherwise the highest ID it can bind, still
        // far past any dense range (lavapipe binds 128 MB: about a million).
        let max_lights = adapter.limits().max_storage_buffer_binding_size as u64 / 128;
        let high = 2_000_003u32.min((max_lights - 2) as u32);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // GpuLight is 128 bytes; intensity is word 11. All other words zero
        // describe a directional light, which must reach all four tiles.
        let lights = buffer("sparse lights", (high as u64 + 1) * 128, usage);
        let active = buffer("active IDs", (high as u64 + 2) * 4, usage);
        let camera = buffer("camera", 576, usage);
        let params = LightCullParams {
            num_tiles_x: 2,
            num_tiles_y: 2,
            num_lights: high + 1,
            screen_width: 32,
            screen_height: 32,
            light_mode_direct_index: 1,
            _pad1: 0,
            _pad2: 0,
        };
        queue.write_buffer(&pass.params_buf, 0, bytemuck::bytes_of(&params));
        let compact = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pass.compact_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pass.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: active.as_entire_binding(),
                },
            ],
        });
        let tiles = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pass.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pass.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: active.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: pass.tile_light_lists.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: pass.tile_light_counts.as_entire_binding(),
                },
            ],
        });
        let staging = buffer(
            "readback",
            16 + 4 * 64 * 4,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        for expected in [vec![7, high], vec![high], vec![]] {
            for i in [7, high] {
                let intensity = if expected.contains(&i) {
                    8.0f32
                } else {
                    0.0f32
                };
                queue.write_buffer(&lights, i as u64 * 128 + 44, bytemuck::bytes_of(&intensity));
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.clear_buffer(&active, 0, Some(4));
            {
                let mut compute = encoder.begin_compute_pass(&Default::default());
                compute.set_pipeline(&pass.compact_pipeline);
                compute.set_bind_group(0, &compact, &[]);
                compute.dispatch_workgroups((high + 1).div_ceil(256), 1, 1);
            }
            {
                let mut compute = encoder.begin_compute_pass(&Default::default());
                compute.set_pipeline(&pass.pipeline);
                compute.set_bind_group(0, &tiles, &[]);
                compute.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&pass.tile_light_counts, 0, &staging, 0, 16);
            encoder.copy_buffer_to_buffer(&pass.tile_light_lists, 0, &staging, 16, 1024);
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            {
                let mapped = staging.slice(..).get_mapped_range().unwrap();
                let words: &[u32] = bytemuck::cast_slice(&mapped);
                for tile in 0..4 {
                    assert_eq!(words[tile] as usize, expected.len());
                    let start = 4 + tile * 64;
                    let mut actual = words[start..start + expected.len()].to_vec();
                    actual.sort_unstable();
                    assert_eq!(actual, expected);
                }
            }
            staging.unmap();
        }
    });
}
