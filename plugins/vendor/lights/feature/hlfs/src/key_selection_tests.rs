use super::*;
use wgpu::util::DeviceExt;

#[test]
fn key_selection_depends_on_active_lights_not_sparse_allocation() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production key selection"),
            source: wgpu::ShaderSource::Wgsl(
                helio_core::shader::resolve_with(
                    &pipelines::shader_source_for_sampler("grid", true),
                    pipelines::SNIPPETS,
                )
                .into_owned()
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("select_key"),
            compilation_options: Default::default(),
            cache: None,
        });
        let compact_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production light compaction"),
            source: wgpu::ShaderSource::Wgsl(
                helio_core::shader::resolve_with(
                    include_str!("../shaders/compact_lights.wgsl"),
                    pipelines::SNIPPETS,
                )
                .into_owned()
                .into(),
            ),
        });
        let compact_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("production light compaction"),
            layout: None,
            module: &compact_shader,
            entry_point: Some("compact"),
            compilation_options: Default::default(),
            cache: None,
        });
        let live_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("production live light list"),
            layout: None,
            module: &compact_shader,
            entry_point: Some("list_live"),
            compilation_options: Default::default(),
            cache: None,
        });
        let live_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let proposals = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: resources::PROPOSAL_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 24,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[],
        });
        let output = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 3,
                resource: proposals.as_entire_binding(),
            }],
        });
        // Active population, allocation length, sun present, dominant point,
        // and a malformed sun that culling must reject.
        for (count, capacity, sun, dominant, invalid_sun) in [
            (2, 2, true, false, false),
            (2, 4096, true, false, false),
            (17, 17, false, false, false),
            (17, 4096, false, false, false),
            (128, 128, false, false, false),
            (128, 4096, false, false, false),
            (128, 128, false, true, false),
            (128, 4096, false, true, false),
            (2, 4096, true, false, true),
            (0, 4096, false, false, false),
        ] {
            for flags in [4, 12] {
                // Also exercise the temporal fingerprint path.
                let mut lights = vec![helio_light_api::GpuLight::zeroed(); capacity];
                let slots: Vec<_> = (0..count)
                    .map(|i| if capacity == count { i } else { 2000 + i * 7 })
                    .collect();
                for (i, &slot) in slots.iter().enumerate() {
                    let light = &mut lights[slot];
                    light.light_type = 1;
                    light.position_range = [1., 1., 2., 10.];
                    light.color_intensity =
                        [1., 1., 1., if dominant && i == 0 { 10000. } else { 160. }];
                    if sun && i == 0 {
                        light.light_type = 0;
                        light.direction_outer = [0., -1., 0., 0.];
                        light.color_intensity = if invalid_sun {
                            [-1., -1., -1., -4.]
                        } else {
                            [1., 1., 1., 4.]
                        };
                    }
                }
                let expected = if (sun && !invalid_sun) || dominant {
                    slots[0] as u32
                } else {
                    u32::MAX
                };
                let mut globals = Globals::zeroed();
                globals.light_count = capacity as u32;
                globals.surface_flags = flags;
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&globals),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let lights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&lights),
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let compact = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("compact light rows for key selection"),
                    size: (capacity * 64) as u64,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                });
                let compact_input = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &compact_pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: lights.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: compact.as_entire_binding(),
                        },
                    ],
                });
                let live = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("live light rows for key selection"),
                    size: ((capacity + 1) * 4) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let live_input = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &live_pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: lights.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: live.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: uniform.as_entire_binding(),
                        },
                    ],
                });
                let input = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: compact.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: live.as_entire_binding(),
                        },
                    ],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&compact_pipeline);
                    pass.set_bind_group(0, &compact_input, &[]);
                    pass.dispatch_workgroups((capacity as u32).div_ceil(256), 1, 1);
                }
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&live_pipeline);
                    pass.set_bind_group(0, &live_input, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &input, &[]);
                    pass.set_bind_group(1, &empty, &[]);
                    pass.set_bind_group(2, &output, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                encoder.copy_buffer_to_buffer(&proposals, 0, &read, 0, 24);
                encoder.copy_buffer_to_buffer(&live, 0, &live_read, 0, 4);
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                let live_tx = tx.clone();
                read.slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                live_read
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| live_tx.send(r).unwrap());
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                rx.recv().unwrap().unwrap();
                rx.recv().unwrap().unwrap();
                let live_count = bytemuck::cast_slice::<u8, u32>(
                    &live_read.slice(..).get_mapped_range().unwrap(),
                )[0];
                live_read.unmap();
                // Pulsar-Native#838: the list holds live rows only, however
                // sparse the allocation.
                assert_eq!(
                    live_count as usize,
                    count - usize::from(sun && invalid_sun),
                    "live list must hold exactly the lit rows: count={count} capacity={capacity}"
                );
                let bytes = read.slice(..).get_mapped_range().unwrap();
                let words = bytemuck::cast_slice::<u8, u32>(&bytes);
                let key = words[5];
                assert_eq!(key,expected,"count={count} capacity={capacity} sun={sun} dominant={dominant} invalid_sun={invalid_sun} flags={flags}");
                let active_count = count - usize::from(sun && invalid_sun);
                assert_eq!(
                    f32::from_bits(words[1]),
                    active_count as f32,
                    "active emitter count must ignore sparse allocation and invalid lights"
                );
                drop(bytes);
                read.unmap();
            }
        }
    });
}
