use super::*;

fn validate(label: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
}

/// The snippets the lights' shaders opt into, as their passes compose them.
const SNIPPETS: &[helio_core::shader::ShaderSnippet] = &[
    helio_mats::PBR_EVAL_SNIPPET,
    helio_frame_api::lighting::sky::ATMOSPHERE_SNIPPET,
    helio_frame_api::lighting::sky::ATMOSPHERE_SKY_TABLE_SNIPPET,
    helio_frame_api::lighting::sun_transmittance::SNIPPET,
    helio_light_api::SUN_OCCLUSION_SNIPPET,
    helio_light_api::LIGHT_LIST_SNIPPET,
    helio_light_api::SHADOW_ATLAS_SNIPPET,
];

/// The lights' shadow shaders as their passes build them. (What casts into
/// the views, the meshes' shadow passes, and what samples them, fog,
/// validate in their own crates.)
#[test]
fn production_shadow_shaders_validate() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    for path in [
        "shadows/shaders/shadow_casters.wgsl",
        "shadows/shaders/shadow_matrices.wgsl",
        "deferred/shaders/deferred_lighting.wgsl",
    ] {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap()
            .replace("__PP_HEAD_VEC4__", "8")
            .replace("__PP_TAIL_VEC4__", "64");
        validate(path, &helio_core::shader::resolve_with(&source, SNIPPETS));
    }
    let prefix = "const USE_RAY_TRANSMISSION:bool=false; const USE_TILE_PRESAMPLING:bool=false; alias Visibility=f32; alias VisibilityCache=vec4f; fn visibility_nonzero(v:f32)->bool{return v>0.0;} fn visibility_missing(v:f32)->bool{return v<0.0;} fn visibility_from_rgb(v:vec3f)->f32{return v.x;}";
    let mut source = prefix.to_string();
    for name in ["common", "lighting", "shadows", "sample"] {
        source.push_str(
            &std::fs::read_to_string(root.join(format!("hlfs/shaders/{name}.wgsl"))).unwrap(),
        );
    }
    validate(
        "HLFS raster fallback",
        &helio_core::shader::resolve_with(&source, SNIPPETS),
    );
}

#[test]
fn configuration_obeys_memory_and_update_budgets() {
    for mb in [1, 4, 16, 40, 160, 640] {
        let b = ShadowBudget {
            memory_bytes: mb * 1024 * 1024,
            ..Default::default()
        }
        .validate()
        .unwrap();
        let size = b.atlas_size(16384);
        assert!(u64::from(size).pow(2) * 10 <= b.memory_bytes);
        assert!(size.is_power_of_two());
    }
    assert!(ShadowBudget {
        updates_per_frame: 1,
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(ShadowBudget {
        max_resolution: 300,
        ..Default::default()
    }
    .validate()
    .is_err());
    assert_eq!(std::mem::size_of::<GpuShadowMatrix>(), 96);
    assert_eq!(std::mem::size_of::<ShadowResident>(), 128);
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pass: ShadowMatrixPass,
    lights: wgpu::Buffer,
    params: CasterParams,
}
impl Gpu {
    fn new(rows: u32, capacity: u32) -> Self {
        let mut descriptor = helio_core::wgpu_instance_descriptor();
        descriptor.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("GPU required");
        // Timestamps where the adapter has them (`run_timed`).
        let timestamps =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            required_features: adapter.features() & timestamps,
            ..Default::default()
        }))
        .unwrap();
        device.on_uncaptured_error(std::sync::Arc::new(|e| panic!("{e:?}")));
        let buf = |size| {
            buffer(
                &device,
                "test",
                size,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            )
        };
        let lights = buf(rows as u64 * 128);
        let matrices = buf(MAX_SHADOW_FACES as u64 * 96);
        let camera = buf(736);
        let pass = ShadowMatrixPass::new(
            &device,
            &lights,
            &matrices,
            &camera,
            &buf(1024),
            &buf(1024),
            2048,
        );
        let identity = glam::Mat4::IDENTITY.to_cols_array();
        let params = CasterParams {
            row_count: rows,
            caster_capacity: capacity,
            nonce: 1,
            atlas_size: 2048,
            view_proj: identity,
            inv_view_proj: identity,
            camera: [0., 0., 0., 100.],
            tuning: [1024., 2048., 0.2, 0.],
        };
        Self {
            device,
            queue,
            pass,
            lights,
            params,
        }
    }
    fn read(&self, src: &wgpu::Buffer) -> Vec<u8> {
        let staging = buffer(
            &self.device,
            "read",
            src.size(),
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(src, 0, &staging, 0, src.size());
        self.queue.submit([e.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.device.poll(wgpu::PollType::Poll).unwrap();
            if let Ok(r) = rx.try_recv() {
                r.unwrap();
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GPU readback timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let bytes = staging.slice(..).get_mapped_range().unwrap().to_vec();
        staging.unmap();
        bytes
    }
    fn run(&self, rows: &[[u32; 32]], active: &ResidencyTable) -> ResidencyTable {
        self.run_timed(rows, active, None)
    }
    /// `run`, its solve's GPU time written to timestamps 0 and 1 of `query`.
    fn run_timed(
        &self,
        rows: &[[u32; 32]],
        active: &ResidencyTable,
        query: Option<&wgpu::QuerySet>,
    ) -> ResidencyTable {
        self.queue
            .write_buffer(&self.lights, 0, bytemuck::cast_slice(rows));
        self.queue
            .write_buffer(&self.pass.committed, 0, bytemuck::bytes_of(active));
        self.queue.write_buffer(
            &self.pass.caster_params_buf,
            0,
            bytemuck::bytes_of(&self.params),
        );
        let mut e = self.device.create_command_encoder(&Default::default());
        if let Some(query) = query {
            e.write_timestamp(query, 0);
        }
        let mut cmds = CommandRecorder::from_encoder(&mut e);
        self.pass
            .dispatch(&mut cmds, 0, self.params.row_count.div_ceil(64));
        self.pass.dispatch(&mut cmds, 1, 1);
        self.pass.dispatch(&mut cmds, 2, 1);
        drop(cmds);
        if let Some(query) = query {
            e.write_timestamp(query, 1);
        }
        self.queue.submit([e.finish()]);
        bytemuck::pod_read_unaligned(&self.read(&self.pass.proposed))
    }
    fn commit(&self, table: &ResidencyTable) -> Vec<u32> {
        self.queue
            .write_buffer(&self.pass.committed, 0, bytemuck::bytes_of(table));
        let mut e = self.device.create_command_encoder(&Default::default());
        self.pass.dispatch(
            &mut CommandRecorder::from_encoder(&mut e),
            3,
            self.params.row_count.div_ceil(64),
        );
        self.queue.submit([e.finish()]);
        bytemuck::cast_slice::<u8, u32>(&self.read(&self.lights)).to_vec()
    }
}
fn light(x: f32, radius: f32, kind: u32) -> [u32; 32] {
    let mut l = [0u32; 32];
    l[0] = x.to_bits();
    l[2] = 0.5f32.to_bits();
    l[3] = radius.to_bits();
    l[5] = (-1f32).to_bits();
    l[11] = 1f32.to_bits();
    l[13] = kind;
    l
}
fn no_overlap(t: &ResidencyTable, extent: u32) {
    let tiles: Vec<_> = t
        .residents
        .iter()
        .filter(|r| r.owner != 0)
        .flat_map(|r| r.tiles)
        .filter(|t| t.size > 0)
        .collect();
    for (i, a) in tiles.iter().enumerate() {
        assert!(a.x + a.size <= extent && a.y + a.size <= extent);
        for b in &tiles[i + 1..] {
            assert!(
                a.x + a.size <= b.x
                    || b.x + b.size <= a.x
                    || a.y + a.size <= b.y
                    || b.y + b.size <= a.y,
                "overlapping tiles: {a:?} {b:?}"
            );
        }
    }
}
#[test]
fn gpu_ranking_tiers_hysteresis_fades_faces_and_fallback() {
    let mut gpu = Gpu::new(600, 64);
    let mut rows = vec![[0u32; 32]; 600];
    // Hundreds of requests, including sparse rows and offscreen high intensity lights.
    for i in 0..500 {
        rows[i] = light(0., 0.01 + (i as f32) * 0.0001, 2);
    }
    rows[599] = light(100., 1., 2);
    rows[599][11] = 1e9f32.to_bits();
    let table = gpu.run(&rows, &ResidencyTable::default());
    assert_eq!(table.header[0], 64);
    assert_eq!(table.residents.iter().filter(|r| r.owner > 0).count(), 64);
    assert!(table
        .residents
        .iter()
        .filter(|r| r.owner > 0)
        .all(|r| r.owner >= 437 && r.owner <= 500));
    no_overlap(&table, 2048);
    let committed = gpu.commit(&table);
    assert_eq!(committed[12], u32::MAX);
    assert_eq!(
        committed[15] & 15,
        15,
        "nonresident preserves shadow request and RT intent"
    );
    assert_eq!(committed[599 * 32 + 12], u32::MAX);
    // Small perturbation cannot evict an incumbent; substantial change fades it first.
    gpu.params.caster_capacity = 1;
    rows.fill([0; 32]);
    rows[0] = light(0., 0.1, 2);
    rows[1] = light(0., 0.105, 2);
    let mut active = ResidencyTable::default();
    active.residents[0] = ShadowResident {
        owner: 1,
        resolution: 128,
        strength: 1.,
        target: 65535,
        tiles: [ShadowTile {
            x: 0,
            y: 0,
            size: 128,
            valid: 0,
        }; 6],
        ..Default::default()
    };
    for f in 1..6 {
        active.residents[0].tiles[f] = Default::default();
    }
    rows[0][15] = 12;
    rows[0][12] = 0;
    let stable = gpu.run(&rows, &active);
    assert_eq!(stable.residents[0].owner, 1);
    assert!(stable.residents[0].target > 0);
    rows[1][3] = 0.3f32.to_bits();
    let fading = gpu.run(&rows, &active);
    assert_eq!(fading.residents[0].owner, 1);
    assert_eq!(fading.residents[0].target, 0);
    assert_eq!(fading.residents[0].strength, 1.);
    active = fading;
    active.residents[0].strength = 0.;
    let replaced = gpu.run(&rows, &active);
    assert_eq!(replaced.residents[0].owner, 2);
    // Quantized coverage tiers and an author cap.
    gpu.params.caster_capacity = 4;
    rows.fill([0; 32]);
    for (i, r) in [0.08, 0.2, 0.4, 0.8].iter().enumerate() {
        rows[i] = light(0., *r, 2);
    }
    rows[3][15] = 8 << 16;
    let tiers = gpu.run(&rows, &ResidencyTable::default());
    for (owner, res) in [(1, 128), (2, 256), (3, 512), (4, 256)] {
        assert_eq!(
            tiers
                .residents
                .iter()
                .find(|r| r.owner == owner)
                .unwrap()
                .resolution,
            res
        );
    }
    no_overlap(&tiers, 2048);
    // Tier hysteresis retains 256 just above its ordinary 256-pixel cutoff.
    let mut active = ResidencyTable::default();
    active.residents[0] = ShadowResident {
        owner: 1,
        resolution: 256,
        strength: 1.,
        ..Default::default()
    };
    rows.fill([0; 32]);
    rows[0] = light(0., 0.26, 2);
    rows[0][15] = 12;
    let tier = gpu.run(&rows, &active);
    assert_eq!(tier.residents[0].resolution, 256);
    assert!(tier.residents[0].target > 0);
    // Outside the receiver frustum, only the cube face pointing back toward it is resident.
    rows.fill([0; 32]);
    rows[0] = light(3., 2.1, 1);
    let faces = gpu.run(&rows, &ResidencyTable::default());
    let r = faces.residents.iter().find(|r| r.owner == 1).unwrap();
    assert!(r.tiles[1].size > 0);
    assert_eq!(r.tiles[0].size, 0);
    assert!(r.tiles.iter().filter(|t| t.size > 0).count() < 6);
    // Explicitly disabling both caster sets yields no map; RT explicit-off stays off.
    rows.fill([0; 32]);
    rows[0] = light(0., 0.5, 2);
    rows[0][15] = 48 | 1;
    let disabled = gpu.run(&rows, &ResidencyTable::default());
    assert_eq!(disabled.header[0], 0);
    let flags = gpu.commit(&disabled);
    assert_eq!(flags[15] & 3, 1);
}

#[test]
fn gpu_tile_sampling_fades_and_honors_author_policy() {
    let gpu = Gpu::new(1, 1);
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow test depth"),
        size: wgpu::Extent3d {
            width: 128,
            height: 128,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let attachment = texture.create_view(&Default::default());
    let sampled = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let mut e = gpu.device.create_command_encoder(&Default::default());
    {
        let _pass = e.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &attachment,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.25),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    gpu.queue.submit([e.finish()]);
    let mut faces = [GpuShadowMatrix {
        light_view_proj: [0.; 16],
        atlas: [0., 0., 1., 1.],
        policy: [0, 0, 128, 2],
    }; 6];
    faces[1].atlas[3] = 0.5;
    faces[2].policy[3] = 1;
    faces[3].policy[1] = 16;
    faces[4].policy[1] = 32;
    faces[5].atlas[2] = 0.;
    let matrices = buffer(
        &gpu.device,
        "sampling matrices",
        6 * 96,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    gpu.queue
        .write_buffer(&matrices, 0, bytemuck::cast_slice(&faces));
    let output = buffer(
        &gpu.device,
        "sampling result",
        6 * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    // The atlas's addressing and sampling, as every reader takes them (over
    // the prelude, whose screen-space helpers the snippet uses).
    let helpers = format!(
        "{}\n{}\n{}",
        helio_core::shader::PRELUDE,
        helio_light_api::SHADOW_ATLAS_SNIPPET.source,
        "fn budget_compare_dynamic(uv:vec2f,layer:u32,depth:f32)->f32 {return budget_compare(shadow_atlas,shadow_sampler,uv,layer,depth,32u);}\n\
         fn budget_compare_static(uv:vec2f,layer:u32,depth:f32)->f32 {return budget_compare(static_shadow_atlas,shadow_sampler,uv,layer,depth,16u);}"
    );
    let source = format!(
        "{}\n{}\n{}",
        r#"
        @group(0) @binding(0) var<storage,read> shadow_matrices:array<LightMatrix>;
        @group(0) @binding(1) var shadow_atlas:texture_depth_2d_array;
        @group(0) @binding(2) var static_shadow_atlas:texture_depth_2d_array;
        @group(0) @binding(3) var shadow_sampler:sampler_comparison;
        @group(0) @binding(4) var<storage,read_write> result:array<vec2f>;
    "#,
        helpers,
        r#"
        @compute @workgroup_size(1) fn main() {
            for(var i=0u;i<6u;i++) {result[i]=vec2f(budget_compare_dynamic(vec2f(0.5),i,0.5),budget_compare_static(vec2f(0.5),i,0.5));}
        }
    "#
    );
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: matrices.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&sampled),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&sampled),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut e = gpu.device.create_command_encoder(&Default::default());
    {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(&pipeline);
        p.set_bind_group(0, &bg, &[]);
        p.dispatch_workgroups(1, 1, 1);
    }
    gpu.queue.submit([e.finish()]);
    let values = gpu.read(&output);
    let values: &[f32] = bytemuck::cast_slice(&values);
    assert_eq!(values, &[0., 0., 0.5, 0.5, 1., 1., 0., 1., 1., 0., 1., 1.]);
}

/// Helio#330 (from #303): with a real camera, equal lights rank by how near
/// they are. The nearest win the budget, resolution never grows with distance,
/// and lights behind the camera get nothing however large they are.
#[test]
fn gpu_ranking_prefers_the_lights_nearest_the_camera() {
    let mut gpu = Gpu::new(320, 64);
    let view = glam::Mat4::look_at_rh(glam::Vec3::ZERO, glam::Vec3::NEG_Z, glam::Vec3::Y);
    let projection = helio_core::depth::perspective(60f32.to_radians(), 1.0, 0.1);
    let view_proj = projection * view;
    gpu.params.view_proj = view_proj.to_cols_array();
    gpu.params.inv_view_proj = view_proj.inverse().to_cols_array();
    let mut rows = vec![[0u32; 32]; 320];
    // Row i sits 2 + i/4 units in front of the camera, on the view axis.
    for (i, row) in rows.iter_mut().take(300).enumerate() {
        *row = light(0., 1.0, 2);
        row[1] = 0f32.to_bits();
        row[2] = (-(2.0 + i as f32 * 0.25)).to_bits();
    }
    // Larger than any of them, but wholly behind the camera: their range
    // ends before the near plane, so nothing they shadow is on screen.
    for row in rows.iter_mut().skip(300) {
        *row = light(0., 4.0, 2);
        row[2] = 6f32.to_bits();
    }
    let table = gpu.run(&rows, &ResidencyTable::default());
    let mut residents: Vec<_> = table.residents.iter().filter(|r| r.owner > 0).collect();
    assert_eq!(residents.len(), 64, "the budget is used in full");
    residents.sort_by_key(|r| r.owner);
    let owners: Vec<u32> = residents.iter().map(|r| r.owner).collect();
    assert_eq!(
        owners,
        (1..=64).collect::<Vec<u32>>(),
        "the 64 nearest lights win"
    );
    for pair in residents.windows(2) {
        assert!(
            pair[0].resolution >= pair[1].resolution,
            "a farther light never gets more resolution: {:?} then {:?}",
            (pair[0].owner, pair[0].resolution),
            (pair[1].owner, pair[1].resolution),
        );
    }
    assert!(
        residents[0].resolution > residents[63].resolution,
        "resolution follows screen coverage"
    );
    no_overlap(&table, 2048);
}

/// A crowded scene facing the camera: thousands of point lights competing
/// for 256 residents and a full atlas (`crowded_lights`).
fn crowded_lights(gpu: &mut Gpu, rows: u32) -> Vec<[u32; 32]> {
    let view = glam::Mat4::look_at_rh(glam::Vec3::ZERO, glam::Vec3::NEG_Z, glam::Vec3::Y);
    let projection = helio_core::depth::perspective(70f32.to_radians(), 16.0 / 9.0, 0.1);
    let view_proj = projection * view;
    gpu.params.view_proj = view_proj.to_cols_array();
    gpu.params.inv_view_proj = view_proj.inverse().to_cols_array();
    let mut lights = vec![[0u32; 32]; rows as usize];
    for (i, row) in lights.iter_mut().enumerate() {
        let (a, b) = ((i % 64) as f32, (i / 64) as f32);
        *row = light(0., 6.0 + (i % 7) as f32, 1);
        row[0] = ((a - 32.0) * 1.5).to_bits();
        row[1] = ((b % 8.0) * 2.0 - 6.0).to_bits();
        row[2] = (-6.0 - b * 0.9).to_bits();
    }
    lights
}

/// Under that pressure the solve stays tile-correct, solve after solve: it
/// runs every few frames while the camera turns.
#[test]
fn gpu_residency_solve_holds_under_pressure() {
    const ROWS: u32 = 4096;
    let mut gpu = Gpu::new(ROWS, 256);
    let rows = crowded_lights(&mut gpu, ROWS);
    let mut table = gpu.run(&rows, &ResidencyTable::default());
    for _ in 0..20 {
        table = gpu.run(&rows, &table);
        assert!(table.residents.iter().filter(|r| r.owner > 0).count() >= 200);
        no_overlap(&table, 2048);
    }
}

/// And cheap: under 2 ms of GPU time (timestamps around its dispatches, the
/// fastest of 20; 0.55 ms on an RTX 3060). A timing, so run on its own on a
/// quiet GPU (`--include-ignored`): other contexts on a shared GPU stretch
/// even GPU timestamps by tens of milliseconds.
#[test]
#[ignore = "timing: run alone on a quiet GPU"]
fn gpu_residency_solve_is_cheap_under_pressure() {
    const ROWS: u32 = 4096;
    let mut gpu = Gpu::new(ROWS, 256);
    let timestamps =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if !gpu.device.features().contains(timestamps) {
        return;
    }
    let rows = crowded_lights(&mut gpu, ROWS);
    let mut table = gpu.run(&rows, &ResidencyTable::default());
    let query = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("residency solve"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let resolve = buffer(
        &gpu.device,
        "solve times",
        16,
        wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
    );
    let mut fastest = f64::INFINITY;
    for _ in 0..20 {
        table = gpu.run_timed(&rows, &table, Some(&query));
        let mut e = gpu.device.create_command_encoder(&Default::default());
        e.resolve_query_set(&query, 0..2, &resolve, 0);
        gpu.queue.submit([e.finish()]);
        let t: [u64; 2] = bytemuck::pod_read_unaligned(&gpu.read(&resolve));
        fastest = fastest.min(
            t[1].saturating_sub(t[0]) as f64 * f64::from(gpu.queue.get_timestamp_period()) / 1e6,
        );
    }
    eprintln!("residency solve: {fastest:.3} ms on the GPU");
    assert!(
        fastest < 2.0,
        "residency solve took {fastest:.3} ms on the GPU"
    );
}

/// A light that turns out of view keeps its map while the pool has room, so
/// turning back does not re-render it and fade it in again.
#[test]
fn gpu_residency_keeps_lights_that_leave_the_view() {
    let mut gpu = Gpu::new(8, 16);
    let projection = helio_core::depth::perspective(60f32.to_radians(), 1.0, 0.1);
    let look = |dir: glam::Vec3| {
        let vp = projection * glam::Mat4::look_at_rh(glam::Vec3::ZERO, dir, glam::Vec3::Y);
        (vp.to_cols_array(), vp.inverse().to_cols_array())
    };
    let (vp, inv) = look(glam::Vec3::NEG_Z);
    gpu.params.view_proj = vp;
    gpu.params.inv_view_proj = inv;
    let mut rows = vec![[0u32; 32]; 8];
    rows[0] = light(0., 3.0, 2);
    rows[0][1] = 0f32.to_bits();
    rows[0][2] = (-8f32).to_bits();
    let table = gpu.run(&rows, &ResidencyTable::default());
    let slot = table
        .residents
        .iter()
        .position(|r| r.owner == 1)
        .expect("visible light is resident");
    // The light is now committed; the camera turns around.
    rows[0][12] = slot as u32 * 6;
    rows[0][15] = 1 | 4 | 8;
    let (vp, inv) = look(glam::Vec3::Z);
    gpu.params.view_proj = vp;
    gpu.params.inv_view_proj = inv;
    let after = gpu.run(&rows, &table);
    assert!(
        after
            .residents
            .iter()
            .any(|r| r.owner == 1 && r.tiles.iter().any(|t| t.size > 0)),
        "a light behind the camera keeps its tiles while capacity is free"
    );
}
