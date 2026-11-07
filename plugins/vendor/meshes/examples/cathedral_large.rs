//! Cathedral-scale procedural interior. Its overall dimensions are inspired by
//! the published Cologne Cathedral measurements (145 m x 45 m x 43 m), but
//! the bay layout and ornament are illustrative rather than a reconstruction.
use crate::architectural_mesh::Mesh;
use crate::v3_demo_common::{make_material, spawn_material, spawn_mesh, spawn_object};
use glam::{Mat4, Vec3};
use helio_mesh_api::MeshUpload;
use pulsar_scenedb::World;

const HALF_LENGTH: f32 = 72.0;
const HALF_WIDTH: f32 = 22.5;
const BAY: f32 = 8.0;

pub fn populate(world: &mut World) {
    // Stone, carved stone, basalt, paving, oak, bronze, wax, flame, altar.
    let properties = [
        ([0.53, 0.47, 0.37, 1.], 0.88, 0., [0.; 3], 0.),
        ([0.72, 0.65, 0.51, 1.], 0.72, 0., [0.; 3], 0.),
        ([0.11, 0.12, 0.13, 1.], 0.75, 0., [0.; 3], 0.),
        ([0.55, 0.54, 0.50, 1.], 0.62, 0., [0.; 3], 0.),
        ([0.14, 0.062, 0.027, 1.], 0.53, 0., [0.; 3], 0.),
        ([0.42, 0.25, 0.075, 1.], 0.24, 0.8, [0.; 3], 0.),
        ([0.86, 0.74, 0.50, 1.], 0.65, 0., [0.; 3], 0.),
        ([1.0, 0.6, 0.15, 1.], 0.5, 0., [1.0, 0.36, 0.045], 5.),
        ([0.51, 0.49, 0.42, 1.], 0.30, 0., [0.; 3], 0.),
    ];
    let mut meshes: Vec<Mesh> = (0..properties.len()).map(|_| Mesh::default()).collect();

    // A single structural slab underlays human-scale, individually bounded
    // pavers. The dark procession band is geometry, so RT sees its edges.
    // Keep the slab below the paver faces. Coplanar tops cause unstable depth
    // ownership and dashed patches on the floor during camera movement.
    meshes[0].block([0., -0.18, 0.], [HALF_WIDTH, 0.16, HALF_LENGTH]);
    for x in -18_i32..18 {
        for z in -60_i32..60 {
            let material = if x.abs() <= 1 && z.rem_euclid(9) == 0 {
                2
            } else {
                3
            };
            meshes[material].block(
                [x as f32 * 1.2 + 0.6, -0.014, z as f32 * 1.2 + 0.6],
                [0.592, 0.014, 0.592],
            );
        }
    }
    for x in [-2.4, 2.4] {
        meshes[2].block([x, 0.012, 0.], [0.09, 0.012, HALF_LENGTH]);
    }

    // Exterior aisle walls, nave clerestory and their open window bays.
    for side in [-1.0_f32, 1.0] {
        let outside = side * HALF_WIDTH;
        let arcade = side * 9.5;
        meshes[0].block([outside, 4., 0.], [0.45, 4., HALF_LENGTH]);
        meshes[0].block([outside, 21., 0.], [0.45, 1.5, HALF_LENGTH]);
        meshes[0].block([arcade, 25.5, 0.], [0.35, 3.5, HALF_LENGTH]);
        meshes[0].block([arcade, 41., 0.], [0.35, 2., HALF_LENGTH]);
        // Aisle vault cap remains below the clerestory window sill.
        meshes[0].block([side * 16., 22.3, 0.], [6.3, 0.3, HALF_LENGTH]);
        for i in -9..=9 {
            let z = i as f32 * BAY;
            meshes[0].block([outside, 14.5, z], [0.45, 6.5, 1.18]);
            meshes[0].block([arcade, 34., z], [0.35, 5., 1.0]);
            meshes[1].block([outside - side * 0.42, 15., z], [0.20, 7., 0.30]);
            for y in [8., 19., 22., 29., 39.] {
                let x = if y < 23. {
                    outside - side * 0.35
                } else {
                    arcade - side * 0.33
                };
                meshes[1].block([x, y, z], [0.18, 0.11, 3.9]);
            }
        }
        for i in -8..=8 {
            let z = i as f32 * BAY;
            let p = Vec3::new(arcade, 0., z);
            meshes[1].smooth_rod(p, p + Vec3::Y * 29., 0.78, 48);
            for j in 0..8 {
                let a = j as f32 * std::f32::consts::TAU / 8.;
                let offset = Vec3::new(a.cos() * 0.69, 0., a.sin() * 0.69);
                meshes[1].smooth_rod(
                    p + offset + Vec3::Y * 0.8,
                    p + offset + Vec3::Y * 26.5,
                    0.17,
                    20,
                );
            }
            for (y, radius, height) in [
                (0.22, 1.18, 0.22),
                (0.6, 0.98, 0.12),
                (26.6, 0.93, 0.15),
                (27., 1.16, 0.22),
            ] {
                meshes[1].smooth_rod(
                    p + Vec3::Y * (y - height),
                    p + Vec3::Y * (y + height),
                    radius,
                    32,
                );
            }
            if side < 0. {
                meshes[1].smooth_arch(Vec3::new(-9.5, 28., z), Vec3::new(9.5, 28., z), 13.1, 0.30);
            }
            meshes[1].smooth_arch(
                Vec3::new(arcade, 18., z),
                Vec3::new(outside - side * 0.45, 18., z),
                5.0,
                0.20,
            );
        }
        for i in -8..8 {
            let z = i as f32 * BAY;
            meshes[1].smooth_arch(
                Vec3::new(arcade, 23., z),
                Vec3::new(arcade, 23., z + BAY),
                7.0,
                0.25,
            );
            // Crossing diagonals distinguish every ribbed vault bay.
            meshes[1].smooth_arch(
                Vec3::new(arcade, 28., z),
                Vec3::new(-arcade, 28., z + BAY),
                13.1,
                0.16,
            );
        }
    }

    // Curved stone web behind the ribs; the underside faces the nave.
    for i in 0..64 {
        let x0 = -9.6 + 19.2 * i as f32 / 64.;
        let x1 = -9.6 + 19.2 * (i + 1) as f32 / 64.;
        let height = |x: f32| 38.8 + 4.0 * (1.0 - (x / 9.6).abs().powf(1.6));
        meshes[0].quad(
            Vec3::new(x0, height(x0), -HALF_LENGTH),
            Vec3::new(x1, height(x1), -HALF_LENGTH),
            Vec3::new(x1, height(x1), HALF_LENGTH),
            Vec3::new(x0, height(x0), HALF_LENGTH),
        );
    }
    meshes[1].rod(
        Vec3::new(0., 42.5, -HALF_LENGTH),
        Vec3::new(0., 42.5, HALF_LENGTH),
        0.24,
        12,
    );

    // End walls leave true square apertures around two eight-metre roses.
    for z in [-HALF_LENGTH, HALF_LENGTH] {
        meshes[0].block([0., 12., z], [HALF_WIDTH, 12., 0.45]);
        meshes[0].block([0., 41.5, z], [HALF_WIDTH, 1.5, 0.45]);
        for x in [-15.25, 15.25] {
            meshes[0].block([x, 32., z], [7.25, 8., 0.45]);
        }
    }
    // Raised sanctuary and a human-scale altar beneath the eastern rose.
    for step in 0..3 {
        meshes[8].block(
            [0., 0.12 + step as f32 * 0.18, -65.0 - step as f32 * 0.35],
            [7.0 - step as f32 * 0.4, 0.12, 3.5 - step as f32 * 0.25],
        );
    }
    meshes[8].block([0., 1.65, -66.], [2.4, 0.17, 0.9]);
    for x in [-1.8, 1.8] {
        meshes[1].block([x, 1.05, -66.], [0.28, 0.6, 0.6]);
    }
    meshes[5].block([0., 4.6, -68.], [0.12, 2.4, 0.1]);
    meshes[5].block([0., 5.5, -68.], [1.2, 0.12, 0.1]);
    for x in [-5., -4., -3., 3., 4., 5.] {
        meshes[1].smooth_arch(
            Vec3::new(x - 0.42, 5.4, -70.),
            Vec3::new(x + 0.42, 5.4, -70.),
            1.8,
            0.10,
        );
    }

    // Furniture stays at human scale as the shell and bay count grow.
    for side in [-1.0_f32, 1.0] {
        for row in -15..15 {
            let x = side * 5.0;
            let z = row as f32 * 3.55;
            meshes[4].block([x, 0.66, z], [1.65, 0.08, 0.36]);
            meshes[4].block([x, 1.17, z + 0.32], [1.65, 0.43, 0.07]);
            meshes[4].block([x, 1.61, z + 0.32], [1.7, 0.06, 0.11]);
            meshes[4].block([x, 0.23, z - 0.57], [1.55, 0.08, 0.18]);
            for dx in [-1.58, 1.58] {
                meshes[4].block([x + dx, 0.72, z], [0.09, 0.72, 0.43]);
            }
            for dx in [-1.0, -0.33, 0.33, 1.0] {
                meshes[4].block([x + dx, 1.15, z + 0.25], [0.028, 0.32, 0.028]);
            }
        }
    }

    // Seven chandeliers make the far end legible without giant point lights.
    for &z in super::LARGE_CHANDELIER_Z {
        let center = Vec3::new(0., 31., z);
        meshes[5].rod(center, Vec3::new(0., 42.4, z), 0.06, 10);
        for radius in [1.7, 1.2] {
            meshes[5].smooth_ring(center, Vec3::X, Vec3::Z, radius, 0.06);
        }
        for i in 0..16 {
            let a = i as f32 * std::f32::consts::TAU / 16.;
            let p = center + Vec3::new(a.cos() * 1.7, 0., a.sin() * 1.7);
            meshes[5].rod(center + Vec3::Y * 1.5, p, 0.035, 6);
            meshes[6].rod(p, p + Vec3::Y * 0.42, 0.07, 8);
            meshes[7].rod(p + Vec3::Y * 0.42, p + Vec3::Y * 0.55, 0.03, 6);
        }
    }
    for &(x, y, z) in super::LARGE_CANDLES {
        for j in -1..=1 {
            let p = Vec3::new(x + j as f32 * 0.2, y - 0.3, z);
            meshes[5].rod(p - Vec3::Y * 0.6, p, 0.05, 8);
            meshes[6].rod(p, p + Vec3::Y * 0.28, 0.07, 10);
            meshes[7].rod(p + Vec3::Y * 0.28, p + Vec3::Y * 0.38, 0.03, 6);
        }
    }

    let colours = [
        [0.12, 0.32, 0.85],
        [0.8, 0.12, 0.08],
        [0.1, 0.55, 0.3],
        [0.8, 0.48, 0.08],
        [0.45, 0.12, 0.62],
        [0.14, 0.65, 0.8],
    ];
    let mut panes: Vec<Mesh> = (0..colours.len()).map(|_| Mesh::default()).collect();
    for side in [-1.0_f32, 1.0] {
        for bay in -8_i32..=8 {
            // Dividers are at integer BAY positions. Both rows of glazing,
            // mullions and pointed surrounds belong halfway between them.
            let z = (bay as f32 + 0.5) * BAY;
            for (x, y0, y1) in [(side * 22.08, 8.0, 19.0), (side * 9.12, 29.0, 39.0)] {
                let left = z - 2.8;
                let width = 5.6;
                for col in 0..4 {
                    for row in 0..8 {
                        let za = left + col as f32 * width / 4.;
                        let zb = za + width / 4.;
                        let ya = y0 + row as f32 * (y1 - y0) / 8.;
                        let yb = y0 + (row + 1) as f32 * (y1 - y0) / 8.;
                        let a = Vec3::new(x, ya, za);
                        let b = Vec3::new(x, yb, za);
                        let c = Vec3::new(x, yb, zb);
                        let d = Vec3::new(x, ya, zb);
                        panes[((bay + 8 + col + row) as usize) % 6].quad(a, b, c, d);
                        meshes[5].rod(a, c, 0.022, 5);
                    }
                }
                for col in 0..=4 {
                    let zz = left + col as f32 * width / 4.;
                    meshes[1].rod(Vec3::new(x, y0, zz), Vec3::new(x, y1, zz), 0.09, 8);
                }
                // Stone spandrels turn the rectangular construction bay into
                // a pointed lancet. Two faces make it opaque to exterior rays
                // as well as to the interior camera.
                let spring = y1 - 2.6;
                let mid = left + width * 0.5;
                for (face_x, inward) in [(x - side * 0.06, true), (x + side * 0.06, false)] {
                    let at = |y, z| Vec3::new(face_x, y, z);
                    let corners = [
                        [at(y1, left), at(spring, left), at(y1, mid)],
                        [at(y1, left + width), at(y1, mid), at(spring, left + width)],
                    ];
                    for [a, b, c] in corners {
                        if (side > 0.) == inward {
                            meshes[0].triangle(a, b, c);
                        } else {
                            meshes[0].triangle(c, b, a);
                        }
                    }
                }
                meshes[1].rod(Vec3::new(x, spring, left), Vec3::new(x, y1, mid), 0.15, 10);
                meshes[1].rod(
                    Vec3::new(x, y1, mid),
                    Vec3::new(x, spring, left + width),
                    0.15,
                    10,
                );
            }
        }
    }
    for z in [-71.55, 71.55] {
        let center = Vec3::new(0., 32., z);
        for radius in [1.2, 2.6, 4.2, 8.0] {
            meshes[1].smooth_ring(
                center,
                Vec3::X,
                Vec3::Y,
                radius,
                if radius > 7. { 0.30 } else { 0.14 },
            );
        }
        for i in 0..72 {
            let a = i as f32 * std::f32::consts::TAU / 72.;
            let b = (i + 1) as f32 * std::f32::consts::TAU / 72.;
            let u = Vec3::new(a.cos(), a.sin(), 0.);
            let v = Vec3::new(b.cos(), b.sin(), 0.);
            for (j, (r0, r1)) in [(0., 1.2), (1.2, 2.6), (2.6, 4.2), (4.2, 8.)]
                .into_iter()
                .enumerate()
            {
                // Short color runs and stone petal tracery avoid broad flat
                // sectors while preserving per-material RGB RT transmission.
                let colour = ((i / 2 + j * 3 + (i / 12) * 2) % 6) as usize;
                if r0 == 0. {
                    panes[colour].triangle(center, center + u * r1, center + v * r1);
                } else {
                    panes[colour].quad(
                        center + u * r0,
                        center + u * r1,
                        center + v * r1,
                        center + v * r0,
                    );
                }
                if i % 3 == 0 && r0 > 0. {
                    meshes[1].rod(center + u * r0, center + u * r1, 0.085, 8);
                }
            }
            let edge_u = u * (8.25 / u.x.abs().max(u.y.abs()));
            let edge_v = v * (8.25 / v.x.abs().max(v.y.abs()));
            meshes[0].quad(
                center + u * 8.,
                center + edge_u,
                center + edge_v,
                center + v * 8.,
            );
            meshes[0].quad(
                center + v * 8.,
                center + edge_v,
                center + edge_u,
                center + u * 8.,
            );
        }
        for petal in 0..12 {
            let angle = (petal as f32 + 0.5) * std::f32::consts::TAU / 12.;
            let offset = Vec3::new(angle.cos() * 5.9, angle.sin() * 5.9, 0.);
            meshes[1].smooth_ring(center + offset, Vec3::X, Vec3::Y, 1.25, 0.10);
        }
    }

    let triangles: usize = meshes
        .iter()
        .chain(&panes)
        .map(|m| m.indices.len() / 3)
        .sum();
    for (index, (mut mesh, (colour, rough, metal, emission, strength))) in
        meshes.into_iter().zip(properties).enumerate()
    {
        let material = spawn_material(
            world,
            make_material(colour, rough, metal, emission, strength),
        );
        match index {
            0 => {
                world.insert(
                    material,
                    crate::hlfs_capture::architectural_materials::StoneMaterial,
                );
                mesh.world_space_uv(2.0);
            }
            1 => {
                world.insert(
                    material,
                    crate::hlfs_capture::architectural_materials::CarvedStoneMaterial,
                );
                mesh.world_space_uv(1.5);
            }
            3 => {
                world.insert(
                    material,
                    crate::hlfs_capture::architectural_materials::FloorMaterial,
                );
                mesh.world_space_uv(2.0);
            }
            4 => {
                world.insert(
                    material,
                    crate::hlfs_capture::architectural_materials::WoodMaterial,
                );
                mesh.world_space_uv(0.8);
            }
            _ => {}
        }
        let mesh = spawn_mesh(
            world,
            MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
        );
        spawn_object(world, mesh, material, Mat4::IDENTITY, 160.)
            .expect("cathedral-scale architecture");
    }
    let glass_alpha = std::env::var("HLFS_GLASS_ALPHA")
        .map(|value| {
            value
                .parse::<f32>()
                .expect("HLFS_GLASS_ALPHA must be a number")
        })
        .unwrap_or(0.65);
    assert!((0.0..=1.0).contains(&glass_alpha));
    for (mesh, colour) in panes.into_iter().zip(colours) {
        let mut material = make_material(
            [colour[0], colour[1], colour[2], glass_alpha],
            0.12,
            0.,
            colour,
            0.8,
        );
        material.flags |= helio_mats::FLAG_ALPHA_BLEND | helio_mats::FLAG_TRANSPARENT_ONLY;
        let material = spawn_material(world, material);
        let transmission = if std::env::var_os("HLFS_CLEAR_GLASS").is_some() {
            [1.0; 3]
        } else {
            colour.map(|c| 0.04 + 0.76 * c)
        };
        world.insert(
            material,
            helio_mesh_host::ray_tracing::RayTransmission(transmission),
        );
        let mesh = spawn_mesh(
            world,
            MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
        );
        spawn_object(world, mesh, material, Mat4::IDENTITY, 160.).expect("cathedral-scale glazing");
    }
    eprintln!("Cathedral-scale interior: {triangles} triangles, 145 x 45 x 43 m shell");
}
