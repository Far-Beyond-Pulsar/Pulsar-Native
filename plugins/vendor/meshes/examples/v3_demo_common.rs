use glam::{Mat4, Vec3};
use helio::{GpuLight, LightType, Renderer, RendererBuilder, RendererConfig, SceneDbHandle};
use helio_components::{MaterialComponent, ReflectionCaptureComponent};
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

/// The scene's meshes as each frame's ray-tracing input, on a device with
/// ray queries: the TLAS (and its instances' surfaces) the GI probes' mesh
/// tracer traces. A partial projection: what it cannot represent (foliage,
/// masked or blended materials, ...) is left out, so the probes see past it.
/// `HELIO_SCENE_RAYS=off` publishes none (for A/B: no mesh GI).
pub struct SceneRays {
    projection: Option<helio_mesh_host::ray_tracing::SceneDbRayTracing>,
    failed: bool,
}

impl SceneRays {
    pub fn new(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> Self {
        let projection = device
            .features()
            .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
            .then_some(())
            .filter(|_| std::env::var("HELIO_SCENE_RAYS").as_deref() != Ok("off"))
            .map(|_| {
                helio_mesh_host::ray_tracing::SceneDbRayTracing::partial(
                    device.clone(),
                    queue.clone(),
                )
            });
        Self {
            projection,
            failed: false,
        }
    }

    /// Traces `runtime`'s virtual-geometry instances through their assets'
    /// proxies (`SceneDbRayTracing::with_virtual_geometry`);
    /// `HELIO_VG_RAYS=off` leaves them out, for A/B.
    pub fn with_virtual_geometry(mut self, runtime: helio_vg_stream::VgRuntime) -> Self {
        if std::env::var("HELIO_VG_RAYS").as_deref() != Ok("off") {
            self.projection = self.projection.map(|p| p.with_virtual_geometry(runtime));
        }
        self
    }

    /// The virtual-geometry proxies' counters.
    pub fn vg_stats(&self) -> helio_mesh_host::ray_proxies::VgRayStats {
        self.projection
            .as_ref()
            .map(|p| p.vg_stats())
            .unwrap_or_default()
    }

    /// Publishes this frame's to `renderer`: after flushing SceneDB, before
    /// rendering.
    pub fn publish(&mut self, world: &World, renderer: &mut Renderer) {
        let Some(projection) = &mut self.projection else {
            return;
        };
        match projection.prepare(world) {
            Ok(_) => projection.publish(renderer),
            Err(error) => {
                if !std::mem::replace(&mut self.failed, true) {
                    log::warn!("scene rays: {error}");
                }
            }
        }
    }
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
        .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
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
    let builder = RendererBuilder::new(config.clone(), scene_db_handle(scene_db))
        .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred));
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

pub fn directional_light(direction: [f32; 3], color: [f32; 3], intensity: f32) -> GpuLight {
    GpuLight {
        position_range: [0.0, 0.0, 0.0, f32::MAX],
        direction_outer: [direction[0], direction[1], direction[2], 0.0],
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
        light_type: LightType::Directional as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
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

pub fn spot_light(
    position: [f32; 3],
    direction: [f32; 3],
    color: [f32; 3],
    intensity: f32,
    range: f32,
    inner_angle: f32,
    outer_angle: f32,
) -> GpuLight {
    GpuLight {
        position_range: [position[0], position[1], position[2], range],
        direction_outer: [direction[0], direction[1], direction[2], outer_angle.cos()],
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
        light_type: LightType::Spot as u32,
        inner_angle: inner_angle.cos(),
        _pad: 0,
        ..Default::default()
    }
}

// ── SceneDB-owned scene content ─────────────────────────────────────────────
//
// A demo's placed objects and lights are authored as *pass-owned* SceneDB
// components — `helio_mesh_host::StaticObjectComponent` and
// `helio_components::LightComponent` — not as example-local structs or
// entries in a Helio-owned `Vec`/arena. Each is `#[derive(SceneStore)]` +
// `#[gpu(layout = packed, ...)]`: the pass that consumes a kind
// of scene content owns its schema and its generated GPU buffer, and a World
// row is the only authoritative record of "this exists."
//
// The renderer consumes the SceneDB GPU projection directly. Asset handles are
// resolved when the component is authored, while existence and transforms stay
// exclusively in the World row.

pub use helio_components::LightComponent;
pub use helio_mesh_host::{MeshComponent, StaticObjectComponent};

/// Spawn an oriented-box reflection-capture influence volume, replacing the
/// removed `Scene::insert_reflection_capture(ReflectionCaptureDescriptor::
/// boxed(..))` API. `transform`'s translation/rotation places the box (scale
/// is ignored -- `extents` is the box's own authored half-size, in
/// capture-local space, same as the old descriptor); `transition_distance`
/// is how far the capture fades out from each face. `cubemap_index` starts
/// at -1 (no cubemap resident) -- same as before, it's the probe bake that
/// assigns a real layer, so an unbaked capture still contributes nothing.
pub fn spawn_reflection_capture_box(
    world: &mut World,
    transform: Mat4,
    extents: [f32; 3],
    transition_distance: f32,
) -> Entity {
    let position = transform.w_axis.truncate();
    let gpu = helio_pass_deferred_light::GpuReflectionCapture {
        position_radius: [position.x, position.y, position.z, 0.0],
        extents_transition: [extents[0], extents[1], extents[2], transition_distance],
        world_to_local: transform.inverse().to_cols_array_2d(),
        cubemap_index: -1,
        shape: helio_pass_deferred_light::ReflectionCaptureShape::Box as u32,
        mobility: helio_pass_deferred_light::ReflectionCaptureMobility::Static as u32,
        brightness: 1.0,
    };
    let entity = world.spawn();
    world.insert(
        entity,
        ReflectionCaptureComponent::from(helio_pass_deferred_light::ReflectionCaptureRow::from(
            gpu,
        )),
    );
    entity
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

/// Despawn a previously spawned object from the authoritative SceneDB world.
pub fn despawn_object(
    world: &mut World,
    _renderer: &mut Renderer,
    entity: Entity,
) -> SceneResult<()> {
    world.despawn(entity);
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

/// Builds a cube mesh centred at `center` (mesh-local origin offset) with the
/// given `half_extent`.  The conventional usage is `[0.0, 0.0, 0.0]` with the
/// world position supplied via the transform passed to `insert_object`; the
/// `center` parameter exists for backwards compatibility only.
pub fn cube_mesh(center: [f32; 3], half_extent: f32) -> MeshUpload {
    box_mesh(center, [half_extent, half_extent, half_extent])
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

/// Move/relight a previously spawned point light. Superseded
/// `update_point_light(renderer, LightId, ...)` (Helio's own light-arena
/// handle) — lights are SceneDB `World` rows now, keyed by `Entity`, and
/// [`rebuild_lights`] resubmits them wholesale each frame, so there is no
/// per-light renderer handle to update in place.
pub fn update_point_light(
    world: &mut World,
    entity: Entity,
    position: Vec3,
    color: [f32; 3],
    intensity: f32,
    range: f32,
) {
    update_light(
        world,
        entity,
        point_light(position.to_array(), color, intensity, range),
    );
}

/// Publish the host's post settings: the tone map's baseline
/// (`helio_pass_postprocess::POST_BASELINE`). It persists until replaced, so
/// interactive examples call this again after editing their controls.
pub fn set_post_baseline(
    renderer: &mut Renderer,
    settings: &helio_pass_postprocess::PostProcessSettings,
) {
    renderer.set_scene_payload(
        &helio_pass_postprocess::POST_BASELINE,
        Arc::new(settings.clone()),
    );
}

// ── Light shafts ─────────────────────────────────────────────────────────────
//
// Shafts are not a separate effect: they are the shadowed part of lit medium,
// so a light only forms them when it (a) participates in the medium and (b) has
// a shadow map to be occluded by. The media themselves (fog) left Helio with
// their plugin (Pulsar `plugins/vendor/fog`).

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

// ── Headless capture (`--probe` modes) ──────────────────────────────────────

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
