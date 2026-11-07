//! What the fog plugin's examples share: the part of Helio's
//! `v3_demo_common` they use, and the preset with the plugin's features.
#![allow(dead_code)]

use glam::{Mat4, Vec3};
use helio::{Renderer, RendererBuilder, RendererConfig, SceneDbHandle};
use helio_light_api::{GpuLight, LightType};
use helio_light_host::LightComponent;
#[allow(unused_imports)]
use helio_mats::host_rows::MaterialComponent;
use helio_mesh_api::{MeshUpload, PackedVertex};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use helio_pass_sky::host_rows::AtmosphereComponent;
use pulsar_scenedb::{Entity, World};
use std::sync::Arc;

pub type SceneResult<T> = Result<T, &'static str>;

/// Creates a fresh SceneDB `SceneDb` with a GPU mirror already attached, and
/// returns the `SceneDbHandle` to hand to `RendererBuilder::new` -- SceneDB
/// is the sole scene authority, so no renderer in this workspace can be
/// built without one.
///
/// `LightComponent` is `#[gpu(layout = packed)]`, which is DOCUMENTED to
/// auto-register its `"scene_lights"` buffer (at `MAX_LIGHTS` capacity) on
/// the first `World::insert` of one -- but that lazy, reactive path (trigger
/// registration from inside the very dispatch call that's also supposed to
/// write the first row) drops that first write: every demo's lights read
/// back as all-zero forever, even after `World::flush_gpu_mirror` runs every
/// frame. Every other `#[gpu]` type in this file sidesteps the same class of
/// hazard by registering explicitly, up front, before any entity exists to
/// race it -- do the same here instead of relying on the auto-register path.
pub fn new_scene_db_with_gpu_mirror(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
) -> pulsar_scenedb::SceneDb {
    new_scene_db_with_gpu_mirror_and(device, queue, |_| {})
}

/// [`new_scene_db_with_gpu_mirror`] plus extra column registrations made
/// before the mirror is attached (registration needs the store mutably).
pub fn new_scene_db_with_gpu_mirror_and(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    register_extra: impl FnOnce(&mut pulsar_scenedb::gpu::SceneGpuStore),
) -> pulsar_scenedb::SceneDb {
    let mut scene_db = pulsar_scenedb::SceneDb::new();
    let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
    let gpu_cfg = pulsar_scenedb::gpu::SceneGpuConfig {
        classes: Vec::new(),
        tombstone_headroom: 0,
        max_cells_metadata: 0,
    };
    let mut gpu_store = pulsar_scenedb::gpu::SceneGpuStore::new(&ctx, gpu_cfg);
    helio_pass_sky::host_rows::AtmosphereComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4,
        device,
    );
    helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, device);
    helio_mats::host_rows::MaterialComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_light_host::LightComponent::register_gpu_columns_growable(
        &mut gpu_store,
        helio_light_api::MAX_LIGHTS,
        device,
    );
    helio_pass_post_volumes::host_rows::CameraPostProcessComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_pass_volumetric_fog::host_rows::GlobalFogComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_pass_volumetric_fog::host_rows::VolumetricFogSettingsComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    register_extra(&mut gpu_store);
    let gpu_store = Arc::new(gpu_store);
    let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(gpu_store, queue.clone());
    scene_db.world.attach_gpu_mirror(mirror);
    scenedb_inspector_agent::install_world(&mut scene_db.world);
    scene_db
}

/// Frame-boundary SceneDB sync shared by every demo's render loop.
///
/// [`pulsar_scenedb::World::flush_gpu_mirror`] uploads every CPU-side row
/// queued since the last frame (spawns/inserts/updates/despawns) into the
/// GPU-mirrored buffers the renderer actually reads -- without it, CPU-side
/// SceneDB writes are authoritative but invisible to the GPU.
///
/// [`pulsar_scenedb::World::publish_inspector_snapshot`] then feeds any
/// attached `scenedb_inspector` client. It is throttled inside SceneDB and a
/// no-op unless the inspector launched this process, so it is always safe to
/// call. `scenedb_inspector_agent::install_world` only *installs* the bridge;
/// the host must publish after each flush for the inspector to see any data.
pub fn flush_scene_db(
    scene_db: &pulsar_scenedb::SceneDb,
    queue: &wgpu::Queue,
) -> Option<pulsar_scenedb::gpu::SyncStats> {
    let stats = scene_db.world.flush_gpu_mirror(queue);
    scene_db.world.publish_inspector_snapshot();
    stats
}

/// The `SceneDbHandle` (`GpuMirrorHandle`) to pass to `RendererBuilder::new`
/// for a `SceneDb` created via [`new_scene_db_with_gpu_mirror`].
pub fn scene_db_handle(scene_db: &pulsar_scenedb::SceneDb) -> SceneDbHandle {
    scene_db
        .world
        .gpu_mirror()
        .cloned()
        .expect("new_scene_db_with_gpu_mirror always attaches a mirror")
}

/// Build the standard renderer against an already-created SceneDB world.
///
/// The returned renderer receives only the cloneable GPU mirror; callers retain
/// the `SceneDb` and author all scene rows through its `World`.
pub fn build_default_renderer(
    scene_db: &pulsar_scenedb::SceneDb,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: RendererConfig,
) -> Renderer {
    let graph_scene_db = scene_db_handle(scene_db);
    RendererBuilder::new(config.clone(), graph_scene_db.clone())
        .with_external_device()
        .with_graph(preset())
        .build(
            device,
            queue,
            config.width,
            config.height,
            config.surface_format,
        )
}

/// Build the standard renderer on Helio's XR-aware device and attach the
/// OpenXR frame loop when `vr_support` found a runtime.
pub fn build_default_renderer_with_context(
    scene_db: &pulsar_scenedb::SceneDb,
    graphics: &mut helio::HelioGraphicsContext<'_>,
    config: RendererConfig,
) -> Renderer {
    let builder =
        RendererBuilder::new(config.clone(), scene_db_handle(scene_db)).with_graph(preset());
    graphics
        .build_renderer(builder, config)
        .expect("Helio renderer")
        .0
}

pub fn make_material(
    base_color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    emissive_strength: f32,
) -> helio_mats::host_rows::MaterialComponent {
    MaterialComponent::from(helio_mats::MaterialRow::new(
        base_color,
        roughness,
        metallic,
        emissive,
        emissive_strength,
    ))
}

/// Insert a material as a normal SceneDB component row and return its entity
/// index. The index is the value object rows store in `material_slot`.
pub fn spawn_material(
    world: &mut World,
    material: helio_mats::host_rows::MaterialComponent,
) -> Entity {
    let entity = world.spawn();
    world.insert(entity, material);
    entity
}

pub fn directional_light(direction: [f32; 3], color: [f32; 3], intensity: f32) -> GpuLight {
    GpuLight {
        position_range: [0.0, 0.0, 0.0, f32::MAX],
        direction_outer: [direction[0], direction[1], direction[2], 0.0],
        color_intensity: [color[0], color[1], color[2], intensity],
        // u32::MAX = "no shadow" (see deferred_lighting.wgsl's sentinel
        // check). NOT 0: SceneDB-authored lights never get a real
        // shadow-atlas slot assigned -- the CPU-side importance-scoring
        // loop that would allocate one predates SceneDB and doesn't run
        // for these (see helio_light_host::LightComponent's
        // doc comment). shadow_index: 0 silently samples atlas layer 0's
        // stale/unrelated contents as if it belonged to this light, which
        // reads as "fully shadowed" -- an entirely black scene, not merely
        // a light without shadows.
        shadow_index: u32::MAX,
        light_type: LightType::Directional as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

/// Insert a mesh payload into SceneDB's shared geometry pools and return its
/// entity. The returned entity index is the mesh slot used by object rows;
/// the generated GPU handles provide the actual vertex/index offsets.
pub fn spawn_mesh(world: &mut World, upload: MeshUpload) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        MeshComponent {
            vertices: upload.vertices,
            indices: upload.indices,
        },
    );
    entity
}

pub fn spawn_object(
    world: &mut World,
    mesh: Entity,
    material: Entity,
    transform: Mat4,
    radius: f32,
) -> SceneResult<Entity> {
    let mirror = world
        .gpu_mirror()
        .cloned()
        .ok_or("scene has no GPU mirror")?;
    let vertices = MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index())
        .filter(|handle| handle.count != 0)
        .ok_or("mesh has no GPU vertex range")?;
    let indices = MeshComponent::indices_gpu_handle(mirror.store(), mesh.index())
        .filter(|handle| handle.count != 0)
        .ok_or("mesh has no GPU index range")?;
    let material_component = world
        .get::<helio_mats::host_rows::MaterialComponent>(material)
        .ok_or("material entity has no MaterialComponent")?;
    let bounds = [
        transform.w_axis.x,
        transform.w_axis.y,
        transform.w_axis.z,
        radius,
    ];
    // `+ 1`, not the raw SceneDB entity generation: `Entity::generation()`
    // legitimately starts at 0 for any slot's first use (see
    // `World::spawn_inner`) and only increments on despawn-then-reuse, but
    // the object-batch gather shader treats `mesh_generation == 0u` as
    // "this StaticObjectComponent row was never written" (its Zeroable
    // default) and silently skips it -- so every object referencing a
    // freshly-spawned, never-despawned mesh (the overwhelmingly common
    // case: nearly every demo spawns its meshes once and never touches
    // them again) was being dropped from every draw, unconditionally. The
    // `+1` keeps 0 as a true "never written" sentinel while staying
    // trivially recoverable (`stored - 1 == real generation`) if anything
    // ever needs the raw value back.
    let component = StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
        mesh.index(),
        mesh.generation().wrapping_add(1),
        material.index(),
        material.generation().wrapping_add(1),
        transform,
        bounds,
        indices.count,
        indices.offset,
        vertices.offset as i32,
        material_component.material_class,
        0,
        0,
    ));
    let entity = world.spawn();
    world.insert(entity, component);
    Ok(entity)
}

/// Spawn a light. Its `#[gpu]`-mirrored row uploads to the `"scene_lights"`
/// buffer automatically on insert; `helio_pass_forward_lit::ForwardLitPass`
/// resolves that buffer by key every frame on its own -- no renderer method,
/// no per-frame step to call from here at all.
pub fn spawn_light(world: &mut World, light: GpuLight) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LightComponent::from(helio_light_api::LightRow::from(light)),
    );
    entity
}

/// Replace a previously spawned light's parameters in the World. SceneDB's
/// insert-time GPU mirroring uploads the change automatically; no per-frame
/// resubmission step exists to call anymore.
pub fn update_light(world: &mut World, entity: Entity, light: GpuLight) {
    if let Some(mut existing) = world.get_mut::<LightComponent>(entity) {
        *existing = LightComponent::from(helio_light_api::LightRow::from(light));
    }
}

/// Builds a box mesh centred at `center` (mesh-local origin offset) with the
/// given `half_extents`.  The conventional usage is `[0.0, 0.0, 0.0]` with the
/// world position supplied via the transform passed to `insert_object`; the
/// `center` parameter exists for backwards compatibility only.
pub fn box_mesh(center: [f32; 3], half_extents: [f32; 3]) -> MeshUpload {
    let c = Vec3::from_array(center);
    let e = Vec3::from_array(half_extents);
    let corners = [
        c + Vec3::new(-e.x, -e.y, e.z),
        c + Vec3::new(e.x, -e.y, e.z),
        c + Vec3::new(e.x, e.y, e.z),
        c + Vec3::new(-e.x, e.y, e.z),
        c + Vec3::new(-e.x, -e.y, -e.z),
        c + Vec3::new(e.x, -e.y, -e.z),
        c + Vec3::new(e.x, e.y, -e.z),
        c + Vec3::new(-e.x, e.y, -e.z),
    ];
    let faces = [
        ([0, 1, 2, 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([5, 4, 7, 6], [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
        ([4, 0, 3, 7], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([1, 5, 6, 2], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([3, 2, 6, 7], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ([4, 5, 1, 0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (face_index, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (face_index * 4) as u32;
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, corner_index) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[*corner_index].to_array(),
                *normal,
                uv[i],
                *tangent,
                1.0,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshUpload { vertices, indices }
}

pub fn plane_mesh(center: [f32; 3], half_extent: f32) -> MeshUpload {
    let c = Vec3::from_array(center);
    let e = half_extent;
    let normal = [0.0, 1.0, 0.0];
    let tangent = [1.0, 0.0, 0.0];
    let positions = [
        c + Vec3::new(-e, 0.0, -e),
        c + Vec3::new(e, 0.0, -e),
        c + Vec3::new(e, 0.0, e),
        c + Vec3::new(-e, 0.0, e),
    ];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let vertices = positions
        .into_iter()
        .zip(uvs)
        .map(|(position, uv)| {
            PackedVertex::from_components(position.to_array(), normal, uv, tangent, 1.0)
        })
        .collect();
    // Reverse triangle winding so the top-facing plane is front-facing (visible from above).
    let indices = vec![0, 2, 1, 0, 3, 2];
    MeshUpload { vertices, indices }
}

/// Shadow-map base layers for demo lights. The GPU shadow-matrix pass writes a
/// caster's layers at `[base, base + 6)` (a directional light uses 4 cascades,
/// a point light 6 cube faces) and tracks it as caster slot `base / 6`, so
/// bases are multiples of 6. The default 32-layer atlas fits five casters.
pub const SHADOW_BASES: [u32; 5] = [0, 6, 12, 18, 24];

/// Opt a light into the medium with a real shadow map, so it forms shafts
/// wherever geometry blocks it. Scattering gain 1, full volumetric shadow.
pub fn volumetric_light(mut light: GpuLight, shadow_base: u32) -> GpuLight {
    debug_assert!(
        shadow_base % 6 == 0,
        "shadow bases are caster slots of 6 layers"
    );
    light.shadow_index = shadow_base;
    light.god_rays_enabled = 1;
    light.god_rays_density = 1.0;
    light.god_rays_weight = 1.0;
    light.god_rays_exposure = 1.0;
    light.god_rays_decay = 1.0;
    light
}

/// A medium confined to a world AABB, visible from outside it, with an inward
/// edge fade in metres.
pub fn spawn_local_fog(
    world: &mut World,
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    medium: helio_pass_volumetric_fog::host_rows::GlobalFogComponent,
    edge_fade: f32,
) -> Entity {
    let mut volume = helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent::from(
        helio_pass_volumetric_fog::LocalFogVolumeRow::new(bounds_min, bounds_max, medium.row()),
    );
    volume.edge_fade = edge_fade;
    let entity = world.spawn();
    world.insert(entity, volume);
    entity
}

/// Volumetric render settings for every view: quality 0 economical, 1 high,
/// and the integration range in metres (keep it tight around the media).
pub fn set_volumetric_quality(world: &mut World, quality: u32, max_distance: f32) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        helio_pass_volumetric_fog::host_rows::VolumetricFogSettingsComponent {
            quality,
            max_distance,
            light_max_distance: max_distance,
            ..Default::default()
        },
    );
    entity
}

/// A GPU device with the renderer's required features and no window.
pub fn headless_gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("GPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Headless probe"),
        required_features: helio::required_wgpu_features(adapter.features()),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .expect("GPU device");
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
        panic!("[GPU UNCAPTURED ERROR] {e:?}")
    }));
    (Arc::new(device), Arc::new(queue))
}

/// Render `frames` frames of `camera` offscreen (temporal effects settle) and
/// save the last as `path`. Returns the mean sRGB value of the image, 0..255.
pub fn capture_png(
    scene_db: &pulsar_scenedb::SceneDb,
    renderer: &mut Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: &helio::Camera,
    size: (u32, u32),
    frames: u32,
    path: &str,
) -> f64 {
    let (width, height) = size;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Probe target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    for _ in 0..frames {
        flush_scene_db(scene_db, queue);
        renderer.render(camera, &view).expect("probe frame");
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    let row = width * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Probe readback"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let pixels = readback.slice(..).get_mapped_range().unwrap().to_vec();
    image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8).unwrap();
    pixels
        .chunks_exact(4)
        .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0)
        .sum::<f64>()
        / (width * height) as f64
}

// ── The fog plugin ─────────────────────────────────────────────────────

/// The plugin's features (Helio's presets do not list them: fog is a
/// Pulsar plugin's).
pub fn features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_volumetric_fog::FogFeature)
        .with(helio_pass_post_volumes::PostVolumesFeature)
        // The lighting plugins' (Helio's presets list neither).
        .with(helio_pass_sky::SkyFeature)
        .with(helio_pass_gi::GiFeature)
}

/// The deferred preset with the plugin's features.
pub fn preset() -> helio_presets::PresetGraph {
    helio_mesh_host::presets::with(helio_presets::Preset::Deferred, &features())
}
