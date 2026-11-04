//! A long indoor showcase hallway for the VR demo.
//!
//! The corridor runs down −Z from the stage origin, divided into bays. Each bay
//! demonstrates one thing the renderer does — so walking its length is a tour
//! of the engine: PBR materials, spot lights, lens flare and light shafts,
//! emissive/HDR colour.
//! Fog, water and post-process volumes left Helio with their plugins (Pulsar
//! `plugins/vendor/fog`, `plugins/vendor/water`, `plugins/vendor/post`): bay 2
//! has no fog volume, bay 3's pool no water and bay 7 no grading volume.
//!
//! # Why it is built the way it is
//!
//! **Everything is bounded tightly, per bay.** Culling is driven by one world-space
//! bounding *sphere* per object, and a sphere fits a long thin thing badly: a single
//! corridor-length wall would have a bounding sphere enclosing the whole level, cull
//! nothing useful, and be easy to get wrong in the direction that deletes visible
//! geometry. So the shell is built as short per-bay segments with correct radii — which
//! is also what real level geometry looks like.
//!
//! **Radii are half-diagonals, not half-extents.** `insert_object` takes a bounding
//! *radius*. Passing a box's half-extent leaves its corners outside the sphere, and the
//! segment vanishes while a corner is still plainly on screen.
//!
//! **Content is at human scale around the stage origin.** OpenXR's stage origin maps 1:1
//! onto the world origin at floor level, so the player starts at `(0, 0, 0)` facing −Z
//! with their eyes near y = 1.6. A 3 m ceiling reads correctly in a headset; 2.2 m feels
//! like a crawlspace.
//!
//! **It is a forward-opaque graph.** `main.rs` builds the forward preset, so the
//! hallway only shows features that graph actually renders. Decals, foliage and virtual
//! geometry are deferred-graph passes — VG in particular crashes the pass (it expects a
//! G-buffer to write into), so it is deliberately absent; the shipping container from
//! `editor_demo` is loaded as a classic mesh instead (bay 5), parked beside an instanced
//! crate stack.

use glam::{Mat4, Quat, Vec3};
use helio::{GpuLight, LightType, Renderer};
use helio_asset_compat::{load_scene_bytes_with_config, LoadConfig};
use helio_mesh_api::MeshUpload;
use pulsar_scenedb::{Entity, World};

use crate::v3_demo_common::{
    box_mesh, cube_mesh, make_material, point_light, spawn_light, spawn_material, spawn_mesh,
    spawn_object, sphere_mesh, spot_light, update_light, update_object_transform,
};

/// Interior half-width of the corridor, in metres.
pub const HALL_HALF_WIDTH: f32 = 2.4;
/// Interior height.
pub const HALL_HEIGHT: f32 = 3.0;
/// Length of one bay along −Z.
pub const BAY_LENGTH: f32 = 8.0;
/// Number of bays; the corridor spans z = 0 to z = -BAY_COUNT * BAY_LENGTH.
pub const BAY_COUNT: usize = 8;

/// Objects the demo animates each frame.
///
/// Returned rather than kept in a global so `main.rs` owns the animation state and this
/// module stays a pure builder. The controller cubes are here too: `main.rs` reparents
/// them to the OpenXR grip poses every frame.
pub struct Animated {
    /// Rotating cubes, with the centre each rotates about (materials bay).
    pub spinners: Vec<(Entity, Vec3)>,
    /// Vertically bobbing orbs, with their rest positions (materials bay).
    pub bobbers: Vec<(Entity, Vec3)>,
    /// Per-bay accent lights, pulsed in sympathy with the emissive strips.
    pub pulse_lights: Vec<(Entity, Vec3, [f32; 3], f32)>,
    /// Cubes attached to the player's left and right controllers. `None`
    /// only if the corresponding `spawn_object` call failed.
    pub hand_cubes: [Option<Entity>; 2],
    /// Pool-bay orb: (object, rest position). It dips into the pool.
    pub pool_orb: Option<(Entity, Vec3)>,
    /// Hue-cycling accent lights (emissive / light-count bay).
    pub colour_lights: Vec<(Entity, Vec3, [f32; 3], f32)>,
}

fn bay_centre_z(index: usize) -> f32 {
    -(index as f32 + 0.5) * BAY_LENGTH
}

fn insert_box_mesh(world: &mut World, half: Vec3) -> Entity {
    spawn_mesh(world, box_mesh([0.0, 0.0, 0.0], [half.x, half.y, half.z]))
}

/// Insert an object at a world-space position and report malformed scene setup.
fn place(world: &mut World, mesh: Entity, material: Entity, pos: Vec3, radius: f32) {
    if let Err(error) = spawn_object(world, mesh, material, Mat4::from_translation(pos), radius) {
        log::error!(
            "VR scene object creation failed (mesh {}, material {}, position {:?}): {error}",
            mesh.index(),
            material.index(),
            pos
        );
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match (i as u32) % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

// ── Shared scene resources ────────────────────────────────────────────────────

struct Mats {
    dark_trim: Entity,
    mirror: Entity,
    gold: Entity,
    copper: Entity,
    chalk: Entity,
    glossy_red: Entity,
    emissive_cyan: Entity,
    emissive_warm: Entity,
    emissive_white: Entity,
    steel: Entity,
}

struct Meshes {
    cube: Entity,
    sphere: Entity,
    plinth: Entity,
    panel: Entity,
}

// ── Per-bay exhibits ──────────────────────────────────────────────────────────

/// Bay 0 — the PBR material space: a rotating metal cube, a bobbing orb, an
/// emissive strip paired with a real light. Everything else is built from the
/// same `make_material` parameters, just spread across roughness/metallic.
fn bay_materials(world: &mut World, z: f32, meshes: &Meshes, mats: &Mats, anim: &mut Animated) {
    let plinth_left = Vec3::new(-1.4, 0.45, z);
    place(world, meshes.plinth, mats.dark_trim, plinth_left, 0.6);
    let spin_centre = plinth_left + Vec3::new(0.0, 0.75, 0.0);
    if let Ok(id) = spawn_object(
        world,
        meshes.cube,
        mats.glossy_red,
        Mat4::from_translation(spin_centre),
        // Half-diagonal of a 0.28 half-extent cube: 0.28 * sqrt(3).
        0.28 * 1.7320508,
    ) {
        anim.spinners.push((id, spin_centre));
    }

    let plinth_right = Vec3::new(1.4, 0.45, z);
    place(world, meshes.plinth, mats.dark_trim, plinth_right, 0.6);
    let orb_rest = plinth_right + Vec3::new(0.0, 0.85, 0.0);
    if let Ok(id) = spawn_object(
        world,
        meshes.sphere,
        mats.gold,
        Mat4::from_translation(orb_rest),
        0.3,
    ) {
        anim.bobbers.push((id, orb_rest));
    }

    for side in [-1.0_f32, 1.0] {
        place(
            world,
            meshes.panel,
            mats.emissive_cyan,
            Vec3::new(side * (HALL_HALF_WIDTH - 0.08), HALL_HEIGHT - 0.7, z),
            1.0,
        );
    }

    // Material shelf: one sphere per material, spanning the roughness/metallic
    // space from mirror to chalk, so the whole range reads at a glance.
    let shelf = [
        mats.mirror,
        mats.gold,
        mats.copper,
        mats.chalk,
        mats.glossy_red,
    ];
    for (i, material) in shelf.into_iter().enumerate() {
        let x = -1.9 + i as f32 * 0.6;
        place(
            world,
            meshes.sphere,
            material,
            Vec3::new(x, 0.3, z + 3.2),
            0.3,
        );
    }
    let position = Vec3::new(0.0, HALL_HEIGHT - 0.5, z);
    let colour = [0.25, 0.85, 1.0];
    let intensity = 7.5;
    let light = spawn_light(
        world,
        point_light(position.into(), colour, intensity, BAY_LENGTH),
    );
    anim.pulse_lights.push((light, position, colour, intensity));
}

/// Bay 1 — light forms: downward fluorescent spot cones and warm wall sconces,
/// each with a visible emissive fixture, demonstrating `LightType::Spot` and
/// per-light shadows.
fn bay_spotlights(world: &mut World, z: f32, meshes: &Meshes, mats: &Mats) {
    for side in [-1.0_f32, 1.0] {
        let x = side * 1.2;
        spawn_light(
            world,
            spot_light(
                [x, HALL_HEIGHT - 0.05, z],
                [0.0, -1.0, 0.0],
                [0.9, 0.95, 1.0],
                5.0,
                6.5,
                1.22,
                1.48,
            ),
        );
        place(
            world,
            meshes.panel,
            mats.emissive_warm,
            Vec3::new(x, HALL_HEIGHT - 0.16, z),
            0.7,
        );

        spawn_light(
            world,
            point_light(
                [side * (HALL_HALF_WIDTH - 0.15), 1.6, z],
                [1.0, 0.65, 0.3],
                2.2,
                4.5,
            ),
        );
        let sconce = insert_box_mesh(world, Vec3::new(0.06, 0.12, 0.25));
        place(
            world,
            sconce,
            mats.dark_trim,
            Vec3::new(side * (HALL_HALF_WIDTH - 0.06), 1.6, z),
            0.3,
        );
    }
}

/// Bay 2 — a bright flare light (ghost lens flare) plus a god-ray overhead light.
fn bay_flare(world: &mut World, z: f32, meshes: &Meshes, mats: &Mats) {
    spawn_light(
        world,
        GpuLight {
            position_range: [0.0, 1.6, z, 9.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [1.0, 0.85, 0.5, 12.0],
            shadow_index: u32::MAX,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
            flare_enabled: 1,
            flare_type: 1,
            flare_intensity: 0.3,
            flare_scale: 1.0,
            flare_tint_r: 1.0,
            flare_tint_g: 0.7,
            flare_tint_b: 0.35,
            ..Default::default()
        },
    );

    let mut shaft = point_light([0.0, HALL_HEIGHT - 0.15, z], [0.7, 0.8, 1.0], 7.0, 9.0);
    shaft.god_rays_enabled = 1;
    spawn_light(world, shaft);

    place(
        world,
        meshes.cube,
        mats.emissive_warm,
        Vec3::new(0.0, 1.6, z),
        0.4,
    );
}

/// Bay 3 — a raised pool with a sphere that bobs in and out of it.
fn bay_pool(world: &mut World, z: f32, meshes: &Meshes, mats: &Mats, anim: &mut Animated) {
    let pool_centre = Vec3::new(-1.5, 0.45, z);
    let pool_half = Vec3::new(0.95, 0.45, 3.4);
    let pool_mesh = insert_box_mesh(world, pool_half);
    place(
        world,
        pool_mesh,
        mats.dark_trim,
        pool_centre,
        pool_half.length(),
    );

    let orb_rest = pool_centre + Vec3::new(0.0, 0.55, 0.0);
    if let Ok(id) = spawn_object(
        world,
        meshes.sphere,
        mats.glossy_red,
        Mat4::from_translation(orb_rest),
        0.3,
    ) {
        anim.pool_orb = Some((id, orb_rest));
    }
}

/// Bay 4 — a warm emissive plinth. (Its GPU particle fountain is the
/// particles plugin's now; Helio's own hosts draw no particles.)
fn bay_plinth(world: &mut World, z: f32, meshes: &Meshes, mats: &Mats) {
    place(
        world,
        meshes.plinth,
        mats.emissive_warm,
        Vec3::new(0.0, 0.05, z),
        0.8,
    );

    spawn_light(world, point_light([0.0, 2.4, z], [1.0, 0.5, 0.2], 3.0, 5.0));
}

/// Bay 5 — instancing: a crate stack and tile floor of identical objects that
/// the renderer auto-batches into a handful of instanced draws, a condensed
/// one_million_cubes. The left side is kept clear for the shipping container
/// parked there by `build`.
fn bay_instancing(world: &mut World, z: f32, mats: &Mats) {
    let crate_half = Vec3::new(0.3, 0.3, 0.3);
    let crate_mesh = insert_box_mesh(world, crate_half);
    for x in 0..3 {
        for d in 0..2 {
            for y in 0..4 {
                let pos = Vec3::new(
                    1.35 + x as f32 * 0.62,
                    0.3 + y as f32 * 0.62,
                    z + d as f32 * 0.62,
                );
                let material = if (x + d + y) % 2 == 0 {
                    mats.gold
                } else {
                    mats.chalk
                };
                place(world, crate_mesh, material, pos, crate_half.length());
            }
        }
    }

    let tile_mesh = insert_box_mesh(world, Vec3::new(0.12, 0.05, 0.12));
    for x in 0..5 {
        for d in 0..4 {
            place(
                world,
                tile_mesh,
                mats.steel,
                Vec3::new(0.5 + x as f32 * 0.5, 0.05, z - 3.0 + d as f32 * 0.5),
                0.17,
            );
        }
    }
}

/// Load the shipping-container FBX (the same asset `editor_demo` uses) as a
/// *classic* mesh so it renders through the forward-opaque graph — virtual geometry
/// is deferred-only and would not draw here.
///
/// Returns `(mesh, material, local_centre, local_size)` so the caller can place
/// the container on the floor regardless of the FBX's origin. `None` if the
/// asset cannot be loaded, so a missing model never crashes the demo.
fn load_container(world: &mut World) -> Option<(Entity, Entity, Vec3, Vec3)> {
    const CONTAINER_FBX: &[u8] = include_bytes!("../models/source/container with textures.fbx");
    let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/source");

    let scene = match load_scene_bytes_with_config(
        CONTAINER_FBX,
        "fbx",
        Some(base_dir.as_path()),
        LoadConfig::default()
            .with_uv_flip(false)
            .with_merge_meshes(true)
            .with_import_scale(glam::Vec3::splat(1.0 / 200.0)),
    ) {
        Ok(scene) => scene,
        Err(e) => {
            log::warn!("[hallway] shipping container FBX failed to load: {e}");
            return None;
        }
    };

    let mat_ids = helio_mesh_host::upload_scene_materials(world, &scene);
    let sm = match scene.sectioned_mesh {
        Some(sm) => sm,
        None => {
            log::warn!("[hallway] shipping container FBX produced no sectioned mesh");
            return None;
        }
    };
    let Some(section) = sm.sections.iter().find(|s| !s.indices.is_empty()) else {
        log::warn!("[hallway] shipping container FBX has no index data");
        return None;
    };

    let mut bb_min = Vec3::splat(f32::INFINITY);
    let mut bb_max = Vec3::splat(f32::NEG_INFINITY);
    for v in &sm.vertices {
        let p = Vec3::from(v.position);
        bb_min = bb_min.min(p);
        bb_max = bb_max.max(p);
    }
    let local_centre = (bb_min + bb_max) * 0.5;

    let fallback = spawn_material(
        world,
        make_material([0.5, 0.5, 0.5, 1.0], 0.8, 0.0, [0.0; 3], 0.0),
    );
    let material = section
        .material_index
        .and_then(|i| mat_ids.get(i))
        .copied()
        .unwrap_or(fallback);

    let mesh = spawn_mesh(
        world,
        MeshUpload {
            vertices: sm.vertices.clone(),
            indices: section.indices.clone(),
        },
    );

    Some((mesh, material, local_centre, bb_max - bb_min))
}

/// Bay 6 — emissive/HDR colour targets plus a grid of hue-cycling lights, a
/// condensed take on the HDR/colour-grading and light-benchmark demos.
fn bay_emissive_colour(
    world: &mut World,
    z: f32,
    meshes: &Meshes,
    mats: &Mats,
    anim: &mut Animated,
) {
    let targets = [
        (
            make_material([1.0, 0.1, 0.1, 1.0], 0.3, 0.0, [10.0, 0.5, 0.5], 10.0),
            -1.4,
        ),
        (
            make_material([0.1, 1.0, 0.1, 1.0], 0.3, 0.0, [0.5, 10.0, 0.5], 10.0),
            0.0,
        ),
        (
            make_material([0.1, 0.1, 1.0, 1.0], 0.3, 0.0, [0.5, 0.5, 10.0], 10.0),
            1.4,
        ),
    ];
    for (gpu, x) in targets {
        let mat = spawn_material(world, gpu);
        place(world, meshes.cube, mat, Vec3::new(x, 0.6, z), 0.6);
    }

    for i in 0..6 {
        let pos = Vec3::new(
            -1.7 + (i % 3) as f32 * 1.7,
            2.2,
            z + ((i / 3) as f32 - 0.5) * 5.0,
        );
        let colour = hsv_to_rgb(i as f32 / 6.0, 0.8, 1.0);
        let base = 3.0;
        let light = spawn_light(world, point_light(pos.into(), colour, base, 6.0));
        anim.colour_lights.push((light, pos, colour, base));
    }
    place(
        world,
        meshes.plinth,
        mats.emissive_white,
        Vec3::new(0.0, 0.05, z),
        0.8,
    );
}

/// Bay 7 — a blindingly bright emissive sun.
fn bay_bright_sun(world: &mut World, z: f32, meshes: &Meshes) {
    let sun_mat = spawn_material(
        world,
        make_material([1.0, 0.9, 0.7, 1.0], 0.2, 0.0, [50.0, 45.0, 35.0], 50.0),
    );
    place(world, meshes.cube, sun_mat, Vec3::new(0.0, 1.4, z), 0.6);
    spawn_light(
        world,
        point_light([0.0, 1.4, z], [1.0, 0.9, 0.7], 12.0, 8.0),
    );
}

// ── Build ─────────────────────────────────────────────────────────────────────

pub fn build(world: &mut World) -> Animated {
    // ── Materials ────────────────────────────────────────────────────────────
    // Deliberately spread across the roughness/metallic space: a showcase that is all
    // mid-roughness dielectric demonstrates almost nothing about the BRDF. The mirror and
    // the chalk are the two ends; everything else sits between them.
    let mut mat = |c: [f32; 4], rough: f32, metal: f32, em: [f32; 3], strength: f32| {
        spawn_material(world, make_material(c, rough, metal, em, strength))
    };

    let concrete = mat([0.24, 0.29, 0.34, 1.0], 0.88, 0.12, [0.0; 3], 0.0);
    let dark_trim = mat([0.035, 0.055, 0.075, 1.0], 0.48, 0.25, [0.0; 3], 0.0);
    let mirror = mat([0.95, 0.96, 0.98, 1.0], 0.04, 1.0, [0.0; 3], 0.0);
    let gold = mat([1.0, 0.78, 0.34, 1.0], 0.28, 1.0, [0.0; 3], 0.0);
    let copper = mat([0.95, 0.55, 0.42, 1.0], 0.16, 1.0, [0.0; 3], 0.0);
    let chalk = mat([0.86, 0.84, 0.80, 1.0], 1.0, 0.0, [0.0; 3], 0.0);
    let glossy_red = mat([0.65, 0.06, 0.08, 1.0], 0.12, 0.0, [0.0; 3], 0.0);
    let emissive_cyan = mat([0.02, 0.05, 0.06, 1.0], 0.6, 0.0, [0.1, 0.9, 1.0], 6.0);
    let emissive_warm = mat([0.06, 0.04, 0.02, 1.0], 0.6, 0.0, [1.0, 0.62, 0.22], 7.0);
    let emissive_white = mat([0.05, 0.05, 0.05, 1.0], 0.6, 0.0, [0.7, 0.7, 0.7], 3.0);
    let steel = mat([0.55, 0.57, 0.60, 1.0], 0.35, 0.75, [0.0; 3], 0.0);
    let wall_glow = mat([0.025, 0.08, 0.10, 1.0], 0.42, 0.2, [0.02, 0.55, 0.8], 3.0);
    let warm_signal = mat([0.12, 0.045, 0.018, 1.0], 0.5, 0.1, [1.0, 0.24, 0.035], 4.0);

    let mats = Mats {
        dark_trim,
        mirror,
        gold,
        copper,
        chalk,
        glossy_red,
        emissive_cyan,
        emissive_warm,
        emissive_white,
        steel,
    };

    // ── Shell ────────────────────────────────────────────────────────────────
    let floor_half = Vec3::new(HALL_HALF_WIDTH, 0.1, BAY_LENGTH * 0.5);
    let wall_half = Vec3::new(0.1, HALL_HEIGHT * 0.5, BAY_LENGTH * 0.5);

    let floor_mesh = insert_box_mesh(world, floor_half);
    let wall_mesh = insert_box_mesh(world, wall_half);
    let wall_panel_mesh = insert_box_mesh(world, Vec3::new(0.035, 0.64, 2.55));
    let kick_mesh = insert_box_mesh(world, Vec3::new(0.055, 0.22, BAY_LENGTH * 0.46));
    let pilaster_mesh = insert_box_mesh(world, Vec3::new(0.09, 1.45, 0.12));
    let route_mesh = insert_box_mesh(world, Vec3::new(0.035, 0.012, BAY_LENGTH * 0.43));

    for bay in 0..BAY_COUNT {
        let z = bay_centre_z(bay);
        place(
            world,
            floor_mesh,
            concrete,
            Vec3::new(0.0, -0.1, z),
            floor_half.length(),
        );
        place(
            world,
            floor_mesh,
            dark_trim,
            Vec3::new(0.0, HALL_HEIGHT + 0.1, z),
            floor_half.length(),
        );
        for side in [-1.0_f32, 1.0] {
            place(
                world,
                wall_mesh,
                if bay % 2 == 0 { concrete } else { dark_trim },
                Vec3::new(side * (HALL_HALF_WIDTH + 0.1), HALL_HEIGHT * 0.5, z),
                wall_half.length(),
            );
        }

        // Recessed light wells and durable lower kick plates make the shell read
        // as a room from both eyes, even when an exhibit's local lighting is dim.
        for side in [-1.0_f32, 1.0] {
            place(
                world,
                wall_panel_mesh,
                if bay % 2 == 0 { dark_trim } else { concrete },
                Vec3::new(side * (HALL_HALF_WIDTH - 0.045), 1.65, z),
                2.63,
            );
            place(
                world,
                kick_mesh,
                if bay % 2 == 0 { steel } else { dark_trim },
                Vec3::new(side * (HALL_HALF_WIDTH - 0.02), 0.22, z),
                3.69,
            );
            // Tall luminous pilasters frame each bay and give a clear visual
            // rhythm for orientation while walking through the exhibits.
            let edge_z = z + BAY_LENGTH * 0.47;
            place(
                world,
                pilaster_mesh,
                wall_glow,
                Vec3::new(side * (HALL_HALF_WIDTH - 0.12), 1.5, edge_z),
                1.46,
            );
        }

        // A thin illuminated route stripe anchors the floor in stereo and
        // gives the long corridor a legible path through the showcases.
        for side in [-1.0_f32, 1.0] {
            place(
                world,
                route_mesh,
                if bay % 2 == 0 { wall_glow } else { warm_signal },
                Vec3::new(side * 1.58, 0.012, z),
                3.45,
            );
        }
    }

    // Mirrored end cap, so the corridor terminates in geometry rather than in the void
    // and the far end shows the whole hall back at you.
    let end_half = Vec3::new(HALL_HALF_WIDTH + 0.2, HALL_HEIGHT * 0.5, 0.15);
    let end_mesh = insert_box_mesh(world, end_half);
    place(
        world,
        end_mesh,
        mirror,
        Vec3::new(
            0.0,
            HALL_HEIGHT * 0.5,
            bay_centre_z(BAY_COUNT - 1) - BAY_LENGTH * 0.5,
        ),
        end_half.length(),
    );

    // The last bay terminates in a readable focal installation: an open
    // instrument arch around a floating energy core. The center is elevated
    // above head level to preserve a comfortable clear walking lane.
    let portal_z = bay_centre_z(BAY_COUNT - 1) - 1.7;
    let portal_post = insert_box_mesh(world, Vec3::new(0.22, 1.42, 0.24));
    let portal_inlay = insert_box_mesh(world, Vec3::new(0.035, 1.12, 0.025));
    let portal_lintel = insert_box_mesh(world, Vec3::new(1.72, 0.2, 0.24));
    let portal_cap = insert_box_mesh(world, Vec3::new(1.95, 0.07, 0.33));
    for side in [-1.0_f32, 1.0] {
        place(
            world,
            portal_post,
            gold,
            Vec3::new(side * 1.62, 1.42, portal_z),
            1.46,
        );
        place(
            world,
            portal_inlay,
            wall_glow,
            Vec3::new(side * 1.62, 1.42, portal_z + 0.255),
            1.13,
        );
    }
    place(
        world,
        portal_lintel,
        steel,
        Vec3::new(0.0, 2.78, portal_z),
        1.75,
    );
    place(
        world,
        portal_cap,
        warm_signal,
        Vec3::new(0.0, 2.97, portal_z),
        1.96,
    );
    let core_mesh = spawn_mesh(world, sphere_mesh([0.0, 0.0, 0.0], 0.62));
    place(
        world,
        core_mesh,
        emissive_cyan,
        Vec3::new(0.0, 1.92, portal_z),
        0.63,
    );
    let core_halo = insert_box_mesh(world, Vec3::new(0.82, 0.025, 0.025));
    for angle in [0.0_f32, 0.7, 1.4, 2.1] {
        let p = Vec3::new(angle.cos() * 0.87, 1.92 + angle.sin() * 0.87, portal_z);
        place(world, core_halo, gold, p, 0.88);
    }
    spawn_light(
        world,
        point_light([0.0, 2.0, portal_z], [0.05, 0.65, 1.0], 16.0, 7.0),
    );

    // ── Shared exhibit meshes ────────────────────────────────────────────────
    let meshes = Meshes {
        cube: spawn_mesh(world, cube_mesh([0.0, 0.0, 0.0], 0.28)),
        sphere: spawn_mesh(world, sphere_mesh([0.0, 0.0, 0.0], 0.3)),
        plinth: insert_box_mesh(world, Vec3::new(0.35, 0.45, 0.35)),
        panel: insert_box_mesh(world, Vec3::new(0.06, 0.5, 1.6)),
    };

    // ── Controller cubes ─────────────────────────────────────────────────────
    // Small bright cubes `main.rs` reparents to the OpenXR grip poses each frame.
    let hand_mesh = insert_box_mesh(world, Vec3::new(0.05, 0.05, 0.05));
    let hand_mat = spawn_material(
        world,
        make_material([0.05, 0.05, 0.06, 1.0], 0.4, 0.0, [0.2, 1.0, 0.9], 8.0),
    );
    let mut hand_cubes = [None; 2];
    for (i, side) in [1.0_f32, -1.0].into_iter().enumerate() {
        let start = Vec3::new(side * 0.2, 1.4, -0.4);
        if let Ok(id) = spawn_object(
            world,
            hand_mesh,
            hand_mat,
            Mat4::from_translation(start),
            0.15,
        ) {
            hand_cubes[i] = Some(id);
        }
    }

    // ── Per-bay exhibits ─────────────────────────────────────────────────────
    let mut anim = Animated {
        spinners: Vec::new(),
        bobbers: Vec::new(),
        pulse_lights: Vec::new(),
        hand_cubes,
        pool_orb: None,
        colour_lights: Vec::new(),
    };

    for bay in 0..BAY_COUNT {
        let z = bay_centre_z(bay);
        match bay {
            0 => bay_materials(world, z, &meshes, &mats, &mut anim),
            1 => bay_spotlights(world, z, &meshes, &mats),
            2 => bay_flare(world, z, &meshes, &mats),
            3 => bay_pool(world, z, &meshes, &mats, &mut anim),
            4 => bay_plinth(world, z, &meshes, &mats),
            5 => bay_instancing(world, z, &mats),
            6 => bay_emissive_colour(world, z, &meshes, &mats, &mut anim),
            7 => bay_bright_sun(world, z, &meshes),
            _ => unreachable!(),
        }
    }

    // ── Shipping container ──────────────────────────────────────────────────
    // Parked along the left wall of the instancing bay, long axis down the
    // corridor. Rotating about Y maps the FBX's long (X) axis onto the
    // corridor's Z axis; the height axis is unchanged so the bottom sits on
    // the floor.
    if let Some((container_mesh, container_mat, local_centre, local_size)) = load_container(world) {
        let z = bay_centre_z(5);
        let centre = Vec3::new(
            -HALL_HALF_WIDTH + local_size.z * 0.5 + 0.03,
            local_size.y * 0.5,
            z,
        );
        let transform = Mat4::from_translation(centre)
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * Mat4::from_translation(-local_centre);
        let radius = (local_size * 0.5).length().max(0.5);
        let _ = spawn_object(world, container_mesh, container_mat, transform, radius);
    }

    // Cool fill at the entrance, so the first bay is not lit solely by its own accent —
    // otherwise the whole corridor reads as one colour from the doorway.
    spawn_light(
        world,
        point_light([0.0, HALL_HEIGHT - 0.6, 1.5], [0.6, 0.7, 1.0], 6.0, 8.0),
    );

    anim
}

/// Advance the animated exhibits. Called once per frame with the scene time.
pub fn animate(world: &mut World, renderer: &mut Renderer, animated: &mut Animated, time: f32) {
    for (index, (id, centre)) in animated.spinners.iter().enumerate() {
        // Staggered rates: a corridor rotating in unison reads as one mechanism rather
        // than as separate exhibits.
        let rate = 0.6 + index as f32 * 0.17;
        let transform = Mat4::from_translation(*centre)
            * Mat4::from_quat(Quat::from_euler(
                glam::EulerRot::YXZ,
                time * rate,
                time * rate * 0.6,
                0.0,
            ));
        let _ = update_object_transform(world, renderer, *id, transform);
    }

    for (index, (id, rest)) in animated.bobbers.iter().enumerate() {
        let offset = (time * 1.1 + index as f32 * 0.8).sin() * 0.18;
        let _ = update_object_transform(
            world,
            renderer,
            *id,
            Mat4::from_translation(*rest + Vec3::Y * offset),
        );
    }

    for (index, (id, position, colour, base)) in animated.pulse_lights.iter().enumerate() {
        // Shallow pulse — deep flicker in a headset is unpleasant at best and a migraine
        // trigger at worst, so this stays well inside a gentle band.
        let pulse = 0.85 + 0.15 * (time * 0.9 + index as f32 * 1.3).sin();
        update_light(
            world,
            *id,
            point_light((*position).into(), *colour, base * pulse, BAY_LENGTH),
        );
    }

    // Pool orb: bob up and down, dipping into the pool.
    if let Some((id, rest)) = &mut animated.pool_orb {
        let y = rest.y + (time * 1.4).sin() * 0.35;
        let _ = update_object_transform(
            world,
            renderer,
            *id,
            Mat4::from_translation(Vec3::new(rest.x, y, rest.z)),
        );
    }

    // Hue-cycle the emissive-bay lights, like the light-benchmark's colour sweep.
    let count = animated.colour_lights.len().max(1);
    for (index, (id, position, _colour, base)) in animated.colour_lights.iter().enumerate() {
        let hue = (time * 0.4 + index as f32 / count as f32) % 1.0;
        let colour = hsv_to_rgb(hue, 0.8, 1.0);
        update_light(
            world,
            *id,
            point_light((*position).into(), colour, *base, 6.0),
        );
    }
}
