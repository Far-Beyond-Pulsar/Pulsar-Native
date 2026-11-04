//! Execute production WGSL and the real pass, not a CPU copy of its equations.
use helio_core::{CommandRecorder, GpuCameraUniforms, SceneBufferProjection, SceneInput};
use helio_pass_volumetric_fog::host_rows::{
    GlobalFogComponent, LocalFogVolumeComponent, VolumetricFogSettingsComponent,
};
use helio_pass_volumetric_fog::VolumetricFogPass;
use std::sync::Arc;
use wgpu::util::DeviceExt;

fn gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter required for fog regressions");
        // The fog's inject stage binds nine storage buffers: the adapter's
        // limits, not the defaults (eight).
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        (Arc::new(device), Arc::new(queue))
    })
}
fn buffer(device: &wgpu::Device, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fog test"),
        contents: bytes,
        usage,
    })
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<u8> {
    let result = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: source.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &result, 0, source.size());
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    result
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = result.slice(..).get_mapped_range().unwrap().to_vec();
    result.unmap();
    bytes
}
fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module: shader,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}
fn group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    entries: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
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
}
fn run(cmds: &mut CommandRecorder<'_>, pipeline: &wgpu::ComputePipeline, bg: &wgpu::BindGroup) {
    let mut pass = cmds.begin_compute_pass(&Default::default());
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bg, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}
fn floats(bytes: &[u8]) -> Vec<f32> {
    bytemuck::cast_slice(bytes).to_vec()
}
fn near(a: f32, b: f32, tolerance: f32) {
    assert!(
        (a - b).abs() <= tolerance,
        "{a} != {b} (tolerance {tolerance})"
    );
}

#[test]
fn gpu_numerics_thin_limit_dense_limit_history_and_cube_faces() {
    let (device, queue) = gpu();
    let source = format!(
        "{}\n{}",
        helio_pass_volumetric_fog::shader_source(),
        r#"
@group(0) @binding(20) var<storage, read_write> result: array<vec4<f32>, 32>;
@compute @workgroup_size(1) fn probe_numerics() {
    for (var i = 0u; i < 12u; i++) {
        let sigma = select(pow(10.0, f32(i) - 8.0), 0.0, i == 0u);
        result[i] = vec4<f32>(sigma, segment_integral(sigma, 7.0), exp(-sigma * 7.0), helio_hg_phase(0.0, 0.0));
    }
    result[12] = temporal_result(vec4<f32>(0), vec4<f32>(10,20,30,1), 0.05, 0.25);
    result[13] = temporal_result(vec4<f32>(0,0,0,1), vec4<f32>(10,20,30,1), 0.05, 0.25);
    result[14] = temporal_result(vec4<f32>(2,2,2,1), vec4<f32>(1,1,1,1), 0.05, 0.25);
    result[15] = temporal_result(vec4<f32>(1.01,1.01,1.01,1), vec4<f32>(1,1,1,1), 0.05, 0.25);
    let directions = array<vec3<f32>, 8>(vec3<f32>(1,0,0),vec3<f32>(-1,0,0),vec3<f32>(0,1,0),vec3<f32>(0,-1,0),
        vec3<f32>(0,0,1),vec3<f32>(0,0,-1),vec3<f32>(1,1,1),vec3<f32>(0,-1,-1));
    for (var i = 0u; i < 8u; i++) { result[16u+i] = vec4<f32>(f32(point_light_face(directions[i]))); }
    result[24] = vec4<f32>(ray_box(vec3<f32>(0),vec3<f32>(0,0,-1),vec3<f32>(-1,-1,-12),vec3<f32>(1,1,-8),100.0),0,0);
    result[25] = vec4<f32>(ray_box(vec3<f32>(3,0,0),vec3<f32>(0,0,-1),vec3<f32>(-1,-1,-12),vec3<f32>(1,1,-8),100.0),0,0);
    var height: FogUniforms;
    height.fog_density = 0.1; height.fog_mode = 1u; height.fog_height_falloff = 0.5;
    result[26] = vec4<f32>(global_optical_depth(height,FLAT,vec3<f32>(0,-10,0),vec3<f32>(0,1,0),20.0),
        global_optical_depth(height,FLAT,vec3<f32>(0,10,0),vec3<f32>(0,-1,0),20.0),
        global_optical_depth(height,FLAT,vec3<f32>(0,2,0),vec3<f32>(1,0,0),20.0),0.0);
    // The same profile over a planet, on its side: heights along +X there.
    let planet = vec4<f32>(0.0, -1000.0, 0.0, 1000.0);
    result[28] = vec4<f32>(global_optical_depth(height,planet,vec3<f32>(990,-1000,0),vec3<f32>(1,0,0),20.0),
        global_optical_depth(height,planet,vec3<f32>(1010,-1000,0),vec3<f32>(-1,0,0),20.0),
        global_optical_depth(height,planet,vec3<f32>(1002,-1000,0),vec3<f32>(0,0,1),20.0),
        density_at(vec3<f32>(1002,-1000,0),height,planet));
    height.fog_height_falloff = 0.0;
    result[27] = vec4<f32>(global_optical_depth(height,FLAT,vec3<f32>(0,-10,0),vec3<f32>(0,1,0),20.0));
}
"#
    );
    let shader = helio_core::shader::module(&device, "fog numerical regressions", &source);
    let pipeline = pipeline(&device, &shader, "probe_numerics");
    let output = buffer(
        &device,
        &[0; 512],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let globals = buffer(
        &device,
        &[0; helio_pass_volumetric_fog::FOG_GLOBALS_BYTES],
        wgpu::BufferUsages::UNIFORM,
    );
    let indices = buffer(&device, &[0; 1920], wgpu::BufferUsages::STORAGE);
    let bg = group(
        &device,
        &pipeline,
        &[(2, &globals), (12, &indices), (20, &output)],
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    run(
        &mut CommandRecorder::from_encoder(&mut encoder),
        &pipeline,
        &bg,
    );
    queue.submit([encoder.finish()]);
    let values = floats(&read(&device, &queue, &output));
    for v in values[..48].as_chunks::<4>().0 {
        let sigma = v[0] as f64;
        let expected = if sigma == 0.0 {
            7.0
        } else {
            -(-sigma * 7.0).exp_m1() / sigma
        };
        near(
            v[1],
            expected as f32,
            (expected.abs() as f32 * 2e-5).max(1e-7),
        );
        near(v[3], 1.0 / (4.0 * std::f32::consts::PI), 1e-6);
        assert!(v.iter().all(|x| x.is_finite()));
    }
    assert_eq!(&values[48..52], &[0.0; 4], "vacuum must reject all history");
    assert_eq!(
        &values[52..56],
        &[0.0, 0.0, 0.0, 1.0],
        "lights off must remain black"
    );
    // Lighting changes blend rather than reject: jittered samples differ by
    // design (across shaft edges by 100%), and rejecting them showed raw noise.
    for (value, expected) in values[56..60].iter().zip([1.05, 1.05, 1.05, 1.0]) {
        near(*value, expected, 1e-6);
    }
    near(values[60], 1.0005, 1e-6);
    for (i, face) in [0., 1., 2., 3., 4., 5., 0., 3.].iter().enumerate() {
        assert_eq!(values[64 + i * 4], *face);
    }
    assert_eq!(&values[96..98], &[8.0, 12.0]);
    assert_eq!(&values[100..102], &[0.0, 0.0]);
    near(
        values[104],
        0.1 * (10.0 + 2.0 * (1.0 - (-5f32).exp())),
        1e-6,
    );
    near(values[105], values[104], 1e-6);
    near(values[106], 2.0 * (-1f32).exp(), 1e-6);
    near(values[108], 2.0, 1e-6);
    // Over a planet: altitude above its surface, up away from its centre.
    for i in 0..3 {
        near(values[112 + i], values[104 + i], 1e-5);
    }
    near(values[115], 0.1 * (-1f32).exp(), 1e-6);
}

struct Scene {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera: wgpu::Buffer,
    data: GpuCameraUniforms,
    projection: SceneBufferProjection,
    frame: u64,
}
impl SceneInput for Scene {
    fn world_origin(&self) -> glam::DVec3 {
        glam::DVec3::ZERO
    }
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn camera(&self) -> &wgpu::Buffer {
        &self.camera
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        &self.data
    }
    fn camera_generation(&self) -> u64 {
        0
    }
    fn frame_count(&self) -> u64 {
        self.frame
    }
    fn scene_buffers(&self) -> &SceneBufferProjection {
        &self.projection
    }
}
fn scene(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Scene {
    let projection = helio_core::depth::perspective(1.0, 16.0 / 9.0, 0.1);
    let mut data = GpuCameraUniforms::new(
        glam::Mat4::IDENTITY,
        projection,
        glam::Vec3::ZERO,
        0.1,
        100.0,
        0,
        [0.0; 2],
        projection,
    );
    data.jitter_frame[3] = f32::from_bits(42);
    let camera = buffer(
        &device,
        bytemuck::cast_slice(&[data; 2]),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    Scene {
        device,
        queue,
        camera,
        data,
        projection: SceneBufferProjection::empty(),
        frame: 0,
    }
}
fn world(scene: &Scene) -> (pulsar_scenedb::World, pulsar_scenedb::gpu::GpuMirrorHandle) {
    let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(scene.device.clone(), scene.queue.clone());
    let store = pulsar_scenedb::gpu::SceneGpuStore::new(
        &ctx,
        pulsar_scenedb::gpu::SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(Arc::new(store), scene.queue.clone());
    let mut world = pulsar_scenedb::World::new();
    world.attach_gpu_mirror(mirror.clone());
    (world, mirror)
}

#[test]
fn native_media_world_space_overlap_transmittance_quality_edits_and_tombstones() {
    let (device, queue) = gpu();
    let scene = scene(device.clone(), queue.clone());
    let (mut world, mirror) = world(&scene);
    for _ in 0..140 {
        world.spawn();
    }
    let global = world.spawn();
    let local = world.spawn();
    let settings = world.spawn();
    let global_value = GlobalFogComponent {
        extinction: 0.1,
        ..Default::default()
    };
    let local_value =
        LocalFogVolumeComponent::from(helio_pass_volumetric_fog::LocalFogVolumeRow::new(
            [-2., -2., -12.],
            [2., 2., -8.],
            helio_pass_volumetric_fog::GlobalFogRow {
                extinction: 0.8,
                albedo: [0.25; 3],
                emission: [0.1, 0.2, 0.3],
                ..Default::default()
            },
        ));
    let mut settings_value = VolumetricFogSettingsComponent {
        view_id: 42,
        max_distance: 40.,
        ..Default::default()
    };
    world.insert(global, global_value);
    world.insert(local, local_value);
    world.insert(settings, settings_value);
    let shader = helio_core::shader::module(
        &device,
        "native medium probes",
        &format!(
            "{}\n{}",
            helio_pass_volumetric_fog::shader_source(),
            r#"
@group(0) @binding(20) var<storage, read_write> probes: array<vec4<f32>, 4>;
@compute @workgroup_size(1) fn probe_native() {
    let a = medium_at(vec3<f32>(0,0,-10), 1000000.0);
    let b = medium_at(vec3<f32>(4,0,-10), -1000000.0);
    probes[0] = vec4<f32>(a.albedo, a.extinction);
    probes[1] = vec4<f32>(a.emissive, b.extinction);
    probes[2] = vec4<f32>(medium_transmittance(vec3<f32>(0), vec3<f32>(0,0,-1), 20.0), vec3<f32>(media_list.grid));
    probes[3] = vec4<f32>(fog.fog_max_distance, f32(media_list.history_compatible), f32(media_list.has_medium), f32(media_list.light_samples));
}
"#
        ),
    );
    let resolve = pipeline(&device, &shader, "cs_resolve");
    let classify = pipeline(&device, &shader, "cs_classify");
    let probe = pipeline(&device, &shader, "probe_native");
    let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
    let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
    let fog = buffer(&device, &[0; 64], uniform);
    // FogGlobals: the world origin (hi, lo) and noise phase zero.
    let mut global_bytes = [0u32; helio_pass_volumetric_fog::FOG_GLOBALS_BYTES / 4];
    global_bytes[7] = 0.1f32.to_bits();
    global_bytes[12..16].copy_from_slice(&[192, 108, 128, 1]);
    let globals = buffer(&device, bytemuck::cast_slice(&global_bytes), uniform);
    let lights = buffer(&device, &[0; 128], storage);
    let volumes = buffer(
        &device,
        &vec![0; std::mem::size_of::<helio_pass_postprocess::GpuPostProcessVolume>()],
        storage,
    );
    let legacy = buffer(&device, &[0; 64], storage);
    let indices = buffer(&device, &[0; 1920], storage);
    let indirect = buffer(&device, &[0; 40], storage);
    let resolved = buffer(&device, &[0; 64], storage);
    let output = buffer(&device, &[0; 64], storage);
    let sample = |world: &pulsar_scenedb::World| {
        world.flush_gpu_mirror(&queue);
        let handle = |name| {
            mirror
                .store()
                .resolve_buffer_handle(helio_core::BufferKey::of(name))
                .unwrap()
        };
        let g = handle("global_fog_media");
        let l = handle("local_fog_media");
        let s = handle("volumetric_fog_settings");
        let resolve_bg = group(
            &device,
            &resolve,
            &[
                (0, &scene.camera),
                (1, &fog),
                (2, &globals),
                (11, &volumes),
                (12, &indices),
                (15, &l.buffer),
                (16, &s.buffer),
                (17, &legacy),
                (18, &resolved),
            ],
        );
        let classify_bg = group(
            &device,
            &classify,
            &[
                (1, &fog),
                (2, &globals),
                (3, &lights),
                (11, &volumes),
                (12, &indices),
                (13, &indirect),
                (14, &g.buffer),
                (15, &l.buffer),
            ],
        );
        let probe_bg = group(
            &device,
            &probe,
            &[
                (1, &fog),
                (2, &globals),
                (11, &volumes),
                (12, &indices),
                (14, &g.buffer),
                (15, &l.buffer),
                (20, &output),
            ],
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&fog, 0, None);
        run(
            &mut CommandRecorder::from_encoder(&mut encoder),
            &resolve,
            &resolve_bg,
        );
        encoder.copy_buffer_to_buffer(&resolved, 0, &fog, 0, 64);
        run(
            &mut CommandRecorder::from_encoder(&mut encoder),
            &classify,
            &classify_bg,
        );
        run(
            &mut CommandRecorder::from_encoder(&mut encoder),
            &probe,
            &probe_bg,
        );
        queue.submit([encoder.finish()]);
        (
            floats(&read(&device, &queue, &output)),
            read(&device, &queue, &indirect),
        )
    };
    let (v, dispatch) = sample(&world);
    near(v[3], 0.9, 1e-6);
    near(v[0], 1.0 / 3.0, 1e-6);
    near(v[7], 0.1, 1e-6);
    assert_eq!(&v[4..7], &[0.1, 0.2, 0.3]);
    near(v[8], (-5.2f32).exp(), 1e-6);
    assert_eq!(&v[9..12], &[96., 54., 64.]);
    assert_eq!(
        bytemuck::cast_slice::<u8, u32>(&dispatch),
        // inject, cull, then integrate over the allocated 192x108 columns,
        // then the resolved range (40 m) published for compositing.
        &[12, 7, 64, 12, 7, 16, 24, 14, 1, 40f32.to_bits()]
    );
    assert_eq!(sample(&world).0[13], 1.0, "unchanged history is reusable");
    settings_value.quality = 1;
    settings_value.history_epoch = 5;
    world.insert(settings, settings_value);
    let v = sample(&world).0;
    assert_eq!(&v[9..12], &[192., 108., 128.]);
    assert_eq!(v[13], 0.0);
    assert_eq!(v[15], 24.0);
    world.remove::<LocalFogVolumeComponent>(local);
    let v = sample(&world).0;
    near(v[3], 0.1, 1e-6);
    near(v[8], (-2f32).exp(), 1e-6);
    world.insert(local, local_value);
    near(sample(&world).0[3], 0.9, 1e-6);
    world.despawn(local);
    world.despawn(global);
    let (v, dispatch) = sample(&world);
    assert_eq!(v[3], 0.0);
    assert_eq!(v[8], 1.0);
    assert_eq!(v[14], 0.0);
    assert_eq!(
        bytemuck::cast_slice::<u8, u32>(&dispatch)[0],
        0,
        "empty scenes skip injection"
    );
    // The medium was live on the previous sample, so that one still integrated
    // back to neutral. Now the grid is neutral: nothing integrates and the
    // range published to compositing is zero, so consumers pass through.
    let dispatch = sample(&world).1;
    let dispatch = bytemuck::cast_slice::<u8, u32>(&dispatch);
    assert_eq!(dispatch[6], 0, "a neutral grid needs no integration");
    assert_eq!(
        f32::from_bits(dispatch[9]),
        0.0,
        "a neutral grid publishes no range"
    );
    world.remove::<VolumetricFogSettingsComponent>(settings);
    assert_eq!(
        sample(&world).0[9],
        96.0,
        "removed settings restore the default tier"
    );
    for _ in 0..66 {
        let entity = world.spawn();
        world.insert(
            entity,
            GlobalFogComponent {
                extinction: 0.001,
                ..Default::default()
            },
        );
    }
    let v = sample(&world).0;
    near(v[3], 0.066, 1e-6);
    near(v[8], (-1.32f32).exp(), 1e-6);
}

fn point_light(shadow_strength: f32) -> [u32; 32] {
    let mut words = [0u32; 32];
    words[3] = 20f32.to_bits();
    for value in &mut words[8..12] {
        *value = 1f32.to_bits();
    }
    words[13] = 1;
    words[16] = 1;
    words[17] = 1f32.to_bits();
    words[18] = 1f32.to_bits();
    words[19] = shadow_strength.to_bits();
    words[20] = 1f32.to_bits();
    words
}

#[test]
fn point_shadow_cube_matches_real_matrix_producer_and_shadow_strength_adapter() {
    let (device, queue) = gpu();
    let scene = scene(device.clone(), queue.clone());
    let storage = wgpu::BufferUsages::STORAGE;
    let lights = buffer(
        &device,
        bytemuck::cast_slice(&[point_light(1.), point_light(0.), point_light(0.5)]),
        storage,
    );
    let matrices = buffer(&device, &[0; 576], storage);
    // Use the real shadow producer's math, including its handedness and face order.
    let producer = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("real point shadow matrices"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}",include_str!("../../../lights/feature/shadows/shaders/shadow_matrices.wgsl"),
            "@compute @workgroup_size(1) fn test_point_matrices() { compute_point_light_matrices(0u, vec3f(0), 20.0); }").into()) });
    let producer = pipeline(&device, &producer, "test_point_matrices");
    let producer_bg = group(&device, &producer, &[(0, &lights), (1, &matrices)]);
    let shader = helio_core::shader::module(
        &device,
        "point cube visibility test",
        &format!(
            "{}\n{}",
            helio_pass_volumetric_fog::shader_source(),
            r#"
@group(0) @binding(30) var<storage, read_write> probes: array<vec4<f32>, 6>;
@compute @workgroup_size(1) fn probe_point_shadow() {
    let points = array<vec3<f32>,6>(vec3<f32>(2,0,0),vec3<f32>(-2,0,0),vec3<f32>(0,2,0),vec3<f32>(0,-2,0),vec3<f32>(0,0,2),vec3<f32>(0,0,-2));
    for (var i = 0u; i < 6u; i++) {
        let p = points[i];
        probes[i] = vec4<f32>(shaft_visibility(0u,p).x,inscatter_from_light(0u,p,normalize(p),0.0).x,
            inscatter_from_light(1u,p,normalize(p),0.0).x,inscatter_from_light(2u,p,normalize(p),0.0).x);
    }
}
"#
        ),
    );
    let probe = pipeline(&device, &shader, "probe_point_shadow");
    let atlas = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = atlas.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let fog = buffer(&device, &[0; 64], wgpu::BufferUsages::UNIFORM);
    let globals = buffer(
        &device,
        &[0; helio_pass_volumetric_fog::FOG_GLOBALS_BYTES],
        wgpu::BufferUsages::UNIFORM,
    );
    let indices = buffer(&device, &[0; 1920], storage);
    let volumes = buffer(
        &device,
        &vec![0; std::mem::size_of::<helio_pass_postprocess::GpuPostProcessVolume>()],
        storage,
    );
    let global = buffer(
        &device,
        &[0; std::mem::size_of::<GlobalFogComponent>()],
        storage,
    );
    let local = buffer(
        &device,
        &[0; std::mem::size_of::<LocalFogVolumeComponent>()],
        storage,
    );
    let output = buffer(&device, &[0; 96], storage | wgpu::BufferUsages::COPY_SRC);
    let mut entries: Vec<_> = [
        (0, &scene.camera),
        (1, &fog),
        (2, &globals),
        (3, &lights),
        (4, &matrices),
        (11, &volumes),
        (12, &indices),
        (14, &global),
        (15, &local),
        (30, &output),
    ]
    .iter()
    .map(|(binding, b)| wgpu::BindGroupEntry {
        binding: *binding,
        resource: b.as_entire_binding(),
    })
    .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: 5,
        resource: wgpu::BindingResource::TextureView(&view),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 6,
        resource: wgpu::BindingResource::Sampler(&sampler),
    });
    // The same atlas stands in for the static casters (min of equal maps), and a
    // zeroed transmittance layer means no glass.
    let glass = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let glass_view = glass.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    // The glass lookup loads texels, so the probe binds no sampler for it.
    entries.push(wgpu::BindGroupEntry {
        binding: 20,
        resource: wgpu::BindingResource::TextureView(&view),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 21,
        resource: wgpu::BindingResource::TextureView(&glass_view),
    });
    // No terrain's sun occlusion: an empty atlas, a frame of none, no pages.
    let no_occlusion = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: helio_light_api::SUN_OCCLUSION_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let no_frame = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: helio_light_api::SUN_OCCLUSION_FRAME_BYTES,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let no_pages = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 22,
        resource: wgpu::BindingResource::TextureView(&no_occlusion),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 23,
        resource: no_frame.as_entire_binding(),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 26,
        resource: no_pages.as_entire_binding(),
    });
    // No sun transmittance (no clouds).
    let (no_clouds, no_cloud_frame) =
        helio_frame_api::lighting::sun_transmittance::stand_in(&device);
    entries.push(wgpu::BindGroupEntry {
        binding: 24,
        resource: wgpu::BindingResource::TextureView(&no_clouds),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 25,
        resource: no_cloud_frame.as_entire_binding(),
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &probe.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    run(
        &mut CommandRecorder::from_encoder(&mut encoder),
        &producer,
        &producer_bg,
    );
    for face in 0..6 {
        let face_view = atlas.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: face,
            array_layer_count: Some(1),
            ..Default::default()
        });
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &face_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear((face % 2) as f32),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    run(
        &mut CommandRecorder::from_encoder(&mut encoder),
        &probe,
        &bg,
    );
    queue.submit([encoder.finish()]);
    let values = floats(&read(&device, &queue, &output));
    for (face, v) in values.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(
            v[0],
            (face % 2) as f32,
            "cube face {face} must use its own layer"
        );
        assert!(v[2] > 0.0, "shadow opt-out retains illumination");
        near(v[1], v[2] * v[0], 1e-6);
        near(v[3], v[2] * (0.5 + 0.5 * v[0]), 1e-6);
    }
}

// Raster clears stand in for a blocker covering each cube face. Changing them
// every frame exercises the graphics-producer -> fog-consumer graph dependency.
struct ShadowProducer {
    atlas: wgpu::Texture,
    view: wgpu::TextureView,
    matrices: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bg: wgpu::BindGroup,
    clear_depth: f32,
}
impl ShadowProducer {
    fn new(device: &wgpu::Device, lights: &wgpu::Buffer) -> Self {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fog test current shadows"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = atlas.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let matrices = buffer(device, &[0; 576], wgpu::BufferUsages::STORAGE);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None, source: wgpu::ShaderSource::Wgsl(format!("{}\n{}",
                include_str!("../../../lights/feature/shadows/shaders/shadow_matrices.wgsl"),
                "@compute @workgroup_size(1) fn test_matrices() { compute_point_light_matrices(0u, vec3f(0), 20.0); }").into()),
        });
        let pipeline = pipeline(device, &shader, "test_matrices");
        let bg = group(device, &pipeline, &[(0, lights), (1, &matrices)]);
        Self {
            atlas,
            view,
            matrices,
            pipeline,
            bg,
            clear_depth: 0.0,
        }
    }
}
impl helio_core::RenderPass for ShadowProducer {
    fn name(&self) -> &'static str {
        "FogTestShadowProducer"
    }
    fn writes(&self) -> &'static [&'static str] {
        &[
            helio_light_api::SHADOW_ATLAS,
            helio_light_api::SHADOW_MATRICES,
        ]
    }
    fn publish<'a>(&self, registry: &mut helio_core::ResourceRegistry<'a>) {
        let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(&self.view) };
        let matrices: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.matrices) };
        registry.write_texture_view(
            helio_core::ResourceKey::new(helio_light_api::SHADOW_ATLAS),
            view,
            "FogTestShadowProducer",
        );
        registry.write(
            helio_light_api::shadow_matrices(),
            helio_light_api::ShadowMatricesFrameData {
                desired_matrices: None,
                residency: None,
                budget: Default::default(),
                shadow_matrices: matrices,
                shadow_count: 6,
                per_caster_dirty_gen: [0; helio_light_api::MAX_SHADOW_CASTERS],
                movable_objects_generation: 0,
                caster_layout: None,
            },
            "FogTestShadowProducer",
        );
    }
    fn execute(&mut self, ctx: &mut helio_core::PassContext) -> helio_core::Result<()> {
        let mut cmds = ctx.graphics_cmds();
        run(&mut cmds, &self.pipeline, &self.bg);
        for layer in 0..6 {
            let view = self.atlas.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let _pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_depth),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        Ok(())
    }
}

struct ReadFog {
    pipeline: wgpu::ComputePipeline,
    output: wgpu::Buffer,
}
impl helio_core::RenderPass for ReadFog {
    fn name(&self) -> &'static str {
        "FogTestReadback"
    }
    fn reads(&self) -> &'static [&'static str] {
        &["fog_accum", "fog_parameters"]
    }
    fn execute(&mut self, ctx: &mut helio_core::PassContext) -> helio_core::Result<()> {
        let view = ctx
            .registry
            .get(helio_core::ResourceKey::new("fog_accum"))
            .unwrap();
        let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.output.as_entire_binding(),
                },
            ],
        });
        run(&mut ctx.graphics_cmds(), &self.pipeline, &bg);
        Ok(())
    }
}

#[test]
fn production_graph_native_fog_black_without_lights_emission_and_disable_clear_history() {
    let (device, queue) = gpu();
    let mut scene = scene(device.clone(), queue.clone());
    let (mut world, mirror) = world(&scene);
    let entity = world.spawn();
    let settings = world.spawn();
    let mut medium = GlobalFogComponent {
        extinction: 0.1,
        ..Default::default()
    };
    world.insert(entity, medium);
    world.insert(
        settings,
        VolumetricFogSettingsComponent {
            max_distance: 20.,
            ..Default::default()
        },
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var fog: texture_3d<f32>;
@group(0) @binding(1) var<storage, read_write> result: array<vec4<f32>, 2>;
@compute @workgroup_size(1) fn main() {
    let size = textureDimensions(fog);
    result[0] = textureLoad(fog, vec3<i32>(vec2<i32>(size.xy / 2u), i32(size.z - 1u)), 0);
    result[1] = textureLoad(fog, vec3<i32>(0), 0);
}
"#
            .into(),
        ),
    });
    let output = buffer(
        &device,
        &[0; 32],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let lights = buffer(
        &device,
        &[0; 128 * 270],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let mut graph = helio_core::RenderGraph::new_with_external_device(&device, &queue);
    graph.add_pass(Box::new(ShadowProducer::new(&device, &lights)));
    graph.add_pass(Box::new(VolumetricFogPass::new(&device)));
    graph.add_pass(Box::new(ReadFog {
        pipeline: pipeline(&device, &shader, "main"),
        output: output.clone(),
    }));
    graph.lock(160, 90);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 160,
            height: 90,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    let sample = |world: &pulsar_scenedb::World,
                  scene: &mut Scene,
                  graph: &mut helio_core::RenderGraph| {
        world.flush_gpu_mirror(&queue);
        let handles = mirror
            .store()
            .buffer_registry()
            .keys()
            .into_iter()
            .filter_map(|key| {
                mirror
                    .store()
                    .resolve_buffer_handle(key)
                    .map(|handle| (key, handle))
            });
        scene.projection = SceneBufferProjection::from_handles(handles.chain(std::iter::once((
            helio_core::BufferKey::of(helio_light_api::LIGHT_LIST),
            helio_core::BufferHandle {
                buffer: lights.clone(),
                epoch: 0,
                row_bytes: 128,
                content_generation: scene.frame,
            },
        ))));
        scene.frame += 1;
        graph.execute(scene, &target, &target).unwrap();
        floats(&read(&device, &queue, &output))
    };
    let v = sample(&world, &mut scene, &mut graph);
    assert_eq!(&v[..3], &[0.0; 3], "no arbitrary ambient radiance");
    near(v[3], (-0.1f32 * (20.0 - 0.5)).exp(), 0.001);
    medium.emission = [1., 0.5, 0.25];
    world.insert(entity, medium);
    let v = sample(&world, &mut scene, &mut graph);
    near(v[0], (1.0 - (-0.1f32 * 19.5).exp()) / 0.1, 0.04);
    near(v[1], v[0] * 0.5, 0.01);
    medium.extinction = 0.0;
    world.insert(entity, medium);
    near(sample(&world, &mut scene, &mut graph)[0], 19.5, 0.04);
    world.despawn(entity);
    let v = sample(&world, &mut scene, &mut graph);
    assert_eq!(
        &v[..4],
        &[0., 0., 0., 1.],
        "removal must clear even previously luminous fog"
    );
    assert_eq!(&v[4..8], &[0., 0., 0., 1.]);
    let entity = world.spawn();
    medium.emission = [0.; 3];
    medium.extinction = 0.1;
    world.insert(entity, medium);
    let one = point_light(0.);
    queue.write_buffer(&lights, 0, bytemuck::cast_slice(&one));
    scene.frame = 0; // Hold the stochastic sample fixed for the linearity check.
    let one_light = sample(&world, &mut scene, &mut graph)[0];
    assert!(one_light > 0.0);
    // 66 overlapping lights overflow a cluster; 270 also overflow the global
    // compact list. Neither is allowed to truncate contributions.
    for count in [66usize, 270] {
        queue.write_buffer(&lights, 0, bytemuck::cast_slice(&vec![one; count]));
        scene.frame = 0;
        let lit = sample(&world, &mut scene, &mut graph)[0];
        near(
            lit,
            one_light * count as f32,
            one_light * count as f32 * 0.015,
        );
    }
    queue.write_buffer(&lights, 0, &[0; 128 * 270]);
    assert_eq!(
        sample(&world, &mut scene, &mut graph)[0],
        0.0,
        "light removal clears radiance history immediately"
    );
    queue.write_buffer(&lights, 0, bytemuck::cast_slice(&point_light(1.0)));
    assert_eq!(
        sample(&world, &mut scene, &mut graph)[0],
        0.0,
        "current blocker must shadow the fog"
    );
    graph.find_pass_mut::<ShadowProducer>().unwrap().clear_depth = 1.0;
    let unblocked = sample(&world, &mut scene, &mut graph);
    assert!(
        unblocked[0] > 0.0,
        "removing a blocker must light fog this frame: {unblocked:?}"
    );
    graph.find_pass_mut::<ShadowProducer>().unwrap().clear_depth = 0.0;
    assert_eq!(
        sample(&world, &mut scene, &mut graph)[0],
        0.0,
        "new blocker must reject old lit history this frame"
    );
    world.insert(
        settings,
        VolumetricFogSettingsComponent {
            enabled: 0,
            ..Default::default()
        },
    );
    assert_eq!(
        &sample(&world, &mut scene, &mut graph)[..4],
        &[0., 0., 0., 1.],
        "disabled quality settings produce vacuum"
    );
}

/// Uniform fog lit by one off-axis point light is a smooth field: the radiance
/// each depth slice adds along a column must vary smoothly with depth. A dark
/// stripe per slice (quadrature, history or resampling error) shows up as a
/// dip in this series, so the test measures its relative roughness.
#[test]
fn uniform_fog_scattering_is_smooth_across_depth_slices() {
    smooth_case(false);
}

#[test]
fn local_fog_volume_scattering_is_smooth_across_depth_slices() {
    smooth_case(true);
}

fn smooth_case(local: bool) {
    let (device, queue) = gpu();
    let mut scene = scene(device.clone(), queue.clone());
    let (mut world, mirror) = world(&scene);
    let entity = world.spawn();
    let settings = world.spawn();
    if local {
        world.insert(
            entity,
            LocalFogVolumeComponent::from(helio_pass_volumetric_fog::LocalFogVolumeRow::new(
                [-9., -9., -40.],
                [9., 9., -6.],
                helio_pass_volumetric_fog::GlobalFogRow {
                    extinction: 0.15,
                    ..Default::default()
                },
            )),
        );
    } else {
        world.insert(
            entity,
            GlobalFogComponent {
                extinction: 0.02,
                ..Default::default()
            },
        );
    }
    world.insert(
        settings,
        VolumetricFogSettingsComponent {
            max_distance: 100.,
            ..Default::default()
        },
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var fog: texture_3d<f32>;
@group(0) @binding(1) var<storage, read_write> result: array<vec4<f32>, 1024>;
@compute @workgroup_size(1) fn main() {
    let size = textureDimensions(fog);
    for (var c = 0u; c < 4u; c++) {
        let xy = vec2<u32>(size.x * (c + 1u) / 8u, size.y * (c + 2u) / 8u);
        for (var z = 0u; z < min(size.z, 256u); z++) {
            result[c * 256u + z] = textureLoad(fog, vec3<i32>(vec2<i32>(xy), i32(z)), 0);
        }
    }
}
"#
            .into(),
        ),
    });
    let output = buffer(
        &device,
        &[0; 16 * 1024],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let mut light = point_light(0.);
    for (word, value) in light[..3].iter_mut().zip([6.0f32, 3.0, -12.0]) {
        *word = value.to_bits();
    }
    light[3] = 80f32.to_bits();
    light[11] = 2000f32.to_bits();
    let lights = buffer(
        &device,
        bytemuck::cast_slice(&light),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let mut graph = helio_core::RenderGraph::new_with_external_device(&device, &queue);
    graph.add_pass(Box::new(VolumetricFogPass::new(&device)));
    graph.add_pass(Box::new(ReadFog {
        pipeline: pipeline(&device, &shader, "main"),
        output: output.clone(),
    }));
    graph.lock(1280, 720);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    let mut columns = Vec::new();
    for _ in 0..6 {
        world.flush_gpu_mirror(&queue);
        let handles = mirror
            .store()
            .buffer_registry()
            .keys()
            .into_iter()
            .filter_map(|key| {
                mirror
                    .store()
                    .resolve_buffer_handle(key)
                    .map(|handle| (key, handle))
            });
        scene.projection = SceneBufferProjection::from_handles(handles.chain(std::iter::once((
            helio_core::BufferKey::of(helio_light_api::LIGHT_LIST),
            helio_core::BufferHandle {
                buffer: lights.clone(),
                epoch: 0,
                row_bytes: 128,
                content_generation: scene.frame,
            },
        ))));
        scene.frame += 1;
        graph.execute(&mut scene, &target, &target).unwrap();
        columns = floats(&read(&device, &queue, &output));
    }
    let mut worst = 0.0f32;
    for c in 0..4 {
        let col = &columns[c * 256 * 4..(c + 1) * 256 * 4];
        let depth_slices = col.chunks(4).take_while(|t| t[3] > 0.0).count();
        let delta: Vec<f32> = (1..depth_slices)
            .map(|z| col[z * 4] - col[(z - 1) * 4])
            .collect();
        let series: Vec<String> = delta.iter().map(|d| format!("{d:.4}")).collect();
        eprintln!(
            "column {c}: slices {depth_slices}\n  d = [{}]",
            series.join(", ")
        );
        for z in 1..delta.len().saturating_sub(1) {
            let mean = 0.5 * (delta[z - 1] + delta[z + 1]);
            // Below ~4e-3 the increment is a few ulps of the half-float running sum.
            // The volume has hard faces; only judge slices wholly inside it.
            if delta[z - 1] > 4e-3 && delta[z] > 4e-3 && delta[z + 1] > 4e-3 {
                let rough = (delta[z] - mean).abs() / delta[z];
                if rough > worst {
                    worst = rough;
                }
            }
        }
    }
    eprintln!("worst relative roughness {worst}");
    assert!(
        columns[127 * 4] > 1e-3,
        "fog must be lit for the measurement to mean anything"
    );
    assert!(
        worst < 0.2,
        "per-slice scattering is not smooth: worst relative roughness {worst}"
    );
}

/// Renders `poses` (camera positions looking down -Z, previous-frame matrices
/// supplied correctly) and returns the four sampled columns of the last frame.
fn render_columns(poses: &[glam::Vec3], jitter: bool, half: f32) -> Vec<f32> {
    let (device, queue) = gpu();
    let mut scene = scene(device.clone(), queue.clone());
    let (mut world, mirror) = world(&scene);
    let entity = world.spawn();
    let settings = world.spawn();
    world.insert(
        entity,
        LocalFogVolumeComponent::from(helio_pass_volumetric_fog::LocalFogVolumeRow::new(
            [-half, -half, -60.],
            [half, half, -6.],
            helio_pass_volumetric_fog::GlobalFogRow {
                extinction: 0.15,
                ..Default::default()
            },
        )),
    );
    world.insert(
        settings,
        VolumetricFogSettingsComponent {
            max_distance: 100.,
            ..Default::default()
        },
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var fog: texture_3d<f32>;
@group(0) @binding(1) var<storage, read_write> result: array<vec4<f32>, 1024>;
@compute @workgroup_size(1) fn main() {
    let size = textureDimensions(fog);
    for (var c = 0u; c < 4u; c++) {
        let xy = vec2<u32>(size.x * (c + 1u) / 8u, size.y * (c + 2u) / 8u);
        for (var z = 0u; z < min(size.z, 256u); z++) {
            result[c * 256u + z] = textureLoad(fog, vec3<i32>(vec2<i32>(xy), i32(z)), 0);
        }
    }
}
"#
            .into(),
        ),
    });
    let output = buffer(
        &device,
        &[0; 16 * 1024],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let mut light = point_light(0.);
    for (word, value) in light[..3].iter_mut().zip([6.0f32, 3.0, -20.0]) {
        *word = value.to_bits();
    }
    light[3] = 80f32.to_bits();
    light[11] = 2000f32.to_bits();
    let lights = buffer(
        &device,
        bytemuck::cast_slice(&light),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let mut graph = helio_core::RenderGraph::new_with_external_device(&device, &queue);
    graph.add_pass(Box::new(VolumetricFogPass::new(&device)));
    graph.add_pass(Box::new(ReadFog {
        pipeline: pipeline(&device, &shader, "main"),
        output: output.clone(),
    }));
    graph.lock(1280, 720);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    let base_proj = helio_core::depth::perspective(1.0, 16.0 / 9.0, 0.1);
    let jitter_of = |frame: usize| -> [f32; 2] {
        if !jitter {
            return [0.0; 2];
        }
        let j = helio_core::temporal::r1_r2_jitter(frame as u64);
        [j[0] * 2.0 / 1280.0, j[1] * 2.0 / 720.0]
    };
    let jittered = |frame: usize| {
        let j = jitter_of(frame);
        glam::Mat4::from_translation(glam::Vec3::new(j[0], j[1], 0.0)) * base_proj
    };
    let mut previous = jittered(0) * glam::Mat4::from_translation(-poses[0]);
    let mut columns = Vec::new();
    for (frame, pos) in poses.iter().enumerate() {
        let view = glam::Mat4::from_translation(-*pos);
        let proj = jittered(frame + 1);
        scene.data = GpuCameraUniforms::new(
            view,
            proj,
            *pos,
            0.1,
            100.0,
            frame as u32,
            jitter_of(frame + 1),
            previous,
        );
        scene.data.jitter_frame[3] = f32::from_bits(42);
        queue.write_buffer(&scene.camera, 0, bytemuck::cast_slice(&[scene.data; 2]));
        previous = proj * view;
        world.flush_gpu_mirror(&queue);
        let handles = mirror
            .store()
            .buffer_registry()
            .keys()
            .into_iter()
            .filter_map(|key| {
                mirror
                    .store()
                    .resolve_buffer_handle(key)
                    .map(|handle| (key, handle))
            });
        scene.projection = SceneBufferProjection::from_handles(handles.chain(std::iter::once((
            helio_core::BufferKey::of(helio_light_api::LIGHT_LIST),
            helio_core::BufferHandle {
                buffer: lights.clone(),
                epoch: 0,
                row_bytes: 128,
                content_generation: scene.frame,
            },
        ))));
        scene.frame += 1;
        graph.execute(&mut scene, &target, &target).unwrap();
        columns = floats(&read(&device, &queue, &output));
    }
    columns
}

/// A camera dollying through fog must converge to what a static camera at the
/// same pose renders. History is only 5% new per frame, so any reprojection
/// error is locked in as a pattern that slides through the medium.
#[test]
fn moving_camera_fog_matches_static_reference() {
    moving_case(false);
}

#[test]
fn moving_camera_fog_with_tsr_jitter_matches_static_reference() {
    moving_case(true);
}

fn moving_case(jitter: bool) {
    let end = glam::Vec3::new(0.0, 0.0, -14.0);
    let moving: Vec<glam::Vec3> = (0..60)
        .map(|i| glam::Vec3::new(0.0, 0.0, -14.0 * i as f32 / 59.0))
        .collect();
    let a = render_columns(&moving, jitter, 9.0);
    let b = render_columns(&[end; 8], false, 9.0);
    let mut worst = (0.0f32, 0usize, 0usize);
    for c in 0..4 {
        let mut line = Vec::new();
        for z in 0..128 {
            let i = (c * 256 + z) * 4;
            let (x, y) = (a[i], b[i]);
            if y > 0.02 {
                let rel = (x - y) / y;
                line.push(format!("{rel:+.2}"));
                if rel.abs() > worst.0 {
                    worst = (rel.abs(), c, z);
                }
            }
        }
        eprintln!("column {c} relative error per slice: [{}]", line.join(", "));
    }
    eprintln!("worst moving-vs-static error {:?}", worst);
    assert!(
        worst.0 < 0.1,
        "moving fog differs from the static reference: {worst:?}"
    );
}

/// A bounded volume farther from the camera than the configured fog range must
/// still render: the grid range grows to hold it instead of cutting it off.
#[test]
fn bounded_volume_beyond_configured_range_is_not_clipped() {
    // Camera 100 m behind the volume's near face: it spans 106..160 m away,
    // entirely past the 100 m range the settings request.
    let columns = render_columns(&[glam::Vec3::new(0.0, 0.0, 100.0); 8], false, 400.0);
    let last = 3 * 256 + 127;
    let (radiance, transmittance) = (columns[last * 4], columns[last * 4 + 3]);
    eprintln!("far volume: radiance {radiance}, transmittance {transmittance}");
    assert!(
        radiance > 1e-3 && transmittance < 0.9,
        "distant volume was clipped by the fog far plane"
    );
}
