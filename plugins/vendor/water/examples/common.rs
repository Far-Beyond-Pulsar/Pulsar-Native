//! What the water plugin's examples share: the part of Helio's
//! `v3_demo_common` they use, and the preset with the plugin's features.
#![allow(dead_code)]

use glam::{Mat4, Vec3};
use helio::{GpuLight, LightType, Renderer, RendererBuilder, RendererConfig, SceneDbHandle};
#[allow(unused_imports)]
use helio_components::{LightComponent, MaterialComponent};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use helio_pass_sky::host_rows::AtmosphereComponent;
use helio_mesh_api::{MeshUpload, PackedVertex};
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
    helio_pass_sky::host_rows::AtmosphereComponent::register_gpu_columns_growable(&mut gpu_store, 4, device);
    helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, device);
    helio_components::MaterialComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_components::LightComponent::register_gpu_columns_growable(
        &mut gpu_store,
        helio_pass_light_cull::MAX_LIGHTS,
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

/// `HELIO_CAPTURE=<png>`: a windowed example saves its frame number
/// `HELIO_CAPTURE_FRAME` (default 240: probes, shadows and temporal
/// effects settled) and exits. The surface needs [`Self::usage`].
pub struct SurfaceCapture {
    path: Option<String>,
    at: u32,
    frame: u32,
}

impl SurfaceCapture {
    pub fn from_env() -> Self {
        let at = std::env::var("HELIO_CAPTURE_FRAME")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(240);
        Self {
            path: std::env::var("HELIO_CAPTURE").ok(),
            at,
            frame: 0,
        }
    }

    /// The surface's usage: a capture copies from it.
    pub fn usage(&self) -> wgpu::TextureUsages {
        let copy = if self.path.is_some() {
            wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::empty()
        };
        wgpu::TextureUsages::RENDER_ATTACHMENT | copy
    }

    /// Renders and presents a window frame. With a capture asked for, the
    /// frame is rendered into the surface texture itself so [`Self::frame`]
    /// can copy it; the process exits once the frame is saved.
    pub fn render_window_frame(
        &mut self,
        renderer: &mut helio::Renderer,
        camera: &helio::Camera,
        surface: &wgpu::Surface<'_>,
    ) -> helio_core::Result<()> {
        if self.path.is_none() {
            return renderer.render_window_frame(camera, surface);
        }
        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => return Ok(()),
        };
        let result = renderer.render(camera, &output.texture.create_view(&Default::default()));
        let (device, queue) = (renderer.device().clone(), renderer.queue().clone());
        if self.frame(&device, &queue, &output.texture) {
            std::process::exit(0);
        }
        queue.present(output);
        result
    }

    /// After a frame is rendered into `texture`: on the frame asked for,
    /// saves it (RGBA8 or BGRA8) and returns true (the example then exits).
    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
    ) -> bool {
        self.frame += 1;
        let Some(path) = self.path.as_deref().filter(|_| self.frame == self.at) else {
            return false;
        };
        let (width, height) = (texture.width(), texture.height());
        let row = (width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Capture readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
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
        let bgra = matches!(
            texture.format(),
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for p in mapped[(y * row) as usize..][..(width * 4) as usize].chunks_exact(4) {
                pixels.extend_from_slice(&if bgra {
                    [p[2], p[1], p[0], 255]
                } else {
                    [p[0], p[1], p[2], 255]
                });
            }
        }
        image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8).unwrap();
        println!("[capture] frame {} -> {path}", self.frame);
        true
    }
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

pub fn make_material(
    base_color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    emissive_strength: f32,
) -> helio_components::MaterialComponent {
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
pub fn spawn_material(world: &mut World, material: helio_components::MaterialComponent) -> Entity {
    let entity = world.spawn();
    world.insert(entity, material);
    entity
}

pub fn point_light(position: [f32; 3], color: [f32; 3], intensity: f32, range: f32) -> GpuLight {
    GpuLight {
        position_range: [position[0], position[1], position[2], range],
        direction_outer: [0.0, 0.0, -1.0, 0.0],
        color_intensity: [color[0], color[1], color[2], intensity],
        // u32::MAX = "no shadow" (see deferred_lighting.wgsl's sentinel
        // check). NOT 0: SceneDB-authored lights never get a real
        // shadow-atlas slot assigned -- the CPU-side importance-scoring
        // loop that would allocate one predates SceneDB and doesn't run
        // for these (see helio_components::LightComponent's
        // doc comment). shadow_index: 0 silently samples atlas layer 0's
        // stale/unrelated contents as if it belonged to this light, which
        // reads as "fully shadowed" -- an entirely black scene, not merely
        // a light without shadows.
        shadow_index: u32::MAX,
        light_type: LightType::Point as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

/// Insert the scene's atmosphere: Earth's air, its Rayleigh scattering
/// tinted towards `tint` (each channel scaled by `tint` over its largest
/// channel). The sky is scene content too: the renderer only receives the
/// keyed SceneDB buffer, and the scene's first directional light is its sun;
/// without one the sky is black.
pub fn spawn_sky(world: &mut World, tint: [f32; 3]) -> Entity {
    let mut sky = AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth());
    let peak = tint[0].max(tint[1]).max(tint[2]).max(f32::EPSILON);
    for (scattering, channel) in sky.rayleigh_scattering.iter_mut().zip(tint) {
        *scattering *= channel / peak;
    }
    let entity = world.spawn();
    world.insert(entity, sky);
    entity
}

/// CPU-friendly description of a water volume's static appearance, packed
/// into the raw `[f32; 4]` slots `helio_pass_water_sim::host_rows::WaterVolumeComponent`
/// stores GPU-side. Field-to-slot mapping mirrors the `WaterVolume` struct
/// documented in that pass's WGSL shaders (`surface.wgsl`, `caustics.wgsl`,
/// `underwater_fog.wgsl`, `hitbox.frag.wgsl`) -- this is the removed
/// `helio::WaterVolumeDescriptor`/`.to_gpu()` pair reconstructed from the
/// shader-documented layout, since neither survived the SceneDB migration.
///
/// The descriptor leaves the row's `sim_dynamics`/`wind_params` zero, so
/// the heightfield simulation drives these volumes with the pass-wide
/// dynamics set at runtime through
/// `helio_pass_water_sim::WaterSimPass::set_wind`/`set_sim_dynamics`/
/// `set_wave_scale`/`set_wave_speed` (a row with its own spring simulates
/// with its own instead).
#[derive(Clone, Copy, Debug)]
pub struct WaterVolumeDescriptor {
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub surface_height: f32,
    pub wave_amplitude: f32,
    pub wave_frequency: f32,
    pub wave_speed: f32,
    pub wave_direction: [f32; 2],
    pub wave_steepness: f32,
    pub water_color: [f32; 3],
    pub extinction: [f32; 3],
    pub foam_threshold: f32,
    pub foam_amount: f32,
    pub reflection_strength: f32,
    pub refraction_strength: f32,
    pub fresnel_power: f32,
    pub caustics_enabled: bool,
    pub caustics_intensity: f32,
    pub caustics_scale: f32,
    pub caustics_speed: f32,
    pub fog_density: f32,
    pub god_rays_intensity: f32,
    pub ssr_enabled: bool,
    pub ssr_steps: u32,
    pub ssr_step_size: f32,
    pub ssr_thickness: f32,
    /// Index of refraction; 1.333 for water.
    pub ior: f32,
    /// Base (minimum) Fresnel reflectance at normal incidence; water's real
    /// F0 is ~0.02. `surface.wgsl`'s `sim_params.z`.
    pub fresnel_min: f32,
    /// `surface.wgsl`'s `sim_params.w` -- declared there as "density" but not
    /// yet read by any shader in this pass; carried through for forward
    /// compatibility.
    pub density: f32,
    /// Sun direction used by the underwater fog/caustics shading (points
    /// *from* the sun, same convention as every other light direction in
    /// this file). `shader`'s `sun_direction.xyz`.
    pub sun_direction: [f32; 3],
    /// `surface.wgsl`'s `shadow_params.x` ("rim"); not yet read by any
    /// shader in this pass, carried through for forward compatibility.
    pub shadow_rim: f32,
    /// `shadow_params.y`; not yet read by any shader in this pass.
    pub shadow_hitbox: f32,
    /// `shadow_params.z`; not yet read by any shader in this pass.
    pub shadow_ao: f32,
}

impl Default for WaterVolumeDescriptor {
    fn default() -> Self {
        Self {
            bounds_min: [-1.0, -1.0, -1.0],
            bounds_max: [1.0, 1.0, 1.0],
            surface_height: 0.0,
            wave_amplitude: 0.1,
            wave_frequency: 1.0,
            wave_speed: 1.0,
            wave_direction: [1.0, 0.0],
            wave_steepness: 0.3,
            water_color: [0.02, 0.08, 0.12],
            extinction: [0.15, 0.08, 0.04],
            foam_threshold: 0.5,
            foam_amount: 0.5,
            reflection_strength: 0.6,
            refraction_strength: 1.0,
            fresnel_power: 5.0,
            caustics_enabled: false,
            caustics_intensity: 1.0,
            caustics_scale: 4.0,
            caustics_speed: 0.5,
            fog_density: 0.0,
            god_rays_intensity: 0.0,
            ssr_enabled: false,
            ssr_steps: 32,
            ssr_step_size: 0.05,
            ssr_thickness: 0.02,
            ior: 1.333,
            fresnel_min: 0.02,
            density: 0.0,
            sun_direction: [0.0, -1.0, 0.0],
            shadow_rim: 0.0,
            shadow_hitbox: 0.0,
            shadow_ao: 0.0,
        }
    }
}

impl WaterVolumeDescriptor {
    pub fn to_component(&self) -> helio_pass_water_sim::host_rows::WaterVolumeComponent {
        helio_pass_water_sim::host_rows::WaterVolumeComponent {
            bounds_min: [
                self.bounds_min[0],
                self.bounds_min[1],
                self.bounds_min[2],
                0.0,
            ],
            bounds_max: [
                self.bounds_max[0],
                self.bounds_max[1],
                self.bounds_max[2],
                self.surface_height,
            ],
            wave_params: [
                self.wave_amplitude,
                self.wave_frequency,
                self.wave_speed,
                self.wave_steepness,
            ],
            wave_direction: [self.wave_direction[0], self.wave_direction[1], 0.0, 0.0],
            water_color: [
                self.water_color[0],
                self.water_color[1],
                self.water_color[2],
                self.foam_threshold,
            ],
            extinction: [
                self.extinction[0],
                self.extinction[1],
                self.extinction[2],
                self.foam_amount,
            ],
            reflection_refraction: [
                self.reflection_strength,
                self.refraction_strength,
                self.fresnel_power,
                0.0,
            ],
            caustics_params: [
                if self.caustics_enabled { 1.0 } else { 0.0 },
                self.caustics_intensity,
                self.caustics_scale,
                self.caustics_speed,
            ],
            fog_params: [self.fog_density, self.god_rays_intensity, 0.0, 0.0],
            sim_params: [
                self.ior,
                self.caustics_intensity,
                self.fresnel_min,
                self.density,
            ],
            shadow_params: [self.shadow_rim, self.shadow_hitbox, self.shadow_ao, 0.0],
            sun_direction: [
                self.sun_direction[0],
                self.sun_direction[1],
                self.sun_direction[2],
                0.0,
            ],
            ssr_params: [
                if self.ssr_enabled { 1.0 } else { 0.0 },
                self.ssr_steps as f32,
                self.ssr_step_size,
                self.ssr_thickness,
            ],
            sim_dynamics: [0.0; 4],
            wind_params: [0.0; 4],
            _pad6: [0.0; 4],
        }
    }
}

/// Spawn a water volume row from its CPU-side descriptor.
pub fn spawn_water_volume(world: &mut World, descriptor: WaterVolumeDescriptor) -> Entity {
    let entity = world.spawn();
    world.insert(entity, descriptor.to_component());
    entity
}

/// CPU-friendly description of one AABB water-displacement hitbox; packs into
/// `helio_pass_water_sim::host_rows::WaterHitboxComponent` per `hitbox.frag.wgsl`'s
/// `GpuWaterHitbox` layout. Coordinates are world space; the simulation
/// maps the box into each water volume it overlaps.
#[derive(Clone, Copy, Debug)]
pub struct WaterHitboxDescriptor {
    pub old_min: [f32; 3],
    pub old_max: [f32; 3],
    pub new_min: [f32; 3],
    pub new_max: [f32; 3],
    pub edge_softness: f32,
    pub strength: f32,
}

impl WaterHitboxDescriptor {
    pub fn to_component(&self) -> helio_pass_water_sim::host_rows::WaterHitboxComponent {
        helio_pass_water_sim::host_rows::WaterHitboxComponent {
            old_min: [self.old_min[0], self.old_min[1], self.old_min[2], 0.0],
            old_max: [self.old_max[0], self.old_max[1], self.old_max[2], 0.0],
            new_min: [self.new_min[0], self.new_min[1], self.new_min[2], 0.0],
            new_max: [self.new_max[0], self.new_max[1], self.new_max[2], 0.0],
            params: [self.edge_softness, self.strength, 0.0, 0.0],
        }
    }
}

/// Spawn a water hitbox row from its CPU-side descriptor.
pub fn spawn_water_hitbox(world: &mut World, descriptor: WaterHitboxDescriptor) -> Entity {
    let entity = world.spawn();
    world.insert(entity, descriptor.to_component());
    entity
}

/// Replace a previously spawned water hitbox's bounds (e.g. each frame, as
/// the object displacing the water moves).
pub fn update_water_hitbox(world: &mut World, entity: Entity, descriptor: WaterHitboxDescriptor) {
    if let Some(mut existing) =
        world.get_mut::<helio_pass_water_sim::host_rows::WaterHitboxComponent>(entity)
    {
        *existing = descriptor.to_component();
    }
}

/// Spawn the atmosphere shared by the indoor-cathedral demo family: a dim,
/// blue-tinted air. (Its
/// cloud layer went with the procedural sky pass; clouds will return as
/// part of the atmosphere.)
pub fn spawn_indoor_cathedral_sky(world: &mut World) -> Entity {
    spawn_sky(world, [0.05, 0.05, 0.1])
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
        .get::<helio_components::MaterialComponent>(material)
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

pub fn spawn_object_with_movability(
    world: &mut World,
    mesh: Entity,
    material: Entity,
    transform: Mat4,
    radius: f32,
    movability: Option<helio::Movability>,
) -> SceneResult<Entity> {
    let entity = spawn_object(world, mesh, material, transform, radius)?;
    if let Some(movability) = movability {
        set_object_movability(world, entity, movability)?;
    }
    Ok(entity)
}

/// Keep the CPU mobility promise and the GPU shadow partition flag in sync.
pub fn set_object_movability(
    world: &mut World,
    entity: Entity,
    movability: helio::Movability,
) -> SceneResult<()> {
    {
        let Some(mut object) = world.get_mut::<StaticObjectComponent>(entity) else {
            return Err("object entity has no StaticObjectComponent");
        };
        let flag = helio_mesh_api::INSTANCE_FLAG_MOVABLE;
        if movability.can_move() {
            object.flags |= flag;
        } else {
            object.flags &= !flag;
        }
    }
    world.insert(entity, movability);
    Ok(())
}

/// Move a previously spawned object by updating its authoritative SceneDB row.
pub fn update_object_transform(
    world: &mut World,
    _renderer: &mut Renderer,
    entity: Entity,
    transform: Mat4,
) -> SceneResult<()> {
    let Some(mut object) = world.get_mut::<StaticObjectComponent>(entity) else {
        return Err("object entity has no StaticObjectComponent");
    };
    let bounds = [
        transform.w_axis.x,
        transform.w_axis.y,
        transform.w_axis.z,
        object.bounds[3],
    ];
    *object = object.with_transform(transform, bounds).into();
    Ok(())
}

/// Spawn a light. Its `#[gpu]`-mirrored row uploads to the `"scene_lights"`
/// buffer automatically on insert; `helio_pass_forward_lit::ForwardLitPass`
/// resolves that buffer by key every frame on its own -- no renderer method,
/// no per-frame step to call from here at all.
pub fn spawn_light(world: &mut World, light: GpuLight) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LightComponent::from(helio_pass_light_cull::LightRow::from(light)),
    );
    entity
}

/// Replace a previously spawned light's parameters in the World. SceneDB's
/// insert-time GPU mirroring uploads the change automatically; no per-frame
/// resubmission step exists to call anymore.
pub fn update_light(world: &mut World, entity: Entity, light: GpuLight) {
    if let Some(mut existing) = world.get_mut::<LightComponent>(entity) {
        *existing = LightComponent::from(helio_pass_light_cull::LightRow::from(light));
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

pub fn sphere_mesh(center: [f32; 3], radius: f32) -> MeshUpload {
    let center = Vec3::from_array(center);
    let lat_steps = 16;
    let lon_steps = 32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for i in 0..=lat_steps {
        let phi = std::f32::consts::PI * (i as f32 / lat_steps as f32);
        let y = phi.cos();
        let sin_phi = phi.sin();
        for j in 0..=lon_steps {
            let theta = 2.0 * std::f32::consts::PI * (j as f32 / lon_steps as f32);
            let x = sin_phi * theta.cos();
            let z = sin_phi * theta.sin();

            let position = center + Vec3::new(x, y, z) * radius;
            let normal = [x, y, z];
            let uv = [j as f32 / lon_steps as f32, i as f32 / lat_steps as f32];
            let tangent_vec = Vec3::new(-z, 0.0, x).normalize_or_zero();
            let tangent = tangent_vec.to_array();
            vertices.push(PackedVertex::from_components(
                position.to_array(),
                normal,
                uv,
                tangent,
                1.0,
            ));
        }
    }

    for i in 0..lat_steps {
        for j in 0..lon_steps {
            let a = (i * (lon_steps + 1) + j) as u32;
            let b = a + (lon_steps + 1) as u32;
            // CCW winding when viewed from outside (outward normals).
            indices.extend_from_slice(&[a, a + 1, b]);
            indices.extend_from_slice(&[b, a + 1, b + 1]);
        }
    }

    MeshUpload { vertices, indices }
}

// ── The water plugin ─────────────────────────────────────────────────────

/// The plugin's features (Helio's presets do not list them: water is a
/// Pulsar plugin's).
pub fn features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_water_sim::WaterFeature)
        // The lighting plugins' (Helio's presets list neither).
        .with(helio_pass_sky::SkyFeature)
        .with(helio_pass_gi::GiFeature)
}

/// The deferred preset with the plugin's features.
pub fn preset() -> helio_presets::PresetGraph {
    helio_mesh_host::presets::with(helio_presets::Preset::Deferred, &features())
}
