//! Run the real shaders against the SceneDB GPU mirror. The camera is outside
//! the medium and its global fog is disabled: the regression that hid the demo.
use helio_pass_post_volumes::host_rows::PostProcessVolumeComponent;
use helio_pass_postprocess::{
    FogMode, GpuPostProcessUniforms, PostProcessSettings, PostProcessVolumeDescriptor,
};
use std::sync::Arc;
use wgpu::util::DeviceExt;

fn buffer(device: &wgpu::Device, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fog regression"),
        contents: bytes,
        usage,
    })
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &out, 0, size);
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    out.slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = out.slice(..).get_mapped_range().unwrap().to_vec();
    out.unmap();
    bytes
}

#[test]
fn scenedb_local_fog_is_visible_from_outside_and_tracks_edits_and_removal() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter required");
        // The fog's inject stage binds nine storage buffers: the adapter's
        // limits, not the defaults (eight).
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        // Also validate the production injection/integration pipeline layouts.
        let _pass = helio_pass_volumetric_fog::VolumetricFogPass::new(&device);
        let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
        let store = pulsar_scenedb::gpu::SceneGpuStore::new(
            &ctx,
            pulsar_scenedb::gpu::SceneGpuConfig {
                classes: Vec::new(),
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(Arc::new(store), queue.clone());
        let mut world = pulsar_scenedb::World::new();
        world.attach_gpu_mirror(mirror.clone());
        // GPU packed rows use entity indices, not a dense per-component index.
        // A volume added after scene geometry can be well past initial capacity.
        for _ in 0..96 {
            world.spawn();
        }
        let entity = world.spawn();
        let mut descriptor = PostProcessVolumeDescriptor {
            bounds_min: [-2.0, -2.0, -12.0],
            bounds_max: [2.0, 2.0, -8.0],
            blend_radius: 1.0,
            blend_weight: 1.0,
            unbound: false,
            settings: PostProcessSettings {
                fog_enabled: true,
                fog_density: 0.8,
                fog_mode: FogMode::Uniform,
                fog_max_distance: 40.0,
                fog_color: [0.02, 0.03, 0.04],
                ..Default::default()
            },
            ..Default::default()
        };
        world.insert(
            entity,
            PostProcessVolumeComponent::from(helio_pass_post_volumes::PostProcessVolumeRow::from(
                descriptor.to_gpu(),
            )),
        );
        world.flush_gpu_mirror(&queue);
        let camera_data = helio_core::GpuCameraUniforms::new(
            glam::Mat4::IDENTITY,
            helio_core::depth::perspective(1.0, 1.0, 0.1),
            glam::Vec3::ZERO,
            0.1,
            100.0,
            0,
            [0.0; 2],
            glam::Mat4::IDENTITY,
        );
        let camera = buffer(
            &device,
            bytemuck::cast_slice(&[camera_data; 2]),
            wgpu::BufferUsages::STORAGE,
        );
        let base = PostProcessSettings {
            fog_enabled: false,
            ..Default::default()
        }
        .to_gpu();
        let base_buf = buffer(
            &device,
            bytemuck::bytes_of(&base),
            wgpu::BufferUsages::UNIFORM,
        );
        let blended = buffer(
            &device,
            bytemuck::bytes_of(&base),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let fog_buf = buffer(
            &device,
            &[0; 64],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let globals = buffer(
            &device,
            &[0; helio_pass_volumetric_fog::FOG_GLOBALS_BYTES],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        // No CameraPostProcessComponent rows: the resolver uses its baseline.
        let no_camera_rows = buffer(&device, &[0; 608], wgpu::BufferUsages::STORAGE);
        let lights = buffer(&device, &[0; 128], wgpu::BufferUsages::STORAGE);
        let indices = buffer(&device, &[0; 1920], wgpu::BufferUsages::STORAGE);
        let indirect = buffer(&device, &[0; 40], wgpu::BufferUsages::STORAGE);
        let global_media = buffer(
            &device,
            &[0; std::mem::size_of::<helio_pass_volumetric_fog::host_rows::GlobalFogComponent>()],
            wgpu::BufferUsages::STORAGE,
        );
        let local_media = buffer(
            &device,
            &[0;
                std::mem::size_of::<helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent>(
                )],
            wgpu::BufferUsages::STORAGE,
        );
        let output = buffer(
            &device,
            &[0; 64],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let probe_source = format!(
            "{}\n{}",
            helio_pass_volumetric_fog::shader_source(),
            r#"
@group(0) @binding(20) var<storage, read_write> probes: array<vec4<f32>, 4>;
@compute @workgroup_size(1)
fn cs_probe() {
    let points = array<vec3<f32>, 4>(vec3<f32>(0,0,-10), vec3<f32>(4,0,-10), vec3<f32>(0,0,-5), vec3<f32>(1.75,0,-10));
    for (var i = 0u; i < 4u; i++) {
        let m = fog_medium_at(points[i], -points[i].z);
        probes[i] = vec4<f32>(m.albedo, m.extinction);
    }
}"#
        );
        let shader = helio_core::shader::module(&device, "fog probes", &probe_source);
        let blend_shader = helio_pass_post_volumes::volume_blend_shader(&device);
        let pipeline = |shader: &wgpu::ShaderModule, entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let classify = pipeline(&shader, "cs_classify");
        let probe = pipeline(&shader, "cs_probe");
        let blend = pipeline(&blend_shader, "cs_volume_blend");
        let group = |pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries
                    .iter()
                    .map(|(binding, b)| wgpu::BindGroupEntry {
                        binding: *binding,
                        resource: b.as_entire_binding(),
                    })
                    .collect::<Vec<_>>(),
            })
        };
        let sample = |world: &pulsar_scenedb::World| {
            world.flush_gpu_mirror(&queue);
            let volume = mirror
                .store()
                .resolve_buffer_handle(pulsar_scenedb::gpu::BufferKey::of("post_process_volumes"))
                .unwrap();
            let classify_bg = group(
                &classify,
                &[
                    (1, &fog_buf),
                    (2, &globals),
                    (3, &lights),
                    (11, &volume.buffer),
                    (12, &indices),
                    (13, &indirect),
                    (14, &global_media),
                    (15, &local_media),
                ],
            );
            let probe_bg = group(
                &probe,
                &[
                    (1, &fog_buf),
                    (2, &globals),
                    (11, &volume.buffer),
                    (12, &indices),
                    (14, &global_media),
                    (15, &local_media),
                    (20, &output),
                ],
            );
            let blend_bg = group(
                &blend,
                &[
                    (0, &base_buf),
                    (1, &camera),
                    (15, &volume.buffer),
                    (16, &blended),
                    (20, &no_camera_rows),
                ],
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&blend);
                pass.set_bind_group(0, &blend_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
                pass.set_pipeline(&classify);
                pass.set_bind_group(0, &classify_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(
                &blended,
                GpuPostProcessUniforms::FOG_BLOCK_OFFSET,
                &fog_buf,
                0,
                64,
            );
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&probe);
                pass.set_bind_group(0, &probe_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            queue.submit([encoder.finish()]);
            let data = read(&device, &queue, &output, 64);
            let values: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
            let data = read(
                &device,
                &queue,
                &blended,
                std::mem::size_of::<GpuPostProcessUniforms>() as u64,
            );
            let settings: GpuPostProcessUniforms = bytemuck::pod_read_unaligned(&data);
            (values, settings)
        };
        let (samples, settings) = sample(&world);
        assert_eq!(
            settings.fog_enabled, 1,
            "local volume must enable compositing outside its bounds"
        );
        assert_eq!(
            settings.fog_density, 0.0,
            "local medium must not leak into global fog"
        );
        assert_eq!(settings.fog_max_distance, 40.0);
        assert!(
            (samples[3] - 0.8).abs() < 1e-5,
            "SceneDB first insert must reach the shader"
        );
        assert!(
            (samples[0] - 0.02).abs() < 1e-5,
            "GPU fog block offset/row stride mismatch"
        );
        assert_eq!(samples[7], 0.0, "outside bounds must be clear");
        assert_eq!(
            samples[11], 0.0,
            "space in front of the volume must be clear"
        );
        assert!(
            samples[15] > 0.0 && samples[15] < samples[3],
            "soft boundary"
        );
        descriptor.settings.fog_density = 0.3;
        world.insert(
            entity,
            PostProcessVolumeComponent::from(helio_pass_post_volumes::PostProcessVolumeRow::from(
                descriptor.to_gpu(),
            )),
        );
        assert!(
            (sample(&world).0[3] - 0.3).abs() < 1e-5,
            "live density update"
        );
        descriptor.settings.fog_mode = FogMode::Smoke;
        world.insert(
            entity,
            PostProcessVolumeComponent::from(helio_pass_post_volumes::PostProcessVolumeRow::from(
                descriptor.to_gpu(),
            )),
        );
        let first = sample(&world).0;
        queue.write_buffer(&globals, 32, bytemuck::bytes_of(&8.0f32));
        let animated = sample(&world).0;
        assert!(
            (first[3] - animated[3]).abs() > 0.001,
            "smoke must animate in world space"
        );
        world.remove::<PostProcessVolumeComponent>(entity);
        let (removed, settings) = sample(&world);
        assert_eq!(settings.fog_enabled, 0);
        assert_eq!(removed[3], 0.0, "removed rows must stop injecting fog");
        world.insert(
            entity,
            PostProcessVolumeComponent::from(helio_pass_post_volumes::PostProcessVolumeRow::from(
                descriptor.to_gpu(),
            )),
        );
        assert!(
            sample(&world).0[3] > 0.0,
            "reinsertion must restore the volume"
        );
        world.despawn(entity);
        assert_eq!(
            sample(&world).0[3],
            0.0,
            "despawn must clear the GPU row too"
        );
    });
}
