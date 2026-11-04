//! Dense local-light laboratory for glossy surfaces and reflection validation.
//! Its light camera fog left Helio with the fog plugin (Pulsar
//! `plugins/vendor/fog`); this scene no longer has any.
use crate::{architectural_mesh::Mesh, v3_demo_common::*};
use glam::{Mat4, Vec3};
use helio::{Camera};
use helio_mesh_api::{MeshUpload};
use pulsar_scenedb::{Entity, World};

pub fn camera(t: f32, aspect: f32) -> Camera {
    let camera = Camera::perspective_look_at(
        Vec3::new(2.0 * t, 3.0, 25.0 - 12.0 * t),
        Vec3::new(0.0, 3.0, -18.0),
        Vec3::Y,
        0.95,
        aspect,
        0.1,
        180.0,
    );
    camera
}

pub fn populate(world: &mut World) -> (Vec<Entity>, Vec<Entity>) {
    let mut meshes: Vec<Mesh> = (0..8).map(|_| Mesh::default()).collect();
    // Dark polished floor, architectural shell, and segmented ceiling.
    meshes[0].block([0.0, -0.15, 0.0], [24.0, 0.15, 34.0]);
    meshes[1].block([0.0, 5.0, -34.0], [24.0, 5.0, 0.25]);
    for side in [-1.0, 1.0] {
        meshes[1].block([side * 24.0, 5.0, 0.0], [0.25, 5.0, 34.0]);
        for bay in 0..12 {
            let z = -30.0 + bay as f32 * 5.5;
            meshes[2].block([side * 18.0, 3.0, z], [0.35, 3.0, 0.35]);
            meshes[2].block([side * 20.5, 1.4, z], [2.0, 1.4, 1.6]);
            meshes[3].block([side * 20.5, 2.85, z], [1.8, 0.07, 1.35]);
            for k in 0..5 {
                meshes[4 + (bay + k) % 3].block(
                    [side * 18.45, 0.4 + k as f32 * 0.45, z],
                    [0.045, 0.055, 1.1],
                );
            }
        }
    }
    for row in 0..16 {
        let z = -31.0 + row as f32 * 4.0;
        meshes[1].block([0.0, 9.5, z], [24.0, 0.18, 0.65]);
        for side in [-1.0, 1.0] {
            meshes[4 + row % 3].block([side * 8.0, 0.018, z], [0.06, 0.018, 1.7]);
        }
    }
    // Smooth metallic specimens at several roughnesses, on display plinths.
    for row in 0..6 {
        let z = -24.0 + row as f32 * 8.0;
        for side in [-1.0, 1.0] {
            let center = Vec3::new(side * 9.5, 0.6, z);
            meshes[1].block(center.to_array(), [2.0, 0.6, 2.0]);
            meshes[if row % 2 == 0 { 3 } else { 7 }].smooth_rod(
                center + Vec3::Y * 0.6,
                center + Vec3::Y * 3.5,
                1.35,
                96,
            );
            meshes[4 + row % 3].ring(center + Vec3::Y * 0.65, Vec3::X, Vec3::Z, 1.65, 0.045);
        }
    }
    let colors = [[0.04, 0.6, 1.0], [1.0, 0.08, 0.3], [0.5, 0.16, 1.0]];
    // 32x32 independently shadowed sources, with matching emissive fixtures.
    for z in 0..32 {
        for x in 0..32 {
            let p = [
                -22.0 + x as f32 * 44.0 / 31.0,
                8.8,
                -31.0 + z as f32 * 62.0 / 31.0,
            ];
            let group = (x / 8 + z / 8) % 3;
            meshes[4 + group].block(p, [0.28, 0.05, 0.45]);
            spawn_light(
                world,
                point_light([p[0], p[1] - 0.12, p[2]], colors[group], 30.0, 14.0),
            );
        }
    }
    let specs = [
        ([0.12, 0.15, 0.18, 1.0], 0.10, 0.85, [0.0; 3], 0.0),
        ([0.11, 0.13, 0.17, 1.0], 0.65, 0.20, [0.0; 3], 0.0),
        ([0.25, 0.29, 0.34, 1.0], 0.24, 0.90, [0.0; 3], 0.0),
        ([0.78, 0.82, 0.86, 1.0], 0.08, 1.0, [0.0; 3], 0.0),
        ([0.04, 0.6, 1.0, 1.0], 0.3, 0.0, colors[0], 4.0),
        ([1.0, 0.08, 0.3, 1.0], 0.3, 0.0, colors[1], 4.0),
        ([0.5, 0.16, 1.0, 1.0], 0.3, 0.0, colors[2], 4.0),
        ([0.8, 0.45, 0.12, 1.0], 0.32, 1.0, [0.0; 3], 0.0),
    ];
    let triangles: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    for (mesh, (color, roughness, metallic, emissive, power)) in meshes.into_iter().zip(specs) {
        let mat = spawn_material(
            world,
            make_material(color, roughness, metallic, emissive, power),
        );
        let geometry = spawn_mesh(
            world,
            MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
        );
        spawn_object(world, geometry, mat, Mat4::IDENTITY, 60.0).expect("technology geometry");
    }
    eprintln!("Technology gallery: 1024 lights, {triangles} triangles, 8 material batches");
    (Vec::new(), Vec::new())
}
