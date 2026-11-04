//! What the portals plugin's examples share: the part of Helio's
//! `v3_demo_common` they use, the plugin's rows as standalone hosts
//! author them, and the preset with the plugin's features.
#![allow(dead_code)]

use glam::{Mat4, Vec3};
use helio::{GpuLight, LightType, SceneDbHandle};
use helio_components::MaterialComponent;
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
    helio_pass_sky::host_rows::AtmosphereComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4,
        device,
    );
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
    PortalComponent::register_gpu_columns_growable(&mut gpu_store, 1024, device);
    PortalViewComponent::register_gpu_columns_growable(&mut gpu_store, 1024, device);
    PortalChainComponent::register_gpu_columns_growable(&mut gpu_store, 1024, device);
    PortalProjectionCountsComponent::register_gpu_columns_growable(&mut gpu_store, 1, device);
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

// ── SceneDB-owned scene content ─────────────────────────────────────────────
//
// A demo's placed objects and lights are authored as *pass-owned* SceneDB
// components — `helio_mesh_host::StaticObjectComponent` and
// `helio_components::LightComponent` — not as example-local structs or
// entries in a Helio-owned `Vec`/arena. Each is `#[derive(SceneStore)]` +
// `#[gpu(layout = packed, ...)]`, exactly like the already-shipped
// `CoronaEmitterComponent`: the pass that consumes a kind
// of scene content owns its schema and its generated GPU buffer, and a World
// row is the only authoritative record of "this exists."
//
// The renderer consumes the SceneDB GPU projection directly. Asset handles are
// resolved when the component is authored, while existence and transforms stay
// exclusively in the World row.

pub use helio_components::LightComponent;
pub use helio_mesh_host::{MeshComponent, StaticObjectComponent};

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

// ── Participating media and light shafts (component API) ─────────────────────
//
// Fog is world data: global and local media are SceneDB components owned by
// the volumetric fog pass, in physical units (one world unit = one metre,
// extinction in m^-1). Shafts are not a separate effect: they are the shadowed
// part of lit medium, so a light only forms them when it (a) participates in
// the medium and (b) has a shadow map to be occluded by.

// ── Headless capture (`--probe` modes) ──────────────────────────────────────

// ── The portals plugin ─────────────────────────────────────────────────────

/// The deferred preset with the plugin's features (Helio's presets do not
/// list them: the portals are a Pulsar plugin's).
pub fn preset() -> helio_presets::PresetGraph {
    helio_mesh_host::presets::with(
        helio_presets::Preset::Deferred,
        &helio_feature_api::FeatureSet::new()
            .with(helio_pass_portal_cull::PortalCullFeature)
            .with(helio_pass_portal_instances::PortalFeature)
            // The lighting plugins' (Helio's presets list neither).
            .with(helio_pass_sky::SkyFeature)
            .with(helio_pass_gi::GiFeature),
    )
}

/// Hand `frame` (built with its own table, `PortalProjectionBridge::build`)
/// to `renderer`: its spaces become the table, and its live counts the
/// portal passes' `PORTAL_COUNTS` payload.
pub fn set_projection_frame(
    renderer: &mut helio::Renderer,
    frame: &helio_pass_portal_cull::PortalProjectionFrame,
) {
    renderer.set_coordinate_spaces(frame.renderer_coordinate_spaces());
    renderer.set_scene_payload(
        &helio_pass_portal_cull::PORTAL_COUNTS,
        std::sync::Arc::new(helio_pass_portal_cull::PortalCounts {
            views: frame.counts.portal_view_count,
            chains: frame.counts.portal_chain_count,
        }),
    );
}

// The plugin's rows as standalone hosts author them (contract rows, as
// `helio_components` does for Helio's own kinds).
use pulsar_scenedb_derive::SceneStore;

/// Ties a component to its contract row: same size, every field at the same
/// offset, stored under the contract's key. Adds the conversions. (As
/// `helio_components::same_layout!`, for rows Helio no longer has.)
macro_rules! same_layout {
    ($component:ident, $row:path, $contract:path, $key:literal; $($field:ident),* $(,)?) => {
        const _: () = {
            assert!(::core::mem::size_of::<$component>() == ::core::mem::size_of::<$row>());
            assert!(::core::mem::align_of::<$component>() == ::core::mem::align_of::<$row>());
            $(
                assert!(
                    ::core::mem::offset_of!($component, $field)
                        == ::core::mem::offset_of!($row, $field)
                );
            )*
            $contract.assert_producer::<$component>($key);
        };

        impl ::core::convert::From<$row> for $component {
            fn from(row: $row) -> Self {
                ::bytemuck::cast(row)
            }
        }

        impl ::core::convert::From<$component> for $row {
            fn from(component: $component) -> Self {
                ::bytemuck::cast(component)
            }
        }

        impl $component {
            /// This component as its contract row.
            pub fn row(&self) -> $row {
                ::bytemuck::cast(*self)
            }
        }

        impl ::core::ops::Deref for $component {
            type Target = $row;
            fn deref(&self) -> &$row {
                ::bytemuck::cast_ref(self)
            }
        }

        impl ::core::ops::DerefMut for $component {
            fn deref_mut(&mut self) -> &mut $row {
                ::bytemuck::cast_mut(self)
            }
        }
    };
}

/// Authored portal relationship and local aperture data.
///
/// This is intentionally distinct from [`PortalViewComponent`] and
/// [`PortalChainComponent`]. Those rows are derived GPU projections consumed by
/// the current render passes; this row is the persistent SceneDB-facing source
/// of truth. A portal can point to a peer in the same sublevel, another
/// sublevel, or a nested actor context without storing a sublevel index of its
/// own.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "portals")]
pub struct PortalComponent {
    #[gpu]
    pub peer: u64,
    #[gpu]
    pub flags: u32,
    #[gpu]
    pub _pad: u32,
    #[gpu]
    pub transform: [[f32; 4]; 4],
    #[gpu]
    pub half_extent: [f32; 2],
}
same_layout!(PortalComponent, helio_pass_portal_cull::PortalRow, helio_pass_portal_cull::PORTALS, "portals"; peer, flags, _pad, transform, half_extent);

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "portal_views")]
pub struct PortalViewComponent {
    #[gpu]
    pub transform: [f32; 16],
    #[gpu]
    pub inverse_transform: [f32; 16],
    #[gpu]
    pub half_extent: [f32; 2],
    #[gpu]
    pub coordinate_space: u32,
    /// Reserved compatibility word. Zero means the row is a physical portal
    /// for legacy manually-authored rows. The projection bridge sets
    /// [`Self::FLAG_MASK_HIDDEN`] on virtual nested rows so they can still be
    /// referenced by a chain without being stamped as another opening.
    #[gpu]
    pub _pad: u32,
}
same_layout!(PortalViewComponent, helio_pass_portal_cull::PortalViewRow, helio_pass_portal_cull::PORTAL_VIEWS, "portal_views"; transform, inverse_transform, half_extent, coordinate_space, _pad);

/// Derived frame counts for the portal projection tables.
///
/// The authored [`PortalComponent`] remains the source of truth. This row is
/// published by the projection bridge alongside the dense derived view and
/// chain rows so a renderer integration can distinguish active rows from a
/// growable SceneDB buffer's reserved capacity without changing the authored
/// data model.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "portal_projection_counts")]
pub struct PortalProjectionCountsComponent {
    #[gpu]
    pub portal_view_count: u32,
    #[gpu]
    pub portal_chain_count: u32,
    #[gpu]
    pub coordinate_space_count: u32,
    #[gpu]
    pub _pad: u32,
}
same_layout!(PortalProjectionCountsComponent, helio_pass_portal_cull::PortalProjectionCountsRow, helio_pass_portal_cull::PORTAL_PROJECTION_COUNTS, "portal_projection_counts"; portal_view_count, portal_chain_count, coordinate_space_count, _pad);

impl From<helio_pass_portal_cull::GpuPortalView> for PortalViewComponent {
    fn from(value: helio_pass_portal_cull::GpuPortalView) -> Self {
        helio_pass_portal_cull::PortalViewRow::from(value).into()
    }
}

impl From<PortalViewComponent> for helio_pass_portal_cull::GpuPortalView {
    fn from(value: PortalViewComponent) -> Self {
        helio_pass_portal_cull::PortalViewRow::from(value).into()
    }
}

/// One derived portal chain ([`helio_pass_portal_cull::PortalChain`]),
/// stored in the portal-chain pool the portal passes bind.
///
/// The chain is deliberately a variable-length SceneDB field. SceneDB mirrors
/// `portals` through one growable `u32` payload pool and one growable
/// `VarLenHandle` table; the renderer binds both buffers and uses the handle's
/// `count` as the runtime recursion depth. This is the single representation
/// for every depth, rather than a family of fixed `[u32; N]` component types.
#[derive(SceneStore, Clone, Debug, PartialEq)]
pub struct PortalChainComponent {
    #[gpu(buffer = "PortalChainComponent::portals")]
    pub portals: Vec<u32>,
}

impl From<helio_pass_portal_cull::PortalChain> for PortalChainComponent {
    fn from(chain: helio_pass_portal_cull::PortalChain) -> Self {
        Self {
            portals: chain.portals,
        }
    }
}

/// Publish a portal projection's derived rows into `world`.
///
/// The portal GPU ABI is entity-indexed. Callers reserve a contiguous
/// projection entity range whose entity indices match the dense row IDs, and
/// keep those entities separate from authored portal entities. This does not
/// spawn or repurpose entities: their lifetime stays with the host.
pub fn publish_portal_projection(
    world: &mut pulsar_scenedb::World,
    frame: &helio_pass_portal_cull::PortalProjectionFrame,
    view_entities: &[pulsar_scenedb::Entity],
    chain_entities: &[pulsar_scenedb::Entity],
    counts_entity: pulsar_scenedb::Entity,
) -> Result<(), helio_pass_portal_cull::ProjectionError> {
    use helio_pass_portal_cull::ProjectionError;
    for (kind, expected, entities) in [
        ("portal view", frame.portal_views.len(), view_entities),
        ("portal chain", frame.portal_chains.len(), chain_entities),
    ] {
        if entities.len() != expected {
            return Err(ProjectionError::SceneRowCount {
                kind,
                expected,
                actual: entities.len(),
            });
        }
        for (dense_index, entity) in entities.iter().enumerate() {
            if entity.index() as usize != dense_index {
                return Err(ProjectionError::NonDenseSceneRow {
                    kind,
                    dense_index,
                    entity_index: entity.index(),
                });
            }
        }
    }
    for (entity, row) in view_entities.iter().zip(&frame.portal_views) {
        world.insert(*entity, PortalViewComponent::from(*row));
    }
    for (entity, chain) in chain_entities.iter().zip(&frame.portal_chains) {
        world.insert(*entity, PortalChainComponent::from(chain.clone()));
    }
    world.insert(
        counts_entity,
        PortalProjectionCountsComponent::from(frame.counts),
    );
    Ok(())
}
