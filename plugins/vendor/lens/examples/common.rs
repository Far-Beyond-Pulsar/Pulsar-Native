//! What the lens plugin's examples share: the part of Helio's
//! `v3_demo_common` they use, the plugin's rows as standalone hosts
//! author them, and the preset with the plugin's features.
#![allow(dead_code)]

use glam::{Mat4, Vec3};
use helio::{GpuLight, SceneDbHandle};
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
    helio_components::AtmosphereComponent::register_gpu_columns_growable(&mut gpu_store, 4, device);
    helio_components::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, device);
    helio_components::MaterialComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_components::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_components::LightComponent::register_gpu_columns_growable(
        &mut gpu_store,
        helio_pass_forward_lit::MAX_LIGHTS,
        device,
    );
    helio_components::CameraPostProcessComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    LensComponent::register_gpu_columns_growable(&mut gpu_store, 4096, device);
    LensElementComponent::register_gpu_columns_growable(&mut gpu_store, 4096, device);
    helio_components::GlobalFogComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_components::LocalFogVolumeComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        device,
    );
    helio_components::VolumetricFogSettingsComponent::register_gpu_columns_growable(
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

pub fn make_material(
    base_color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    emissive_strength: f32,
) -> helio_components::MaterialComponent {
    MaterialComponent::from(helio_pass_gbuffer::MaterialRow::new(
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

// ── SceneDB-owned scene content ─────────────────────────────────────────────
//
// A demo's placed objects and lights are authored as *pass-owned* SceneDB
// components — `helio_components::StaticObjectComponent` and
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
pub use helio_components::{MeshComponent, StaticObjectComponent};

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
    let component = StaticObjectComponent::from(helio_pass_gbuffer::StaticObjectRow::new(
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
        LightComponent::from(helio_pass_forward_lit::LightRow::from(light)),
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

/// Author one persistent camera-settings row per view. Interactive examples call
/// this after editing their controls, before flushing the world's GPU mirror.
pub fn set_camera_postprocess(
    world: &mut World,
    view_id: u32,
    settings: &helio_pass_postprocess::PostProcessSettings,
) {
    use helio_components::CameraPostProcessComponent;
    let existing = world
        .query::<&CameraPostProcessComponent>()
        .find(|(_, row)| row.view_id == view_id)
        .map(|(entity, _)| entity);
    let entity = existing.unwrap_or_else(|| world.spawn());
    world.insert(
        entity,
        CameraPostProcessComponent::from(helio_pass_post_volumes::CameraPostProcessRow::new(
            view_id, settings,
        )),
    );
}

/// Author one persistent lens row per view (the camera's optics and focus).
pub fn set_camera_lens(world: &mut World, lens: LensComponent) {
    use LensComponent;
    let existing = world
        .query::<&LensComponent>()
        .find(|(_, row)| row.view_id == lens.view_id)
        .map(|(entity, _)| entity);
    let entity = existing.unwrap_or_else(|| world.spawn());
    world.insert(entity, lens);
}

// ── Participating media and light shafts (component API) ─────────────────────
//
// Fog is world data: global and local media are SceneDB components owned by
// the volumetric fog pass, in physical units (one world unit = one metre,
// extinction in m^-1). Shafts are not a separate effect: they are the shadowed
// part of lit medium, so a light only forms them when it (a) participates in
// the medium and (b) has a shadow map to be occluded by.

// ── Headless capture (`--probe` modes) ──────────────────────────────────────

// ── The lens plugin ─────────────────────────────────────────────────────

/// The deferred preset with the plugin's features (Helio's presets do not
/// list them: the lens are a Pulsar plugin's).
pub fn preset() -> helio_presets::PresetGraph {
    helio_presets::Preset::Deferred.with_features(
        &helio_feature_api::FeatureSet::new()
            .with(helio_pass_lens::LensFeature)
            .with(helio_pass_flare::LensFlareFeature),
    )
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

/// A camera's lens. One row per view; see the module doc.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_lenses")]
pub struct LensComponent {
    #[gpu]
    pub view_id: u32,
    #[gpu]
    pub enabled: u32,
    /// [`FOCUS_MANUAL`] or [`FOCUS_AUTO_POINT`].
    #[gpu]
    pub focus_mode: u32,
    /// 0 economical, 1 high (more gather taps), 2 ultra. Also the number of
    /// wavelength samples of lateral chromatic aberration: 3, 7, 16.
    #[gpu]
    pub quality: u32,
    #[gpu]
    pub focal_length_mm: f32,
    #[gpu]
    pub f_number: f32,
    /// Transmission stop; the light lost against the f-number is applied by
    /// a [`element_kind::T_STOP_LOSS`] element.
    #[gpu]
    pub t_stop: f32,
    #[gpu]
    pub sensor_width_mm: f32,
    /// Zero derives it from the width at 3:2.
    #[gpu]
    pub sensor_height_mm: f32,
    #[gpu]
    pub focus_distance_m: f32,
    /// Autofocus point in uv (y down).
    #[gpu]
    pub af_u: f32,
    #[gpu]
    pub af_v: f32,
    /// Autofocus ease rate in 1/s (<= 0: instant).
    #[gpu]
    pub af_speed: f32,
    /// Largest circle of confusion diameter in pixels (<= 0: 32).
    #[gpu]
    pub max_coc_px: f32,
    /// First row of this lens's elements in the `lens_elements` buffer.
    #[gpu]
    pub element_first: u32,
    #[gpu]
    pub element_count: u32,
    /// Scheimpflug: focal plane tilt in radians about an axis in the image
    /// plane at `tilt_axis` radians from +x.
    #[gpu]
    pub tilt_angle: f32,
    #[gpu]
    pub tilt_axis: f32,
    /// Petzval field curvature in dioptres at the frame corner; the focus
    /// shifts by `field_curvature * r^2`.
    #[gpu]
    pub field_curvature: f32,
    /// Iris blade count (0 = circular); shapes the flare pupil.
    #[gpu]
    pub aperture_blades: u32,
    /// Iris rotation in radians.
    #[gpu]
    pub aperture_rotation: f32,
    /// Autofocus sampling radius in uv (<= 0: 0.02).
    #[gpu]
    pub af_radius: f32,
    /// The overscan factor (>= 1; 0 reads as 1) the camera rendering this view
    /// widened its field of view by: the image the pass is given shows
    /// `overscan` times the frame's tangent extent about the frame centre, and
    /// the pass maps it back to the frame. Author it with
    /// `ray_map::applied_overscan` from the lens and elements the
    /// camera widened by; see the README.
    #[gpu]
    pub overscan: f32,
    #[gpu]
    pub _pad1: f32,
}
same_layout!(LensComponent, helio_pass_lens::LensRow, helio_pass_lens::CAMERA_LENSES, "camera_lenses"; view_id, enabled, focus_mode, quality, focal_length_mm, f_number, t_stop, sensor_width_mm, sensor_height_mm, focus_distance_m, af_u, af_v, af_speed, max_coc_px, element_first, element_count, tilt_angle, tilt_axis, field_curvature, aperture_blades, aperture_rotation, af_radius, overscan, _pad1);

/// One optical element in front of the sensor. Rows are indexed from
/// [`LensComponent::element_first`]; kind [`element_kind::NONE`] rows are
/// skipped, so a gap in the buffer is inert.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "lens_elements")]
pub struct LensElementComponent {
    /// [`element_kind`].
    #[gpu]
    pub kind: u32,
    /// [`region_kind`].
    #[gpu]
    pub region_kind: u32,
    /// Polygon vertex count.
    #[gpu]
    pub region_count: u32,
    #[gpu]
    pub flags: u32,
    /// Width of the region's soft edge in image-height units. Negative:
    /// derived from the aperture (see [`element_kind::DIOPTER`]).
    #[gpu]
    pub feather: f32,
    #[gpu]
    pub _pad0: f32,
    #[gpu]
    pub _pad1: f32,
    #[gpu]
    pub _pad2: f32,
    /// Kind-specific parameters.
    #[gpu]
    pub params: [f32; 16],
    /// Region-kind-specific geometry.
    #[gpu]
    pub region: [f32; 16],
}
same_layout!(LensElementComponent, helio_pass_lens::LensElementRow, helio_pass_lens::LENS_ELEMENTS, "lens_elements"; kind, region_kind, region_count, flags, feather, _pad0, _pad1, _pad2, params, region);

impl LensElementComponent {
    pub fn with_anamorphic(self, squeeze: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_anamorphic(squeeze)
            .into()
    }

    pub fn with_aperture_texture(self, layer: u32, amount: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_aperture_texture(layer, amount)
            .into()
    }

    pub fn with_apodization(self, sigma: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_apodization(sigma)
            .into()
    }

    pub fn with_cats_eye(self, strength: f32, barrel_radius: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_cats_eye(strength, barrel_radius)
            .into()
    }

    pub fn with_central_obstruction(self, ratio: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_central_obstruction(ratio)
            .into()
    }

    pub fn with_diffusion_tail(self, tail: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_diffusion_tail(tail)
            .into()
    }

    pub fn with_f_number_rounding(self, straight_from_f_number: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_f_number_rounding(straight_from_f_number)
            .into()
    }

    pub fn with_feather(self, feather: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_feather(feather)
            .into()
    }

    pub fn with_glint(self, contrast: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_glint(contrast)
            .into()
    }

    pub fn with_halation_tint(self, tint: [f32; 3]) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_halation_tint(tint)
            .into()
    }

    pub fn with_iris(self, blades: u32, curvature: f32, rotation: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_iris(blades, curvature, rotation)
            .into()
    }

    pub fn with_kannala_brandt(self, k: [f32; 4]) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_kannala_brandt(k)
            .into()
    }

    pub fn with_mask_transform(self, scale: f32, offset: [f32; 2], tile: bool) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_mask_transform(scale, offset, tile)
            .into()
    }

    pub fn with_mount_distance(self, metres: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_mount_distance(metres)
            .into()
    }

    pub fn with_onion_rings(self, amplitude: f32, rings: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_onion_rings(amplitude, rings)
            .into()
    }

    pub fn with_panini(self, d: f32, s: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_panini(d, s)
            .into()
    }

    pub fn with_prism_falloff(self, ratio: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_prism_falloff(ratio)
            .into()
    }

    pub fn with_prism_ring(self) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_prism_ring()
            .into()
    }

    pub fn with_prism_spin(self, radians: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_prism_spin(radians)
            .into()
    }

    pub fn with_psf_amount(self, amount: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_psf_amount(amount)
            .into()
    }

    pub fn with_seam_darkening(self, amount: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_seam_darkening(amount)
            .into()
    }

    pub fn with_spherical_aberration(self, amount: f32, asymmetry: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_spherical_aberration(amount, asymmetry)
            .into()
    }

    pub fn with_swirl(self, swirl: f32) -> Self {
        helio_pass_lens::LensElementRow::from(self)
            .with_swirl(swirl)
            .into()
    }
}
