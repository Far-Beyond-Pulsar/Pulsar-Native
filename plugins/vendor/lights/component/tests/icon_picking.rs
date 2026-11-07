//! A light object picks by its editor icon (the lights' `ViewIconRegistration`):
//! the billboard's visible extent at any distance, hidden objects never, and
//! a nearer object occludes it. (Moved from engine_backend's interaction tests
//! with the icon: picking names no kind.)

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility};
use engine_backend::subsystems::render::helio_renderer::SceneInteraction;
use glam::{DVec3, Vec2, Vec3};
use lights_component::LightComponent;
use pulsar_scenedb::{Entity, World};

/// A test kind's content: a unit box on objects carrying it (what a mesh
/// would offer picking through its own registration).
struct TestBox;

inventory::submit! {
    engine_backend::subsystems::render::helio_renderer::ContentGeometryRegistration {
        origin: pulsar_plugins::origin!(),
        local_bounds: |world, object| {
            world
                .get::<TestBox>(object)
                .map(|_| (Vec3::splat(-0.5), Vec3::splat(0.5)))
        },
        hit: |_, _, _, _| None,
        triangles: |_, _| {},
    }
}

fn spawn_light(world: &mut World, position: [f64; 3], scale: [f32; 3]) -> Entity {
    let owner = world
        .spawn_object(
            SpawnObject::new("light")
                .with_id("light")
                .with_transform(Transform {
                    position,
                    scale,
                    ..Default::default()
                }),
        )
        .unwrap();
    pulsar_world_registry::attach_value(world, owner, LightComponent::default()).unwrap();
    owner
}

#[test]
fn light_icons_pick_the_visible_extent_at_any_distance() {
    for depth in [2.0, 100.0, 1000.0] {
        let mut world = World::new();
        let entity = spawn_light(&mut world, [depth * 0.3, 0.0, -depth], [0.01; 3]);
        let interaction = SceneInteraction::default();
        let dimensions = Vec2::from_array(helio_pass_billboard::feature::light_icon_dimensions());
        let half_width = 0.02 * depth as f32 * dimensions.x / dimensions.max_element();
        let ray = Vec3::new(depth as f32 * 0.3 + half_width * 0.9, 0.0, -depth as f32);
        assert_eq!(
            interaction.pick(&world, DVec3::ZERO, ray),
            Some("light".into())
        );
        let miss = Vec3::new(depth as f32 * 0.3 + half_width * 1.1, 0.0, -depth as f32);
        assert_eq!(interaction.pick(&world, DVec3::ZERO, miss), None);
        world.insert(
            entity,
            Visibility {
                visible: false,
                locked: false,
            },
        );
        assert_eq!(interaction.pick(&world, DVec3::ZERO, ray), None);
    }
}

#[test]
fn mesh_in_front_occludes_light_icon_selection() {
    let mut world = World::new();
    spawn_light(&mut world, [0.0, 0.0, -100.0], [1.0; 3]);
    let mesh = world
        .spawn_object(
            SpawnObject::new("mesh")
                .with_id("mesh")
                .with_transform(Transform {
                    position: [0.0, 0.0, -10.0],
                    ..Default::default()
                }),
        )
        .unwrap();
    world.insert(mesh, TestBox);
    assert_eq!(
        SceneInteraction::default().pick(&world, DVec3::ZERO, -Vec3::Z),
        Some("mesh".into())
    );
    assert_eq!(
        SceneInteraction::default().pick(&world, DVec3::ZERO, Vec3::Z),
        None
    );
}
