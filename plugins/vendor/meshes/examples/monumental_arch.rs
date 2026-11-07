//! Monumental stone arch inspired by the Arc de Triomphe's exterior proportions.
//! Approximately 50 x 44.8 x 22.2 metres (City of Paris architectural overview):
//! https://www.paris.fr/pages/a-la-place-de-l-arc-de-triomphe-devait-troner-un-elephant-18396
//! This is an original procedural interpretation, not a surveyed replica.
//! Its smoke post-process volume left Helio with its plugin (Pulsar
//! `plugins/vendor/post`); this scene no longer has one.
//!
//! The sky (the atmosphere and its clouds) and GI left Helio with their
//! plugins (Pulsar `plugins/vendor/sky`, `plugins/vendor/gi`): this scene
//! has no sky and no GI probes; the background colour and the ambient
//! light stand in.
use crate::{architectural_mesh::Mesh, v3_demo_common::*};
use glam::{Mat4, Vec2, Vec3};
use helio::Camera;
use helio_mesh_api::MeshUpload;
use pulsar_scenedb::{Entity, World};

// Extrude a counter-clockwise polygon along depth. Rotating the entire prism
// makes the transverse passage without changing triangle winding.
fn prism(mesh: &mut Mesh, polygon: [Vec2; 4], front: f32, back: f32, transverse: bool) {
    let point = |p: Vec2, z: f32| {
        if transverse {
            Vec3::new(z, p.y, -p.x)
        } else {
            Vec3::new(p.x, p.y, z)
        }
    };
    let p = [
        point(polygon[0], front),
        point(polygon[1], front),
        point(polygon[2], front),
        point(polygon[3], front),
        point(polygon[0], back),
        point(polygon[1], back),
        point(polygon[2], back),
        point(polygon[3], back),
    ];
    for q in [
        [0, 1, 2, 3],
        [5, 4, 7, 6],
        [4, 0, 3, 7],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
        [4, 5, 1, 0],
    ] {
        mesh.quad(p[q[0]], p[q[1]], p[q[2]], p[q[3]]);
    }
}
fn wedge(
    mesh: &mut Mesh,
    x0: f32,
    x1: f32,
    bottom0: f32,
    bottom1: f32,
    top: f32,
    front: f32,
    back: f32,
    transverse: bool,
) {
    prism(
        mesh,
        [
            Vec2::new(x0, bottom0),
            Vec2::new(x1, bottom1),
            Vec2::new(x1, top),
            Vec2::new(x0, top),
        ],
        front,
        back,
        transverse,
    );
}
fn voussoirs(
    meshes: &mut [Mesh],
    radius: f32,
    spring: f32,
    front: f32,
    back: f32,
    transverse: bool,
) {
    for i in 0..48 {
        let a = (i as f32 + 0.045) * std::f32::consts::PI / 48.0;
        let b = (i as f32 + 0.955) * std::f32::consts::PI / 48.0;
        let p = |t: f32, r: f32| Vec2::new(t.cos() * r, spring + t.sin() * r);
        prism(
            &mut meshes[i % 3],
            [
                p(a, radius + 0.85),
                p(b, radius + 0.85),
                p(b, radius),
                p(a, radius),
            ],
            front,
            back,
            transverse,
        );
    }
}

pub fn camera(t: f32, aspect: f32) -> Camera {
    let angle = 0.30 + t * 0.28;
    let distance = 66.0 - 12.0 * t;
    let camera = Camera::perspective_look_at(
        Vec3::new(
            angle.sin() * distance,
            14.0 + 3.0 * t,
            angle.cos() * distance,
        ),
        Vec3::new(0.0, 24.0, 0.0),
        Vec3::Y,
        0.85,
        aspect,
        0.1,
        350.0,
    );
    camera
}

pub fn populate(world: &mut World) -> (Vec<Entity>, Vec<Entity>) {
    let mut meshes: Vec<Mesh> = (0..7).map(|_| Mesh::default()).collect();
    // Four piers preserve both intersecting passages down to the plaza.
    for side in [-1.0, 1.0] {
        for front in [-1.0, 1.0] {
            // Recessed mortar closes the joints without bridging either vault.
            // Open cracks through the entire pier alias into black dashed lines.
            meshes[3].block([side * 14.86, 7.23, front * 7.66], [7.525, 7.205, 3.410]);
            for course in 0..19 {
                for column in 0..8 {
                    let x = 7.31 + (column as f32 + 0.5) * 15.10 / 8.0;
                    let y = (course as f32 + 0.5) * 14.46 / 19.0;
                    meshes[(course * 7 + column * 3) % 3].block(
                        [side * x, y, front * 7.66],
                        [15.10 / 16.0 - 0.012, 14.46 / 38.0 - 0.012, 3.435],
                    );
                }
            }
        }
        // Transverse barrel vault above the narrow opening, filled to the
        // grand arch's spring line. Actual voids participate in raster and RT.
        for i in 0..64 {
            let a = -4.22 + 8.44 * i as f32 / 64.0;
            let b = -4.22 + 8.44 * (i + 1) as f32 / 64.0;
            let y = |z: f32| 14.46 + (4.22f32.powi(2) - z * z).max(0.0).sqrt();
            let (lo, hi) = if side > 0.0 {
                (7.31, 22.41)
            } else {
                (-22.41, -7.31)
            };
            wedge(&mut meshes[0], a, b, y(a), y(b), 21.88, hi, lo, true);
        }
        for y in [15.0, 16.3, 17.6, 18.9, 20.2, 21.5] {
            for front in [-1.0, 1.0] {
                meshes[1].block([side * 14.86, y, front * 8.11], [7.55, 0.64, 2.99]);
            }
        }
        meshes[0].block([side * 14.86, 27.69, 0.0], [7.55, 5.81, 11.10]);
    }
    // Grand vault: continuous curved intrados with separate radial stones.
    for i in 0..96 {
        let a = -7.31 + 14.62 * i as f32 / 96.0;
        let b = -7.31 + 14.62 * (i + 1) as f32 / 96.0;
        let y = |x: f32| 21.88 + (7.31f32.powi(2) - x * x).max(0.0).sqrt();
        wedge(&mut meshes[0], a, b, y(a), y(b), 33.5, 11.10, -11.10, false);
    }
    meshes[0].block([0.0, 40.35, 0.0], [22.41, 6.85, 11.10]);
    for (y, h, extent) in [
        (33.6, 0.32, 22.9),
        (35.0, 0.25, 23.1),
        (46.9, 0.30, 23.2),
        (48.0, 0.26, 23.45),
        (49.15, 0.35, 23.15),
    ] {
        meshes[1].block([0.0, y, 0.0], [extent, h, 11.65]);
    }
    for front in [-1.0, 1.0] {
        voussoirs(
            &mut meshes,
            7.31,
            21.88,
            front * 11.36 + 0.16,
            front * 11.36 - 0.16,
            false,
        );
        // Dentils, frieze panels and inset attic tablets.
        for i in -22..=22 {
            meshes[2].block([i as f32, 34.4, front * 11.48], [0.28, 0.32, 0.32]);
        }
        for i in -10..=10 {
            let x = i as f32 * 2.05;
            meshes[2].block([x, 42.0, front * 11.16], [0.90, 3.7, 0.10]);
            for y in [39.0, 39.6, 40.2, 40.8, 41.4, 42.0, 42.6, 43.2, 43.8, 44.4] {
                // Shallow carved inscription strokes, original abstract ornament.
                for j in 0..7 {
                    meshes[3].block(
                        [x - 0.65 + j as f32 * 0.21, y, front * 11.285],
                        [0.064, 0.032, 0.012],
                    );
                }
            }
        }
        // Pilasters and sculptural shield/trophy reliefs on the four facades.
        for side in [-1.0, 1.0] {
            for x in [8.25, 21.6] {
                meshes[1].block([side * x, 16.2, front * 11.24], [0.23, 15.7, 0.18]);
            }
            let c = Vec3::new(side * 14.8, 11.0, front * 11.45);
            meshes[1].block([c.x, 10.7, c.z], [4.6, 7.4, 0.15]);
            for radius in [2.0, 2.35, 2.6] {
                meshes[2].ring(c + Vec3::Y * 1.2, Vec3::X, Vec3::Y, radius, 0.12);
            }
            for i in 0..12 {
                let a = i as f32 * std::f32::consts::TAU / 12.0;
                let p = c + Vec3::new(a.cos() * 3.4, a.sin() * 3.4, front * 0.3);
                meshes[2].rod(c, p, 0.16, 10);
            }
            for dx in [-2.5, -1.2, 0.0, 1.2, 2.5] {
                meshes[2].rod(
                    c + Vec3::new(dx, -5.2, front * 0.22),
                    c + Vec3::new(dx * 0.5, 4.3, front * 0.22),
                    0.11,
                    8,
                );
            }
            meshes[1].block([c.x, 2.2, c.z], [4.8, 0.3, 0.7]);
        }
    }
    for side in [-1.0, 1.0] {
        voussoirs(
            &mut meshes,
            4.22,
            14.46,
            side * 22.58 + 0.16,
            side * 22.58 - 0.16,
            true,
        );
    }
    // Coffers run along the actual grand-vault intrados.
    for bay in 0..11 {
        let z = -10.0 + bay as f32 * 2.0;
        for i in 1..15 {
            let a = i as f32 * std::f32::consts::PI / 16.0;
            let c = Vec3::new(a.cos() * 7.22, 21.88 + a.sin() * 7.22, z);
            let tangent = Vec3::new(-a.sin(), a.cos(), 0.0);
            meshes[2].rod(
                c - tangent * 0.40 - Vec3::Z * 0.65,
                c + tangent * 0.40 - Vec3::Z * 0.65,
                0.065,
                6,
            );
            meshes[2].rod(
                c - tangent * 0.40 + Vec3::Z * 0.65,
                c + tangent * 0.40 + Vec3::Z * 0.65,
                0.065,
                6,
            );
            for sign in [-1.0, 1.0] {
                meshes[2].rod(
                    c + tangent * 0.4 * sign - Vec3::Z * 0.65,
                    c + tangent * 0.4 * sign + Vec3::Z * 0.65,
                    0.065,
                    6,
                );
            }
        }
    }
    // Plaza joints and bronze bollards establish metre scale around the monument.
    meshes[4].block([0.0, -0.21, 0.0], [100.0, 0.2, 100.0]);
    for x in -30i32..30 {
        for z in -30i32..30 {
            meshes[4 + (x + z).rem_euclid(2) as usize].block(
                [x as f32 * 2.4 + 1.2, -0.012, z as f32 * 2.4 + 1.2],
                [1.188, 0.012, 1.188],
            );
        }
    }
    for i in 0..32 {
        let a = i as f32 * std::f32::consts::TAU / 32.0;
        let c = Vec3::new(a.cos() * 35.0, 0.0, a.sin() * 35.0);
        meshes[6].rod(c, c + Vec3::Y * 1.1, 0.11, 12);
    }
    let materials = [
        ([0.62, 0.57, 0.47, 1.0], 0.86, 0.0),
        ([0.72, 0.66, 0.54, 1.0], 0.80, 0.0),
        ([0.55, 0.51, 0.43, 1.0], 0.9, 0.0),
        ([0.31, 0.29, 0.25, 1.0], 0.95, 0.0),
        ([0.22, 0.24, 0.25, 1.0], 0.68, 0.0),
        ([0.30, 0.31, 0.30, 1.0], 0.63, 0.0),
        ([0.30, 0.21, 0.10, 1.0], 0.23, 0.85),
    ];
    let triangles: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    for (mesh, (color, roughness, metallic)) in meshes.into_iter().zip(materials) {
        let material = spawn_material(
            world,
            make_material(color, roughness, metallic, [0.0; 3], 0.0),
        );
        let geometry = spawn_mesh(
            world,
            MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
        );
        spawn_object(world, geometry, material, Mat4::IDENTITY, 160.0).expect("monument geometry");
    }
    let mut sun = directional_light([-0.5, -0.75, -0.4], [1.0, 0.86, 0.66], 3.5);
    sun.god_rays_enabled = 1;
    sun.god_rays_weight = 1.0;
    sun.god_rays_exposure = 1.0;
    sun.god_rays_density = 1.0;
    spawn_light(world, sun);
    let mut smoke_rim = point_light([3.0, 22.0, 9.0], [1.0, 0.72, 0.42], 650.0, 30.0);
    smoke_rim.god_rays_enabled = 1;
    smoke_rim.god_rays_weight = 1.0;
    smoke_rim.god_rays_exposure = 1.0;
    smoke_rim.god_rays_density = 1.0;
    spawn_light(world, smoke_rim);
    for x in [-18.0, 18.0] {
        for z in [-15.0, 15.0] {
            spawn_light(
                world,
                point_light([x, 0.8, z], [1.0, 0.72, 0.42], 100.0, 22.0),
            );
        }
    }

    eprintln!(
        "Monumental arch: {triangles} triangles, 7 material batches, intersecting open vaults"
    );
    (Vec::new(), Vec::new())
}
