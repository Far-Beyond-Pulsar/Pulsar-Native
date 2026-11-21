//! Tests for gizmo hit-testing, dragging and picking.
use super::math::projected_ring_angle;
use super::registrations::ContentGeometryRegistration;
use super::*;
use crate::scene::{SpawnObject, StableId};
use crate::subsystems::render::helio_renderer::gizmo_geometry::rotation_grid;

fn setup(mode: GizmoType) -> (World, SceneInteraction, Entity) {
    let mut world = World::new();
    spawn_cube(&mut world, "selected", -10.0);
    let entity = world.query::<&StableId>().next().unwrap().0;
    world.select(Some(entity));
    let mut interaction = SceneInteraction::default();
    interaction.set_mode(mode);
    interaction.set_view(
        DVec3::ZERO,
        -Vec3::Z,
        glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_4,
            1.0,
            0.1,
            10000.0,
        ),
        Vec2::splat(900.0),
        10_000.0,
    );
    (world, interaction, entity)
}

#[test]
fn distant_planet_origin_has_no_gizmo_beyond_camera_far_plane() {
    let (mut world, interaction, entity) = setup(GizmoType::Translate);
    let mut planet = *world.get::<Transform>(entity).unwrap();
    planet.position = [0.0, 0.0, -1_000_000.0];
    world.insert(entity, planet);
    assert_eq!(world.selected_entity(), Some(entity));
    assert!(interaction.selected(&world).is_some());
    assert!(interaction
        .view
        .length(DVec3::from_array(planet.position).as_vec3())
        .is_none());
}

#[test]
fn press_captures_without_hover_and_drag_continues_off_handle() {
    let (mut world, mut interaction, entity) = setup(GizmoType::Translate);
    let pivot = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(pivot).unwrap();
    let start = pivot + Vec3::X * length * 0.7;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, start.normalize()));
    assert_eq!(interaction.drag.unwrap().handle, Handle::Axis(0));
    let away = pivot + Vec3::new(length * 4.0, length * 2.0, 0.0);
    interaction.update_hover(&world, DVec3::ZERO, away.normalize());
    assert_eq!(interaction.hovered, Some(Handle::Axis(0)));
    interaction.update_drag(&mut world, DVec3::ZERO, away.normalize());
    // The cursor is 3.3 handle lengths along the axis from where the drag
    // started; the moved position lands on the location snap grid.
    let step = f32::from_bits(LOCATION_SNAP.load(Ordering::Relaxed));
    let expected = (3.3 * length / step).round() * step;
    let x = world.get::<Transform>(entity).unwrap().position[0];
    assert!(
        (x - f64::from(expected)).abs() < 1e-3 && x > 0.0,
        "dragged off the handle to x = {x}, expected {expected}"
    );
    interaction.cancel_drag();
    let final_transform = *world.get::<Transform>(entity).unwrap();
    interaction.update_drag(&mut world, DVec3::ZERO, start.normalize());
    assert_eq!(*world.get::<Transform>(entity).unwrap(), final_transform);
}

#[test]
fn dragging_a_folder_moves_what_is_inside_it_without_rewriting_it() {
    let (mut world, mut interaction, folder) = setup(GizmoType::Translate);
    spawn_cube(&mut world, "inside", -10.0);
    let inside = world.entity_for("inside").unwrap();
    // Put in the folder where it stands: its local transform becomes zero.
    crate::scene::hierarchy_transform::reparent_keeping_world(&mut world, inside, Some(folder))
        .unwrap();
    spawn_cube(&mut world, "outside", -10.0);
    let outside = world.entity_for("outside").unwrap();
    assert_eq!(world.get::<Transform>(inside).unwrap().position, [0.0; 3]);

    let pivot = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(pivot).unwrap();
    let start = pivot + Vec3::X * length * 0.7;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, start.normalize()));
    let away = pivot + Vec3::new(length * 4.0, length * 2.0, 0.0);
    interaction.update_drag(&mut world, DVec3::ZERO, away.normalize());

    let moved = world.get::<Transform>(folder).unwrap().position[0];
    assert!(moved > 0.0, "the folder did not move");
    assert_eq!(
        world.get::<Transform>(inside).unwrap().position,
        [0.0; 3],
        "the contents are not rewritten"
    );
    let world_x = world_transform_of(&world, inside).unwrap().position[0];
    assert!(
        (world_x - moved).abs() < 1e-4,
        "contents follow: {world_x} vs {moved}"
    );
    assert_eq!(world.get::<Transform>(outside).unwrap().position[0], 0.0);
}

#[test]
fn dragging_a_child_moves_it_in_the_world_and_stores_it_relative() {
    let (mut world, mut interaction, folder) = setup(GizmoType::Translate);
    // The folder stands 4 to the right of where the child will be dragged.
    world.get_mut::<Transform>(folder).unwrap().position = [4.0, 0.0, -10.0];
    spawn_cube(&mut world, "child", -10.0);
    let child = world.entity_for("child").unwrap();
    crate::scene::hierarchy_transform::reparent_keeping_world(&mut world, child, Some(folder))
        .unwrap();
    world.select(Some(child));
    let before = world_transform_of(&world, child).unwrap();
    assert!((before.position[0]).abs() < 1e-4, "{before:?}");

    let pivot = DVec3::from_array(before.position).as_vec3();
    let length = interaction.view.length(pivot).unwrap();
    let start = pivot + Vec3::X * length * 0.7;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, start.normalize()));
    let away = pivot + Vec3::new(length * 4.0, length * 2.0, 0.0);
    interaction.update_drag(&mut world, DVec3::ZERO, away.normalize());

    let after = world_transform_of(&world, child).unwrap();
    let local = world.get::<Transform>(child).unwrap().position[0];
    assert!(
        after.position[0] > 0.0,
        "the child moved right in the world"
    );
    assert!(
        (local - (after.position[0] - 4.0)).abs() < 1e-3,
        "stored relative to the folder at x = 4: local {local}, world {}",
        after.position[0]
    );
    // Undo restores the child's own (relative) transform.
    let moved = interaction.finish_drag().unwrap();
    let [(entity, restore)] = moved.as_slice() else {
        panic!("one object moved: {moved:?}");
    };
    assert_eq!(*entity, child);
    assert!((restore.position[0] - -4.0).abs() < 1e-4, "{restore:?}");
}

#[test]
fn ring_is_pickable_between_axes() {
    let (world, interaction, entity) = setup(GizmoType::Rotate);
    let t = *world.get::<Transform>(entity).unwrap();
    let pivot = DVec3::from_array(t.position).as_vec3();
    let length = interaction.view.length(pivot).unwrap();
    let point = pivot + Vec3::new(0.6, 0.6, 0.0) * length;
    assert_eq!(
        interaction.hit(t, interaction.view.project(point).unwrap()),
        Some(Handle::Axis(2))
    );
}

#[test]
fn handle_size_is_constant_with_distance() {
    let (_, interaction, _) = setup(GizmoType::Translate);
    for depth in [1.0, 10.0, 1000.0] {
        let pivot = Vec3::new(0.0, 0.0, -depth);
        let end = pivot + Vec3::X * interaction.view.length(pivot).unwrap();
        let size = interaction
            .view
            .project(end)
            .unwrap()
            .distance(interaction.view.project(pivot).unwrap());
        assert!((size - HANDLE_PIXELS).abs() < 0.01);
    }
}

#[test]
fn hover_margin_is_in_pixels_and_empty_space_does_not_capture() {
    let (_, interaction, _) = setup(GizmoType::Translate);
    for depth in [2.0, 40.0, 1000.0] {
        let t = Transform {
            position: [0.0, 0.0, -depth],
            ..Default::default()
        };
        let pivot = DVec3::from_array(t.position).as_vec3();
        let length = interaction.view.length(pivot).unwrap();
        let shaft = interaction
            .view
            .project(pivot + Vec3::X * length * 0.65)
            .unwrap();
        assert_eq!(
            interaction.hit(t, shaft + Vec2::Y * 6.0),
            Some(Handle::Axis(0))
        );
        assert_eq!(interaction.hit(t, shaft + Vec2::Y * 25.0), None);
    }
}

#[test]
fn plane_and_center_handles_capture_and_locked_objects_do_not() {
    let (mut world, mut interaction, entity) = setup(GizmoType::Translate);
    let t = *world.get::<Transform>(entity).unwrap();
    let pivot = DVec3::from_array(t.position).as_vec3();
    let length = interaction.view.length(pivot).unwrap();
    let plane = pivot + Vec3::new(0.33, 0.33, 0.0) * length;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, plane.normalize()));
    assert_eq!(interaction.drag.unwrap().handle, Handle::Plane(2));
    interaction.cancel_drag();
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, pivot.normalize()));
    assert_eq!(interaction.drag.unwrap().handle, Handle::Center);
    interaction.cancel_drag();
    world.get_mut::<Visibility>(entity).unwrap().locked = true;
    assert!(!interaction.try_start_drag(&world, DVec3::ZERO, plane.normalize()));
}

#[test]
fn rotation_follows_ring_plane_and_preserves_other_fields() {
    let (mut world, mut interaction, entity) = setup(GizmoType::Rotate);
    let pivot = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(pivot).unwrap();
    let start = pivot + Vec3::new(0.6, 0.6, 0.0) * length;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, start.normalize()));
    let end = pivot + Vec3::new(-0.6, 0.6, 0.0) * length;
    interaction.update_drag(&mut world, DVec3::ZERO, end.normalize());
    let t = world.get::<Transform>(entity).unwrap();
    assert!((t.rotation[2] - 90.0).abs() < 0.01);
    assert_eq!(t.position, pivot.as_dvec3().to_array());
    assert_eq!(t.scale, [1.0; 3]);
}

#[test]
fn projected_rotation_tracks_oblique_rings_through_multiple_turns() {
    let (_, interaction, _) = setup(GizmoType::Rotate);
    let pivot = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(pivot).unwrap();
    for tilt in [0.0_f32, 0.7, 1.3] {
        let basis = Mat3::from_rotation_x(tilt);
        let mut previous = 0.4;
        for frame in 1..=1440 {
            let expected = 0.4 + frame as f32 * std::f32::consts::TAU / 720.0;
            let position = pivot
                + length * 0.85 * (basis.col(0) * expected.cos() + basis.col(1) * expected.sin());
            let cursor = interaction.view.project(position).unwrap();
            let angle =
                projected_ring_angle(interaction.view, pivot, basis, 2, length, cursor, previous)
                    .unwrap();
            assert!(
                (angle - expected).abs() < 0.001,
                "tilt={tilt} frame={frame} angle={angle} expected={expected}"
            );
            assert!((angle - previous).abs() < 0.02);
            previous = angle;
        }
    }
}

#[test]
fn hidden_rotation_quadrants_do_not_capture() {
    let (world, interaction, entity) = setup(GizmoType::Rotate);
    let t = *world.get::<Transform>(entity).unwrap();
    let pivot = DVec3::from_array(t.position).as_vec3();
    let length = interaction.view.length(pivot).unwrap();
    let hidden = interaction
        .view
        .project(pivot - Vec3::new(0.6, 0.6, 0.0) * length)
        .unwrap();
    assert_eq!(interaction.hit(t, hidden), None);
}

#[test]
fn arcs_and_grids_share_camera_facing_quadrants() {
    let (_, mut interaction, _) = setup(GizmoType::Rotate);
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                let signs = Vec3::new(x, y, z);
                interaction.view.position = signs * 10.0;
                assert_eq!(
                    interaction.quadrant_signs(Vec3::ZERO, Mat3::IDENTITY),
                    signs
                );
                interaction.view.position *= Vec3::new(0.2, 3.0, 0.7);
                assert_eq!(
                    interaction.quadrant_signs(Vec3::ZERO, Mat3::IDENTITY),
                    signs
                );
                for shape in shapes(GizmoType::Rotate) {
                    let visible = shape
                        .triangles
                        .iter()
                        .filter(|tri| {
                            interaction.triangle_visible(
                                shape.handle,
                                tri,
                                Vec3::ZERO,
                                Mat3::IDENTITY,
                            )
                        })
                        .count();
                    assert_eq!(visible * 4, shape.triangles.len());
                    let Handle::Axis(axis) = shape.handle else {
                        unreachable!()
                    };
                    let grid = rotation_grid(axis, signs);
                    assert_eq!(grid.len(), 18);
                    assert_eq!(grid[0][0], Vec3::ZERO);
                    for [a, b] in grid {
                        assert!(a.length() <= 0.81001);
                        assert!((b.length() - 0.81).abs() < 0.00001);
                        assert_eq!(a[axis], 0.0);
                        assert_eq!(b[axis], 0.0);
                        assert!((a * signs).min_element() >= 0.0);
                        assert!((b * signs).min_element() >= 0.0);
                    }
                }
            }
        }
    }
}

#[test]
fn rotation_remains_stable_far_outside_ring() {
    let (_, interaction, _) = setup(GizmoType::Rotate);
    let pivot = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(pivot).unwrap();
    let center = interaction.view.project(pivot).unwrap();
    let mut previous = 0.4;
    for frame in 1..=360 {
        let expected = 0.4 + frame as f32 * 0.01;
        let on_ring = interaction
            .view
            .project(pivot + length * 0.85 * Vec3::new(expected.cos(), expected.sin(), 0.0))
            .unwrap();
        let cursor = center + (on_ring - center) * 5.0;
        let angle = projected_ring_angle(
            interaction.view,
            pivot,
            Mat3::IDENTITY,
            2,
            length,
            cursor,
            previous,
        )
        .unwrap();
        assert!((angle - expected).abs() < 0.002);
        previous = angle;
    }
}

/// A test kind's content: a unit box on objects carrying it.
struct TestBox;

inventory::submit! {
    ContentGeometryRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "test.box",
        local_bounds: |world, object| {
            world
                .get::<TestBox>(object)
                .map(|_| (Vec3::splat(-0.5), Vec3::splat(0.5)))
        },
        hit: |_, _, _, _| None,
        triangles: |_, _| {},
    }
}

fn spawn_cube(world: &mut World, id: &str, z: f64) {
    let object = world
        .spawn_object(SpawnObject::new(id).with_id(id).with_transform(Transform {
            position: [0.0, 0.0, z],
            ..Default::default()
        }))
        .unwrap();
    world.insert(object, TestBox);
}

#[test]
fn nearest_visible_mesh_is_picked_from_world() {
    let mut world = World::new();
    spawn_cube(&mut world, "far", -10.0);
    spawn_cube(&mut world, "near", -3.0);

    let interaction = SceneInteraction::default();
    assert_eq!(
        interaction.pick(&world, DVec3::ZERO, -Vec3::Z),
        Some("near".into())
    );
}

#[test]
fn hidden_mesh_is_not_pickable() {
    let mut world = World::new();
    let hidden = world
        .spawn_object(
            SpawnObject::new("Hidden")
                .with_id("hidden")
                .with_visibility(Visibility {
                    visible: false,
                    locked: false,
                }),
        )
        .unwrap();
    world.insert(hidden, TestBox);
    assert_eq!(
        SceneInteraction::default().pick(&world, DVec3::ZERO, -Vec3::Z),
        None
    );
}

/// Two cubes 2 m either side of the view's centre line, 10 m away.
fn two_cubes(mode: GizmoType) -> (World, SceneInteraction, Entity, Entity) {
    let (mut world, interaction, _) = setup(mode);
    world.despawn_tree(world.selected_entity().unwrap());
    spawn_cube(&mut world, "left", -10.0);
    spawn_cube(&mut world, "right", -10.0);
    let left = world.entity_for("left").unwrap();
    let right = world.entity_for("right").unwrap();
    for (entity, x) in [(left, -2.0), (right, 2.0)] {
        let mut t = *world.get::<Transform>(entity).unwrap();
        t.position[0] = x;
        world.insert(entity, t);
    }
    (world, interaction, left, right)
}

#[test]
fn box_pick_takes_only_objects_wholly_inside() {
    let (world, interaction, left, right) = two_cubes(GizmoType::Translate);
    // The left half of the view holds the left cube.
    let picked = interaction.box_pick(&world, Vec2::new(0.0, 0.0), Vec2::new(0.5, 1.0));
    assert_eq!(picked, vec![left]);
    // Corners in either order; the whole view holds both.
    let picked = interaction.box_pick(&world, Vec2::ONE, Vec2::ZERO);
    assert_eq!(picked.len(), 2);
    assert!(picked.contains(&left) && picked.contains(&right));
    // A box through the middle of the left cube does not hold all of it.
    let center = interaction
        .view
        .project(Vec3::new(-2.0, 0.0, -10.0))
        .unwrap()
        / interaction.view.size;
    let picked = interaction.box_pick(&world, Vec2::ZERO, Vec2::new(center.x, 1.0));
    assert!(picked.is_empty(), "{picked:?}");
}

#[test]
fn group_gizmo_sits_at_the_average_and_moves_every_object() {
    let (mut world, mut interaction, left, right) = two_cubes(GizmoType::Translate);
    world.select_all(&[left, right]);
    assert_eq!(
        world.selected_entity(),
        Some(right),
        "the last picked is primary"
    );
    let (_, pivot) = interaction.selected(&world).unwrap();
    assert_eq!(pivot.position, [0.0, 0.0, -10.0]);
    assert_eq!(pivot.rotation, [0.0; 3], "world axes for a group");

    let at = Vec3::new(0.0, 0.0, -10.0);
    let length = interaction.view.length(at).unwrap();
    let start = at + Vec3::X * length * 0.7;
    assert!(interaction.try_start_drag(&world, DVec3::ZERO, start.normalize()));
    let away = at + Vec3::new(length * 4.0, 0.0, 0.0);
    interaction.update_drag(&mut world, DVec3::ZERO, away.normalize());
    let step = f32::from_bits(LOCATION_SNAP.load(Ordering::Relaxed));
    let travel = f64::from((3.3 * length / step).round() * step);
    let x = |e| world.get::<Transform>(e).unwrap().position[0];
    assert!(
        (x(left) - (-2.0 + travel)).abs() < 1e-3,
        "left at {}",
        x(left)
    );
    assert!(
        (x(right) - (2.0 + travel)).abs() < 1e-3,
        "right at {}",
        x(right)
    );

    // One undo step holds both starting transforms.
    let moved = interaction.finish_drag().unwrap();
    assert_eq!(moved.len(), 2);
    assert!(moved.contains(&(
        left,
        Transform {
            position: [-2.0, 0.0, -10.0],
            ..Default::default()
        }
    )));
}

#[test]
fn a_selected_child_moves_with_its_selected_parent_only_once() {
    let (mut world, interaction, left, right) = two_cubes(GizmoType::Translate);
    world.reparent(right, Some(left)).unwrap();
    world.select_all(&[left, right]);
    let targets = interaction.gizmo_targets(&world).unwrap();
    assert_eq!(
        targets.objects.len(),
        1,
        "the child rides along with its parent"
    );
    assert_eq!(targets.objects[0].0, left);
}
