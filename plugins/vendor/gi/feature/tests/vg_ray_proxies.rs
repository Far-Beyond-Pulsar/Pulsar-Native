//! Virtual-geometry instances in the scene TLAS (`SceneDbRayTracing::
//! with_virtual_geometry`): a closed room split in two by a VG wall, a
//! point light in one half. The wall's object carries only a zero-area
//! placeholder mesh (the VG asset's identity, as `vg_stress` places its
//! statues), so without proxies the GI probes' rays and the light's shadow
//! rays pass it and the other half fills with light; through its proxy the
//! far half stays dark until the wall moves away. A wall of an
//! alpha-tested material (a cut-out, classic or VG) stops rays where its
//! alpha is 1 and lets them through where it is 0. Needs hardware ray
//! queries: without them each test returns at once.

use helio_components::LightComponent;
use std::sync::{Arc, RwLock};

use glam::{DVec3, Mat4, Vec3};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_components::ray_tracing::SceneDbRayTracing;
use helio_components::{MaterialComponent, MeshComponent, StaticObjectComponent};
use helio_pass_gi::{Clipmap, GiPass, LEVEL_PROBES, PROBE_WORDS};
use helio_vg_stream::{content_words, VgRuntime, VgeoBytes, VgeoSource};
use pulsar_scenedb::{Entity, World};

fn scene_db(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> pulsar_scenedb::SceneDb {
    let mut scene_db = pulsar_scenedb::SceneDb::new();
    let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
    let mut store = pulsar_scenedb::gpu::SceneGpuStore::new(
        &ctx,
        pulsar_scenedb::gpu::SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    MeshComponent::register_gpu_columns_growable(&mut store, 64, device);
    MaterialComponent::register_gpu_columns_growable(&mut store, 64, device);
    StaticObjectComponent::register_gpu_columns_growable(&mut store, 64, device);
    helio_components::LightComponent::register_gpu_columns_growable(
        &mut store,
        helio_pass_forward_lit::MAX_LIGHTS,
        device,
    );
    let textures = Arc::new(RwLock::new(pulsar_scenedb::gpu::TextureStore::new(64)));
    let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(Arc::new(store), queue.clone())
        .with_texture_store(textures)
        .unwrap();
    scene_db.world.attach_gpu_mirror(mirror);
    scene_db
}

/// The six faces of the box of half extent `half` around the origin, each an
/// `n` x `n` grid of quads, wound outward.
fn box_faces(half: Vec3, n: u32) -> (Vec<Vec3>, Vec<Vec3>, Vec<u32>) {
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let normal =
                Vec3::from_array(std::array::from_fn(|k| if k == axis { sign } else { 0.0 }));
            let u = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 1) % 3)));
            let v = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 2) % 3)));
            let (u, v) = if sign > 0.0 { (u, v) } else { (v, u) };
            let base = positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let (a, b) = (
                        2.0 * i as f32 / n as f32 - 1.0,
                        2.0 * j as f32 / n as f32 - 1.0,
                    );
                    positions.push((normal + u * a + v * b) * half);
                    normals.push(normal);
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let at = |i: u32, j: u32| base + j * (n + 1) + i;
                    indices.extend_from_slice(&[
                        at(i, j),
                        at(i + 1, j),
                        at(i + 1, j + 1),
                        at(i, j),
                        at(i + 1, j + 1),
                        at(i, j + 1),
                    ]);
                }
            }
        }
    }
    (positions, normals, indices)
}

fn material(world: &mut World, colour: [f32; 3], emissive: f32) -> (Entity, MaterialComponent) {
    let entity = world.spawn();
    let row = MaterialComponent::from(helio_pass_gbuffer::MaterialRow::new(
        [colour[0], colour[1], colour[2], 1.0],
        0.8,
        0.0,
        [colour[0], colour[1], colour[2]],
        emissive,
    ));
    world.insert(entity, row);
    (entity, row)
}

/// An object of mesh `mesh` at `transform` in `material`.
fn place(
    world: &mut World,
    mesh: Entity,
    (material, row): (Entity, MaterialComponent),
    transform: Mat4,
    radius: f32,
) -> Entity {
    let store = world.gpu_mirror().cloned().unwrap();
    let vertex_range = MeshComponent::vertices_gpu_handle(store.store(), mesh.index()).unwrap();
    let index_range = MeshComponent::indices_gpu_handle(store.store(), mesh.index()).unwrap();
    let centre = transform.w_axis.truncate();
    let object = StaticObjectComponent::from(helio_pass_gbuffer::StaticObjectRow::new(
        mesh.index(),
        mesh.generation().wrapping_add(1),
        material.index(),
        material.generation().wrapping_add(1),
        transform,
        [centre.x, centre.y, centre.z, radius],
        index_range.count,
        index_range.offset,
        vertex_range.offset as i32,
        row.material_class,
        0,
        helio_pass_object_batch::INSTANCE_FLAG_MOVABLE,
    ));
    let entity = world.spawn();
    world.insert(entity, object);
    entity
}

/// A classic box of half extent `half` at `centre` in `colour`.
fn spawn_box(world: &mut World, centre: Vec3, half: Vec3, colour: [f32; 3]) {
    let (positions, normals, indices) = box_faces(half, 1);
    let vertices = positions
        .iter()
        .zip(&normals)
        .map(|(p, n)| {
            helio_mesh_api::PackedVertex::from_components(
                p.to_array(),
                n.to_array(),
                [0.0; 2],
                [1.0, 0.0, 0.0],
                1.0,
            )
        })
        .collect();
    let mesh = world.spawn();
    world.insert(mesh, MeshComponent { vertices, indices });
    let material = material(world, colour, 0.0);
    place(
        world,
        mesh,
        material,
        Mat4::from_translation(centre),
        half.length(),
    );
}

/// The VG wall: a 10.2 x 4.2 x 0.3 m slab, tessellated so its cluster DAG
/// has levels, cooked to a `.vgeo`.
const WALL_HALF: Vec3 = Vec3::new(5.1, 2.1, 0.15);
const WALL_CONTENT: u128 = 0x5647_5241_5950_524f_5859_5741_4c4c_0001;

fn wall_vgeo() -> Arc<dyn VgeoSource> {
    let (positions, _, indices) = box_faces(WALL_HALF, 48);
    cook(&positions, &indices, WALL_CONTENT)
}

/// `positions` and `indices` cooked to a `.vgeo` of content id `content`
/// (attribute records zero: uv0 0).
fn cook(positions: &[Vec3], indices: &[u32], content: u128) -> Arc<dyn VgeoSource> {
    let positions: Vec<[f32; 3]> = positions.iter().map(|p| p.to_array()).collect();
    let attributes = vec![0u8; positions.len() * helio_vg_format::ATTR_STRIDE as usize];
    let mesh = helio_vg_build::MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: helio_vg_format::ATTR_STRIDE as usize,
        indices,
        tri_material: &[],
    };
    let options = helio_vg_build::BuildOptions {
        content_id: content_words(content),
        resident_levels: 2,
    };
    Arc::new(VgeoBytes::new(helio_vg_build::build_vgeo(&mesh, &options).unwrap()).unwrap())
}

/// A card (a sheet, as cut-outs are): the wall's 10.2 x 4.2 m face as an
/// `n` x `n` grid at z = 0, facing +z.
fn card(n: u32) -> (Vec<Vec3>, Vec<u32>) {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (a, b) = (
                2.0 * i as f32 / n as f32 - 1.0,
                2.0 * j as f32 / n as f32 - 1.0,
            );
            positions.push(Vec3::new(a * WALL_HALF.x, b * WALL_HALF.y, 0.0));
        }
    }
    let mut indices = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let at = |i: u32, j: u32| j * (n + 1) + i;
            indices.extend_from_slice(&[
                at(i, j),
                at(i + 1, j),
                at(i + 1, j + 1),
                at(i, j),
                at(i + 1, j + 1),
                at(i, j + 1),
            ]);
        }
    }
    (positions, indices)
}
const CARD_CONTENT: u128 = 0x5647_5241_5950_524f_5859_4341_5244_0001;

/// The VG wall's object: a zero-area placeholder mesh whose interned vertex
/// range `runtime` knows as the wall's asset.
fn spawn_vg_wall(
    world: &mut World,
    runtime: &VgRuntime,
    colour: [f32; 3],
    emissive: f32,
) -> Entity {
    let material = material(world, colour, emissive);
    spawn_vg(world, runtime, (WALL_CONTENT, wall_vgeo()), material)
}

/// A VG object at the wall's place: asset `content` cooked as `vgeo`, in
/// `material`.
fn spawn_vg(
    world: &mut World,
    runtime: &VgRuntime,
    (content, vgeo): (u128, Arc<dyn VgeoSource>),
    material: (Entity, MaterialComponent),
) -> Entity {
    let mut vertex = helio_mesh_api::PackedVertex::default();
    vertex.tex_coords0 = [7.0, 0.0];
    let mesh = world.spawn();
    world.insert(
        mesh,
        MeshComponent {
            vertices: vec![vertex; 3],
            indices: vec![0, 1, 2],
        },
    );
    let store = world.gpu_mirror().cloned().unwrap();
    let range = MeshComponent::vertices_gpu_handle(store.store(), mesh.index()).unwrap();
    runtime.request_asset(content, range.offset, range.count, vgeo);
    place(
        world,
        mesh,
        material,
        Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        WALL_HALF.length(),
    )
}

/// The room's inside: x and z in (-5, 5), y in (0, 4).
fn room(world: &mut World, light: bool) {
    if light {
        let entity = world.spawn();
        world.insert(
            entity,
            LightComponent::from(helio_pass_forward_lit::LightRow::from(
                helio_pass_forward_lit::GpuLight {
                    position_range: [0.0, 2.5, -2.5, 12.0],
                    color_intensity: [1.0, 0.9, 0.8, 20.0],
                    shadow_index: u32::MAX,
                    light_type: 1,
                    ..Default::default()
                },
            )),
        );
    }
    let white = [0.7; 3];
    spawn_box(
        world,
        Vec3::new(0.0, -0.25, 0.0),
        Vec3::new(5.5, 0.25, 5.5),
        white,
    );
    spawn_box(
        world,
        Vec3::new(0.0, 4.25, 0.0),
        Vec3::new(5.5, 0.25, 5.5),
        white,
    );
    spawn_box(
        world,
        Vec3::new(-5.25, 2.0, 0.0),
        Vec3::new(0.25, 2.5, 5.5),
        white,
    );
    spawn_box(
        world,
        Vec3::new(5.25, 2.0, 0.0),
        Vec3::new(0.25, 2.5, 5.5),
        white,
    );
    spawn_box(
        world,
        Vec3::new(0.0, 2.0, -5.25),
        Vec3::new(5.5, 2.5, 0.25),
        white,
    );
    spawn_box(
        world,
        Vec3::new(0.0, 2.0, 5.25),
        Vec3::new(5.5, 2.5, 0.25),
        white,
    );
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u32> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec()
}

const EYE: Vec3 = Vec3::new(0.3, 2.1, 2.6);
const SPACING: f64 = 0.5;

fn half(h: u32) -> f32 {
    let (e, m) = ((h >> 10) & 31, h & 1023);
    let v = if e == 0 {
        m as f32 * 2f32.powi(-24)
    } else {
        (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15)
    };
    if h & 0x8000 != 0 {
        -v
    } else {
        v
    }
}

/// The finest level's updated, active probes in the box `lo`..`hi`, and
/// their mean light (their DC terms' red plus green). Probe interpolation
/// leaves a residue of order 1e-5 behind any wall; light that gets through
/// is of the order of the lit side's.
fn probe_light(words: &[u32], lo: DVec3, hi: DVec3) -> (usize, f32) {
    let clipmap = Clipmap::around(EYE.as_dvec3(), SPACING);
    let (mut probes, mut sum) = (0, 0.0);
    for index in 0..LEVEL_PROBES {
        let p = clipmap.position(index);
        let at = (index * PROBE_WORDS) as usize;
        let state = words[at + 6];
        if state >> 8 == 0 || state & 1 == 0 || p.cmplt(lo).any() || p.cmpgt(hi).any() {
            continue;
        }
        probes += 1;
        sum += half(words[at] & 0xffff) + half(words[at] >> 16);
    }
    (probes, sum / probes.max(1) as f32)
}

/// The far half's inside, a probe spacing in from its surfaces and the wall.
const DARK_LO: DVec3 = DVec3::new(-4.4, 0.6, 0.6);
const DARK_HI: DVec3 = DVec3::new(4.4, 3.4, 4.4);
/// The near half's.
const LIT_LO: DVec3 = DVec3::new(-4.4, 0.6, -4.4);
const LIT_HI: DVec3 = DVec3::new(4.4, 3.4, -0.6);

struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: pulsar_scenedb::SceneDb,
    renderer: helio::Renderer,
    rays: SceneDbRayTracing,
    runtime: VgRuntime,
    _target: wgpu::Texture,
    view: wgpu::TextureView,
    camera: Camera,
}

impl Harness {
    /// A device with ray queries, `build`'s scene and its renderer; `rays`
    /// gets the scene's VG runtime when `vg`.
    fn new(vg: bool, build: impl FnOnce(&mut World, &VgRuntime)) -> Option<Self> {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        let features = required_wgpu_features(helio::usable_adapter_features(&adapter));
        if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
            eprintln!("no ray queries: skipped");
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            required_limits: required_wgpu_limits(adapter.limits()),
            experimental_features: required_experimental_features(features),
            ..Default::default()
        }))
        .unwrap();
        let (device, queue) = (Arc::new(device), Arc::new(queue));
        let mut scene = scene_db(&device, &queue);
        let runtime = VgRuntime::default();
        build(&mut scene.world, &runtime);
        let config = RendererConfig::new(64, 64, wgpu::TextureFormat::Rgba8Unorm);
        let renderer =
            RendererBuilder::new(config.clone(), scene.world.gpu_mirror().cloned().unwrap())
                .with_graph(helio_presets::Preset::Deferred.with_features(
                &helio_feature_api::FeatureSet::new().with(helio_pass_gi::GiFeature),
            ))
                .with_external_device()
                .build(
                    device.clone(),
                    queue.clone(),
                    config.width,
                    config.height,
                    config.surface_format,
                );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut rays = SceneDbRayTracing::partial(device.clone(), queue.clone());
        if vg {
            rays = rays.with_virtual_geometry(runtime.clone());
        }
        let camera = Camera::perspective_look_at(
            EYE,
            Vec3::new(0.0, 1.5, -1.0),
            Vec3::Y,
            1.0,
            1.0,
            0.1,
            100.0,
        );
        Some(Self {
            device,
            queue,
            scene,
            renderer,
            rays,
            runtime,
            _target: target,
            view,
            camera,
        })
    }

    /// A grey cut-out material (alpha-tested, double-sided, as leaf cards
    /// are) whose base colour texture's alpha is `alpha` everywhere.
    fn cut_out_material(&mut self, alpha: u8) -> (Entity, MaterialComponent) {
        let textures = self
            .scene
            .world
            .gpu_mirror()
            .and_then(|m| m.texture_store())
            .unwrap();
        let texels = [255, 255, 255, alpha].repeat(16);
        let descriptor = wgpu::TextureDescriptor {
            label: Some("cut-out alpha"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let slot = textures
            .write()
            .unwrap()
            .register_mips(&self.device, &self.queue, &descriptor, &[&texels])
            .unwrap();
        let entity = self.scene.world.spawn();
        let mut row = MaterialComponent::from(helio_pass_gbuffer::MaterialRow::new(
            [0.7, 0.7, 0.7, 1.0],
            0.8,
            0.0,
            [0.0; 3],
            0.0,
        ));
        row.flags |= helio_mats::FLAG_ALPHA_TEST | helio_mats::FLAG_DOUBLE_SIDED;
        row.tex_base_color = slot;
        self.scene.world.insert(entity, row);
        (entity, row)
    }

    fn frame(&mut self) {
        self.prepare();
        self.renderer.set_ray_tracing_frame(self.rays.frame());
        self.renderer.render(&self.camera, &self.view).unwrap();
    }

    /// The scene's rows and its ray-tracing projection for the next frame.
    fn prepare(&mut self) {
        self.scene.world.flush_gpu_mirror(&self.queue);
        self.rays.prepare(&self.scene.world).unwrap();
    }

    /// Until the wall's proxy is in the TLAS (built off-thread) and no build
    /// is left. No frame is rendered meanwhile: the probes blend every update
    /// into their history, so frames traced before the proxy arrived (the
    /// wall left out) would leave light behind it for as long as the build
    /// took, which is what made the measurements depend on the build's speed.
    fn wait_for_proxy(&mut self) {
        let start = std::time::Instant::now();
        loop {
            let stats = self.rays.vg_stats();
            if stats.proxies > 0 && stats.pending == 0 {
                break;
            }
            assert!(
                stats.failed == 0 && start.elapsed().as_secs() < 60,
                "the wall's proxy never arrived: {stats:?}"
            );
            self.prepare();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn probes(&self) -> Vec<u32> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        read(
            &self.device,
            &self.queue,
            self.renderer.find_pass::<GiPass>().unwrap().buffers()[1],
        )
    }
}

/// 60 frames of 2720 probe updates: every finest probe several times over.
const FRAMES: usize = 60;

/// The far half's mean probe light after [`FRAMES`] frames of the scene
/// `build` makes.
fn far_light(vg: bool, build: impl FnOnce(&mut World, &VgRuntime)) -> Option<f32> {
    let mut harness = Harness::new(vg, build)?;
    if vg {
        harness.wait_for_proxy();
    }
    for _ in 0..FRAMES {
        harness.frame();
    }
    let (probes, light) = probe_light(&harness.probes(), DARK_LO, DARK_HI);
    assert!(probes > 300, "{probes} probes in the far half");
    Some(light)
}

#[test]
fn a_vg_wall_blocks_probe_and_shadow_rays_through_its_proxy() {
    // Without proxies (the projection before VG instances were traced): the
    // wall is its zero-area placeholder and the light fills both halves.
    let Some(leaky) = far_light(false, |world, runtime| {
        room(world, true);
        spawn_vg_wall(world, runtime, [0.7; 3], 0.0);
    }) else {
        return;
    };
    // The same wall as a classic mesh: what a wall lets through.
    let classic = far_light(false, |world, _| {
        room(world, true);
        spawn_box(world, Vec3::new(0.0, 2.0, 0.0), WALL_HALF, [0.7; 3]);
    })
    .unwrap();
    eprintln!("far half's mean probe light: placeholder {leaky:.5}, classic wall {classic:.6}");
    assert!(
        classic * 100.0 < leaky,
        "a classic wall let {classic} through of {leaky}"
    );

    let mut wall = None;
    let mut sealed = Harness::new(true, |world, runtime| {
        room(world, true);
        wall = Some(spawn_vg_wall(world, runtime, [0.7; 3], 0.0));
    })
    .unwrap();
    sealed.wait_for_proxy();
    let errors = sealed
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    for _ in 0..FRAMES {
        sealed.frame();
    }
    let stats = sealed.rays.vg_stats();
    eprintln!("VG ray proxies: {stats:?}");
    assert_eq!((stats.proxies, stats.pending, stats.failed), (1, 0, 0));
    assert!(
        stats.triangles > 0 && stats.triangles < stats.source_triangles / 4,
        "{stats:?}"
    );
    let words = sealed.probes();
    let (_, near) = probe_light(&words, LIT_LO, LIT_HI);
    let (_, far) = probe_light(&words, DARK_LO, DARK_HI);
    eprintln!("VG wall: near half {near:.5}, far half {far:.6}");
    assert!(
        near * 2.0 > leaky,
        "the near half got {near} (the open room's far half {leaky})"
    );
    assert!(
        far <= classic * 2.0 + 1e-4,
        "the VG wall let {far} through, a classic wall {classic}"
    );

    // The wall moves out of the room (a movable instance): its TLAS
    // instance follows and the light reaches the far half.
    let wall = wall.unwrap();
    let moved = {
        let object = *sealed
            .scene
            .world
            .get::<StaticObjectComponent>(wall)
            .unwrap();
        object.with_transform(
            Mat4::from_translation(Vec3::new(0.0, 50.0, 0.0)),
            [0.0, 50.0, 0.0, WALL_HALF.length()],
        )
    };
    sealed.scene.world.insert(wall, moved);
    for _ in 0..FRAMES {
        sealed.frame();
    }
    let (_, far) = probe_light(&sealed.probes(), DARK_LO, DARK_HI);
    eprintln!("VG wall moved away: far half {far:.5}");
    // Probes blend each update into their history: a quarter of the open
    // room's light after as many frames, tens of thousands of times what
    // the wall let through.
    assert!(
        far * 4.0 > leaky,
        "after the wall moved the far half got {far} of {leaky}"
    );
    let error = pollster::block_on(errors.pop());
    assert!(error.is_none(), "{error:?}");
}

/// A proxy hit is shaded as its object's material: an emissive VG wall in
/// an unlit room lights the probes around it.
#[test]
fn a_vg_proxy_hit_takes_its_object_s_material() {
    let Some(mut harness) = Harness::new(true, |world, runtime| {
        room(world, false);
        spawn_vg_wall(world, runtime, [1.0, 0.5, 0.2], 4.0);
    }) else {
        return;
    };
    harness.wait_for_proxy();
    for _ in 0..FRAMES {
        harness.frame();
    }
    let words = harness.probes();
    for (lo, hi) in [(DARK_LO, DARK_HI), (LIT_LO, LIT_HI)] {
        let (probes, light) = probe_light(&words, lo, hi);
        eprintln!("by the glowing VG wall: {probes} probes, mean light {light:.4}");
        assert!(
            probes > 300 && light > 0.05,
            "{probes} probes by the glowing wall got {light}"
        );
    }
}

/// Cut-outs in the TLAS: the room split by a card (a double-sided sheet, as
/// leaves are) of an alpha-tested material, as a classic mesh and through a
/// VG proxy (its UVs from the asset's attribute records). Where the
/// material's alpha is 1 the card stops probe and shadow rays as an opaque
/// wall does; where it is 0 the far half fills with light as if there were
/// no wall. (A closed solid of a double-sided material has no inside to the
/// probes: those within it stay active and pass light on.)
#[test]
fn a_cut_out_card_stops_rays_where_its_alpha_says() {
    let Some(open) = far_light(false, |world, _| room(world, true)) else {
        return;
    };
    for vg in [false, true] {
        for alpha in [255u8, 0] {
            let mut harness = Harness::new(vg, |world, _| room(world, true)).unwrap();
            let material = harness.cut_out_material(alpha);
            let world = &mut harness.scene.world;
            if vg {
                let (positions, indices) = card(48);
                spawn_vg(
                    world,
                    &harness.runtime,
                    (CARD_CONTENT, cook(&positions, &indices, CARD_CONTENT)),
                    material,
                );
                harness.wait_for_proxy();
            } else {
                let (positions, indices) = card(1);
                let vertices = positions
                    .iter()
                    .map(|p| {
                        helio_mesh_api::PackedVertex::from_components(
                            p.to_array(),
                            [0.0, 0.0, 1.0],
                            [0.0; 2],
                            [1.0, 0.0, 0.0],
                            1.0,
                        )
                    })
                    .collect();
                let mesh = world.spawn();
                world.insert(mesh, MeshComponent { vertices, indices });
                place(
                    world,
                    mesh,
                    material,
                    Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)),
                    WALL_HALF.length(),
                );
            }
            let errors = harness
                .device
                .push_error_scope(wgpu::ErrorFilter::Validation);
            for _ in 0..FRAMES {
                harness.frame();
            }
            let (probes, far) = probe_light(&harness.probes(), DARK_LO, DARK_HI);
            let error = pollster::block_on(errors.pop());
            assert!(error.is_none(), "{error:?}");
            eprintln!("cut-out card (vg {vg}, alpha {alpha}): far half {far:.6} of the open room's {open:.5}");
            assert!(probes > 300, "{probes} probes in the far half");
            if alpha == 255 {
                assert!(
                    far < open * 1e-3,
                    "a solid cut-out card (vg {vg}) let {far} through of {open}"
                );
            } else {
                assert!(
                    far > open * 0.8,
                    "a cut-out card of holes (vg {vg}) let {far} through of {open}"
                );
            }
        }
    }
}

/// A rough scan stand-in: a unit icosphere of `subdiv` subdivisions with
/// its radius modulated by a few octaves of ripples, cooked to a `.vgeo`.
fn bumpy_sphere_vgeo(subdiv: u32, content: u128) -> (Arc<dyn VgeoSource>, usize) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut positions: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut mid = std::collections::HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let mut m = |x: u32, y: u32| {
                *mid.entry((x.min(y), x.max(y))).or_insert_with(|| {
                    positions
                        .push(((positions[x as usize] + positions[y as usize]) * 0.5).normalize());
                    positions.len() as u32 - 1
                })
            };
            let (ab, bc, ca) = (m(a, b), m(b, c), m(c, a));
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    let positions: Vec<[f32; 3]> = positions
        .iter()
        .map(|p| {
            let r = 1.0
                + 0.03 * (p.x * 9.0).sin() * (p.y * 11.0).cos()
                + 0.01 * (p.z * 37.0).sin() * (p.x * 41.0).cos();
            (*p * r).to_array()
        })
        .collect();
    let indices: Vec<u32> = tris.iter().flatten().copied().collect();
    let attributes = vec![0u8; positions.len() * helio_vg_format::ATTR_STRIDE as usize];
    let mesh = helio_vg_build::MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: helio_vg_format::ATTR_STRIDE as usize,
        indices: &indices,
        tri_material: &[],
    };
    let options = helio_vg_build::BuildOptions {
        content_id: content_words(content),
        resident_levels: 2,
    };
    (
        Arc::new(VgeoBytes::new(helio_vg_build::build_vgeo(&mesh, &options).unwrap()).unwrap()),
        tris.len(),
    )
}

/// Numbers for the budgets (run with `--ignored --nocapture`): the GPU
/// allocations a VG asset's proxy adds (its BLAS and its share of the
/// pool), and the time to rebuild the TLAS of thousands of VG instances
/// after one moves (CPU projection plus the GPU build, waited on).
#[test]
#[ignore]
fn measure_proxy_memory_and_tlas_rebuild() {
    const SCAN: u128 = 0x5647_5241_5950_524f_5859_5343_414e_0001;
    const ROWS: i32 = 64;
    let (scan, scan_tris) = bumpy_sphere_vgeo(7, SCAN);
    let mut objects = Vec::new();
    let Some(mut harness) = Harness::new(true, |world, runtime| {
        room(world, true);
        let mut vertex = helio_mesh_api::PackedVertex::default();
        vertex.tex_coords0 = [9.0, 0.0];
        let mesh = world.spawn();
        world.insert(
            mesh,
            MeshComponent {
                vertices: vec![vertex; 3],
                indices: vec![0, 1, 2],
            },
        );
        let store = world.gpu_mirror().cloned().unwrap();
        let range = MeshComponent::vertices_gpu_handle(store.store(), mesh.index()).unwrap();
        runtime.request_asset(SCAN, range.offset, range.count, scan.clone());
        let stone = material(world, [0.6; 3], 0.0);
        for z in 0..ROWS {
            for x in 0..ROWS {
                let at = Mat4::from_translation(Vec3::new(
                    x as f32 * 3.0 - 96.0,
                    1.0,
                    z as f32 * 3.0 + 10.0,
                ));
                objects.push(place(world, mesh, stone, at, 1.1));
            }
        }
    }) else {
        return;
    };
    let report = |device: &wgpu::Device| {
        device
            .generate_allocator_report()
            .map(|r| r.allocations)
            .unwrap_or_default()
    };
    harness.frame();
    harness
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let before = report(&harness.device);
    harness.wait_for_proxy();
    harness
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    while harness.rays.vg_stats().proxies < 1 {
        harness.frame();
    }
    harness.frame();
    harness
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let after = report(&harness.device);
    let stats = harness.rays.vg_stats();
    eprintln!("scan stand-in: {scan_tris} source triangles; proxies: {stats:?}");
    let mut added: std::collections::BTreeMap<String, (u64, u32)> = Default::default();
    let mut seen: std::collections::HashMap<(String, u64, u64), u32> = Default::default();
    for a in &before {
        *seen.entry((a.name.clone(), a.offset, a.size)).or_default() += 1;
    }
    for a in &after {
        match seen.get_mut(&(a.name.clone(), a.offset, a.size)) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                let e = added.entry(a.name.clone()).or_default();
                e.0 += a.size;
                e.1 += 1;
            }
        }
    }
    for (name, (bytes, count)) in &added {
        eprintln!(
            "  allocations added: {name:?} x{count}: {:.1} KiB",
            *bytes as f64 / 1024.0
        );
    }
    // TLAS rebuilds: move one instance a frame.
    let instances = objects.len();
    let mut samples = Vec::new();
    for k in 0..30 {
        let entity = objects[k * 7 % instances];
        let object = *harness
            .scene
            .world
            .get::<StaticObjectComponent>(entity)
            .unwrap();
        let moved = object.transform() * Mat4::from_translation(Vec3::new(0.0, 0.01, 0.0));
        harness
            .scene
            .world
            .insert(entity, object.with_transform(moved, object.bounds));
        harness.scene.world.flush_gpu_mirror(&harness.queue);
        harness
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let start = std::time::Instant::now();
        harness.rays.prepare(&harness.scene.world).unwrap();
        let cpu = start.elapsed();
        harness
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        samples.push((cpu.as_secs_f64() * 1e3, start.elapsed().as_secs_f64() * 1e3));
    }
    samples.sort_by(|a, b| a.1.total_cmp(&b.1));
    let median = samples[samples.len() / 2];
    eprintln!(
        "TLAS of {} instances ({} VG) after one moved: median {:.3} ms CPU, {:.3} ms to GPU completion; min {:.3}, max {:.3}",
        instances + 6,
        instances,
        median.0,
        median.1,
        samples[0].1,
        samples[samples.len() - 1].1
    );
}
