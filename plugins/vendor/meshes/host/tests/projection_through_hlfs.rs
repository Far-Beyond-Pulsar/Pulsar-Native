//! The meshes' ray-tracing projection ([`SceneDbRayTracing`]) observed
//! through a ray-traced consumer: HLFS's reference shading, on HLFS's own
//! test fixture (the lights' crate, in the Helio submodule; read by path so
//! the fixture has one copy). Edits, moves, removal, stale frames,
//! transmission rows and BLAS reuse all show in its shadow.
#![allow(dead_code)]
#[path = "../../../lights/feature/hlfs/tests/support/mod.rs"]
mod support;
use helio_mesh_host::ray_tracing::{RayTransmission, SceneDbRayTracing};
use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode, HlfsMode};
use support::{mean, point, Fixture};

fn light() -> helio_light_api::GpuLight {
    let mut light = point([10.0, 0.0, 2.0], [1.0, 1.0, 1.0], 10000.0);
    light.position_range[3] = 100.0;
    light.shadow_index = u32::MAX;
    light.set_ray_traced_shadows(true);
    light
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn scenedb_projection_tracks_mesh_edits_transforms_removal_and_stale_frames() {
    pollster::block_on(async {
        use helio_components::MaterialComponent;
        use helio_mesh_host::{MeshComponent, StaticObjectComponent};
        use std::sync::Arc;
        let mut f = Fixture::new_rt(65, 49).await;
        f.publish_ray_frame = false;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.lights(vec![light()]);
        let mut db = pulsar_scenedb::SceneDb::new();
        let context = pulsar_scenedb::gpu::EngineGpuContext::new(f.device.clone(), f.queue.clone());
        let mut store = pulsar_scenedb::gpu::SceneGpuStore::new(
            &context,
            pulsar_scenedb::gpu::SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        MeshComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        MaterialComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        StaticObjectComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        db.world
            .attach_gpu_mirror(pulsar_scenedb::gpu::GpuMirrorHandle::new(
                Arc::new(store),
                f.queue.clone(),
            ));
        let mut acceleration = SceneDbRayTracing::new(f.device.clone(), f.queue.clone());
        let render = |f: &mut Fixture,
                      acceleration: &mut SceneDbRayTracing,
                      db: &pulsar_scenedb::SceneDb| {
            db.world.flush_gpu_mirror(&f.queue);
            acceleration.prepare(&db.world).unwrap();
            f.rays = acceleration.scene_rays();
            f.frame();
            mean(&f.read())
        };
        let clear = render(&mut f, &mut acceleration, &db);
        assert!(
            f.try_frame().is_err(),
            "last frame's acceleration input must expire"
        );
        let mesh = db.world.spawn();
        db.world.insert(
            mesh,
            MeshComponent {
                vertices: [
                    [5.0, -100.0, -100.0],
                    [5.0, 100.0, -100.0],
                    [5.0, 0.0, 100.0],
                ]
                .map(|position| helio_mesh_api::PackedVertex {
                    position,
                    ..Default::default()
                })
                .to_vec(),
                indices: vec![0, 1, 2],
            },
        );
        let material = db.world.spawn();
        db.world.insert(
            material,
            MaterialComponent::from(helio_mats::MaterialRow::new(
                [1.0; 4], 0.5, 0.0, [0.0; 3], 0.0,
            )),
        );
        let mirror = db.world.gpu_mirror().unwrap();
        let vertices = MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index()).unwrap();
        let indices = MeshComponent::indices_gpu_handle(mirror.store(), mesh.index()).unwrap();
        let object = db.world.spawn();
        db.world.insert(
            object,
            StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
                mesh.index(),
                mesh.generation() + 1,
                material.index(),
                material.generation() + 1,
                glam::Mat4::IDENTITY,
                [0.0, 0.0, 0.0, 200.0],
                indices.count,
                indices.offset,
                vertices.offset as i32,
                0,
                0,
                helio_mesh_api::INSTANCE_FLAG_CASTS_SHADOW,
            )),
        );
        let shadow = render(&mut f, &mut acceleration, &db);
        assert!(
            shadow < clear * 0.2,
            "SceneDB offscreen caster missing: {shadow}/{clear}"
        );
        db.world
            .get_mut::<MaterialComponent>(material)
            .unwrap()
            .flags |= helio_mats::FLAG_ALPHA_BLEND;
        db.world.insert(material, RayTransmission([1.0; 3]));
        assert!((render(&mut f, &mut acceleration, &db) / clear - 1.0).abs() < 0.01);
        // One mesh may simultaneously use both acceleration opacity classes.
        let opaque_material = db.world.spawn();
        db.world.insert(
            opaque_material,
            MaterialComponent::from(helio_mats::MaterialRow::new(
                [1.0; 4], 0.5, 0.0, [0.0; 3], 0.0,
            )),
        );
        let mut opaque_row = *db.world.get::<StaticObjectComponent>(object).unwrap();
        opaque_row.material_slot = opaque_material.index();
        opaque_row.material_generation = opaque_material.generation() + 1;
        let opaque_object = db.world.spawn();
        db.world.insert(opaque_object, opaque_row);
        assert!(
            render(&mut f, &mut acceleration, &db) < clear * 0.01,
            "opaque mesh variant must block through clear glass"
        );
        db.world.despawn(opaque_object);
        assert!(
            (render(&mut f, &mut acceleration, &db) / clear - 1.0).abs() < 0.01,
            "opaque variant removal must preserve the glass variant"
        );
        db.world.insert(material, RayTransmission([0.25; 3]));
        assert!((render(&mut f, &mut acceleration, &db) / clear - 0.25).abs() < 0.015);
        db.world.insert(material, RayTransmission([f32::NAN; 3]));
        db.world.flush_gpu_mirror(&f.queue);
        assert!(acceleration.prepare(&db.world).is_err());
        assert!(acceleration.transmission().is_none());
        db.world.remove::<RayTransmission>(material);
        db.world
            .get_mut::<MaterialComponent>(material)
            .unwrap()
            .flags &= !helio_mats::FLAG_ALPHA_BLEND;
        assert!(render(&mut f, &mut acceleration, &db) < clear * 0.2);
        {
            let mut mesh = db.world.get_mut::<MeshComponent>(mesh).unwrap();
            for vertex in &mut mesh.vertices {
                vertex.position[0] += 20.0;
            }
        }
        let edited = render(&mut f, &mut acceleration, &db);
        assert!(
            (edited - clear).abs() < clear * 0.01,
            "in-place mesh edit retained stale BLAS"
        );
        {
            let mut row = db.world.get_mut::<StaticObjectComponent>(object).unwrap();
            *row = row
                .with_transform(
                    glam::Mat4::from_translation(glam::Vec3::new(-20.0, 0.0, 0.0)),
                    [0.0, 0.0, 0.0, 200.0],
                )
                .into();
        }
        assert!(
            render(&mut f, &mut acceleration, &db) < clear * 0.2,
            "transformed caster missing"
        );
        db.world
            .get_mut::<MaterialComponent>(material)
            .unwrap()
            .flags |= helio_mats::FLAG_ALPHA_TEST;
        db.world.flush_gpu_mirror(&f.queue);
        assert!(
            acceleration.prepare(&db.world).is_err(),
            "unsupported alpha caster must fail closed"
        );
        db.world.despawn(object);
        let removed = render(&mut f, &mut acceleration, &db);
        assert!(
            (removed - clear).abs() < clear * 0.01,
            "removed caster remained in TLAS"
        );
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn non_deforming_meshes_reuse_their_blas_until_marked_dynamic() {
    pollster::block_on(async {
        use helio_components::MaterialComponent;
        use helio_core::Movability;
        use helio_mesh_host::{MeshComponent, StaticObjectComponent};
        use std::sync::Arc;
        let mut f = Fixture::new_rt(65, 49).await;
        f.publish_ray_frame = false;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.lights(vec![light()]);
        let mut db = pulsar_scenedb::SceneDb::new();
        let context = pulsar_scenedb::gpu::EngineGpuContext::new(f.device.clone(), f.queue.clone());
        let mut store = pulsar_scenedb::gpu::SceneGpuStore::new(
            &context,
            pulsar_scenedb::gpu::SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        MeshComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        MaterialComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        StaticObjectComponent::register_gpu_columns_growable(&mut store, 8, &f.device);
        db.world
            .attach_gpu_mirror(pulsar_scenedb::gpu::GpuMirrorHandle::new(
                Arc::new(store),
                f.queue.clone(),
            ));
        let mut acceleration = SceneDbRayTracing::new(f.device.clone(), f.queue.clone());
        let render = |f: &mut Fixture,
                      acceleration: &mut SceneDbRayTracing,
                      db: &pulsar_scenedb::SceneDb| {
            db.world.flush_gpu_mirror(&f.queue);
            acceleration.prepare(&db.world).unwrap();
            f.rays = acceleration.scene_rays();
            f.frame();
            mean(&f.read())
        };
        let clear = render(&mut f, &mut acceleration, &db);
        let mesh = db.world.spawn();
        db.world.insert(
            mesh,
            MeshComponent {
                vertices: [
                    [5.0, -100.0, -100.0],
                    [5.0, 100.0, -100.0],
                    [5.0, 0.0, 100.0],
                ]
                .map(|position| helio_mesh_api::PackedVertex {
                    position,
                    ..Default::default()
                })
                .to_vec(),
                indices: vec![0, 1, 2],
            },
        );
        db.world.insert(mesh, Movability::Static);
        let material = db.world.spawn();
        db.world.insert(
            material,
            MaterialComponent::from(helio_mats::MaterialRow::new(
                [1.0; 4], 0.5, 0.0, [0.0; 3], 0.0,
            )),
        );
        let mirror = db.world.gpu_mirror().unwrap();
        let vertices = MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index()).unwrap();
        let indices = MeshComponent::indices_gpu_handle(mirror.store(), mesh.index()).unwrap();
        let object = db.world.spawn();
        db.world.insert(
            object,
            StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
                mesh.index(),
                mesh.generation() + 1,
                material.index(),
                material.generation() + 1,
                glam::Mat4::IDENTITY,
                [0.0, 0.0, 0.0, 200.0],
                indices.count,
                indices.offset,
                vertices.offset as i32,
                0,
                0,
                helio_mesh_api::INSTANCE_FLAG_CASTS_SHADOW,
            )),
        );
        assert!(
            render(&mut f, &mut acceleration, &db) < clear * 0.2,
            "static caster missing"
        );
        // Breaking the Static promise is not detected: the cached BLAS is kept.
        {
            let mut mesh = db.world.get_mut::<MeshComponent>(mesh).unwrap();
            for vertex in &mut mesh.vertices {
                vertex.position[0] += 20.0;
            }
        }
        assert!(
            render(&mut f, &mut acceleration, &db) < clear * 0.2,
            "static mesh must reuse its BLAS without re-reading vertices"
        );
        // Declaring the mesh Dynamic restores content-based invalidation.
        db.world.insert(mesh, Movability::Dynamic);
        let edited = render(&mut f, &mut acceleration, &db);
        assert!(
            (edited - clear).abs() < clear * 0.01,
            "dynamic mesh edit retained stale BLAS"
        );
    });
}
