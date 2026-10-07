//! Scene spline storage and undoable authoring operations.

use crate::{
    commands::{execute_command, CommandResult, SceneCommand},
    scene_edit::{self, ObjectType, SceneObjectData, Transform},
    state::{
        spline::{SplineData, SplinePoint},
        LevelEditorState,
    },
};
use glam::{Mat4, Quat, Vec3};
use pulsar_scenedb::World;
use rust_i18n::t;

/// The component class a curve is stored as: Helio's SceneDB spline
/// component, so it hydrates into the World and the renderer draws it.
pub const SPLINE_CLASS: &str = helio_component::components::SPLINE_CLASS_NAME;

/// Where curves lived before they were components (a JSON prop the
/// renderer never saw). Still read so older levels load; an edit moves the
/// curve into the component and drops the prop.
pub const LEGACY_SPLINE_PROPERTY: &str = "editor_spline";

/// The object's live spline component: its index in the object's
/// component list and its current value, read off the World (the
/// `component_instances` on a `SceneObjectData` is only a load-time copy).
fn live_component(world: &World, id: &str) -> Option<(usize, serde_json::Value)> {
    scene_edit::components::get_components(world, id)
        .into_iter()
        .enumerate()
        .find(|(_, component)| component.enabled && component.class_name == SPLINE_CLASS)
        .map(|(index, component)| (index, component.data))
}

pub fn data(world: &World, object: &SceneObjectData) -> Option<SplineData> {
    let value = live_component(world, &object.id)
        .map(|(_, data)| data)
        .or_else(|| object.props.get(LEGACY_SPLINE_PROPERTY).cloned())?;
    let data: SplineData = serde_json::from_value(value).ok()?;
    data.is_valid().then_some(data)
}

/// `component_instances` for a new spline object: `AddObject` hydrates it
/// into the World as the object's spline component.
pub fn component_instances(curve: &SplineData) -> Option<serde_json::Value> {
    let data = serde_json::to_value(curve).ok()?;
    Some(serde_json::json!([{
        "class_name": SPLINE_CLASS,
        "enabled": true,
        "data": data,
    }]))
}

/// Replace `object`'s curve with `curve`, undoably: through its spline
/// component, or, for a curve still held in the legacy prop, by attaching a
/// component and dropping the prop. `None` when the curve can't be stored.
pub fn write(
    state: &mut LevelEditorState,
    object: &SceneObjectData,
    curve: &SplineData,
) -> Option<CommandResult> {
    if !curve.is_valid() {
        return None;
    }
    let data = serde_json::to_value(curve).ok()?;
    let index = live_component(&state.scene.world(), &object.id).map(|(index, _)| index);
    let result = match index {
        Some(component_index) => execute_command(
            state,
            SceneCommand::SetComponentData {
                id: object.id.clone(),
                component_index,
                data,
            },
        ),
        None => {
            let result = execute_command(
                state,
                SceneCommand::AddComponent {
                    id: object.id.clone(),
                    class_name: SPLINE_CLASS.to_string(),
                    data,
                },
            );
            if object.props.get(LEGACY_SPLINE_PROPERTY).is_some() {
                let mut migrated = object.clone();
                migrated.props.remove(LEGACY_SPLINE_PROPERTY);
                execute_command(state, SceneCommand::UpdateObject { data: migrated });
            }
            result
        }
    };
    Some(result)
}

pub fn all(state: &LevelEditorState) -> Vec<(SceneObjectData, SplineData)> {
    let world = state.scene.world();
    let mut rows: Vec<_> = scene_edit::objects::get_all_objects(&world)
        .into_iter()
        .filter_map(|o| data(&world, &o).map(|d| (o, d)))
        .collect();
    rows.sort_by(|a, b| a.0.name.cmp(&b.0.name).then(a.0.id.cmp(&b.0.id)));
    rows
}
pub fn selected(state: &LevelEditorState) -> Option<(SceneObjectData, SplineData)> {
    let object = state.scene.get_selected_object()?;
    let spline = data(&state.scene.world(), &object)?;
    Some((object, spline))
}
pub fn sync_selection(state: &mut LevelEditorState) {
    let id = state.scene.selected_object();
    if state.editor.spline.selected_object != id {
        state.editor.spline.selected_object = id;
        state.editor.spline.selected_point = None;
    }
    let n = selected(state).map(|(_, d)| d.points.len()).unwrap_or(0);
    if state.editor.spline.selected_point.is_some_and(|i| i >= n) {
        state.editor.spline.selected_point = None;
    }
}
pub fn select(state: &mut LevelEditorState, id: String) {
    execute_command(state, SceneCommand::SelectObject { id: Some(id) });
    sync_selection(state);
}
pub fn edit(state: &mut LevelEditorState, apply: impl FnOnce(&mut SplineData)) {
    let Some((object, mut curve)) = selected(state) else {
        return;
    };
    if object.locked {
        return;
    }
    let before = curve.clone();
    apply(&mut curve);
    if curve == before {
        return;
    }
    if write(state, &object, &curve).is_some() {
        sync_selection(state);
    }
}
pub fn create(state: &mut LevelEditorState, curve: SplineData) {
    let names: Vec<_> = all(state).into_iter().map(|(o, _)| o.name).collect();
    let base = t!("LevelEditor.SplinePanel.Title").to_string();
    let mut index = 1;
    while names.contains(&format!("{base} {index}")) {
        index += 1;
    }
    let object = SceneObjectData {
        id: String::new(),
        name: format!("{base} {index}"),
        object_type: ObjectType::Empty,
        transform: Transform::default(),
        visible: true,
        locked: false,
        parent: None,
        children: vec![],
        scene_path: String::new(),
        props: Default::default(),
        component_instances: component_instances(&curve),
    };
    let result = execute_command(
        state,
        SceneCommand::AddObject {
            data: object,
            parent_id: None,
        },
    );
    if let Some(id) = result.affected_ids.first() {
        select(state, id.clone());
    }
}
pub fn matrix(object: &SceneObjectData) -> Mat4 {
    let tr = &object.transform;
    Mat4::from_scale_rotation_translation(
        Vec3::from(tr.scale),
        Quat::from_euler(
            glam::EulerRot::YXZ,
            tr.rotation[1].to_radians(),
            tr.rotation[0].to_radians(),
            tr.rotation[2].to_radians(),
        ),
        Vec3::from(tr.position),
    )
}
pub fn local_point(object: &SceneObjectData, world: [f32; 3]) -> Option<[f32; 3]> {
    let m = matrix(object);
    if m.determinant().abs() < 1e-8 {
        return None;
    }
    let v = m.inverse().transform_point3(Vec3::from(world));
    v.is_finite().then_some(v.to_array())
}
pub fn preset(state: &LevelEditorState, kind: &str) -> SplineData {
    let settings = &state.editor.spline;
    let radius = settings.preset_radius;
    let count = settings.preset_count.clamp(3, 128);
    let (a, b) = settings.plane.axes();
    let axis = settings.plane.normal_axis();
    let mut curve = SplineData::default();
    let coordinates: Vec<(f32, f32, f32)> = match kind {
        "line" => vec![(-radius, 0., 0.), (radius, 0., 0.)],
        "rectangle" => {
            curve.closed = true;
            curve.algorithm = crate::state::spline::CurveAlgorithm::Linear;
            vec![
                (-radius, -radius, 0.),
                (radius, -radius, 0.),
                (radius, radius, 0.),
                (-radius, radius, 0.),
            ]
        }
        "circle" => {
            curve.closed = true;
            (0..count)
                .map(|i| {
                    let t = i as f32 / count as f32 * std::f32::consts::TAU;
                    (t.cos() * radius, t.sin() * radius, 0.)
                })
                .collect()
        }
        "arc" => (0..count)
            .map(|i| {
                let t = i as f32 / (count - 1) as f32 * std::f32::consts::PI;
                (t.cos() * radius, t.sin() * radius, 0.)
            })
            .collect(),
        "helix" => (0..count)
            .map(|i| {
                let f = i as f32 / (count - 1) as f32;
                let t = f * std::f32::consts::TAU * 2.;
                (
                    t.cos() * radius,
                    t.sin() * radius,
                    f * settings.helix_height,
                )
            })
            .collect(),
        _ => vec![],
    };
    curve.points = coordinates
        .into_iter()
        .map(|(x, y, z)| {
            let mut p = [0.; 3];
            p[a] = x;
            p[b] = y;
            p[axis] = settings.plane_offset + z;
            SplinePoint::new(p)
        })
        .collect();
    curve.auto_tangents();
    curve
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::scene::SceneWorldExt as _;
    #[test]
    fn stored_curve_round_trips_with_settings() {
        let mut curve = SplineData::default();
        curve.closed = true;
        curve.tension = 0.7;
        curve.points.push(SplinePoint::new([1., 2., 3.]));
        let json = serde_json::to_value(&curve).unwrap();
        assert_eq!(serde_json::from_value::<SplineData>(json).unwrap(), curve);
    }
    #[test]
    fn curves_are_scene_objects_and_edits_are_undoable() {
        let mut state = LevelEditorState::new();
        let curve = preset(&state, "circle");
        create(&mut state, curve.clone());
        let first = state.scene.selected_object().unwrap();
        assert_eq!(selected(&state).unwrap().1, curve);
        create(&mut state, SplineData::default());
        let second = state.scene.selected_object().unwrap();
        assert_ne!(first, second);
        assert_eq!(all(&state).len(), 2);
        edit(&mut state, |d| {
            d.points.push(SplinePoint::new([1., 2., 3.]))
        });
        assert_eq!(selected(&state).unwrap().1.points.len(), 1);
        assert!(state.scene.undo());
        assert!(selected(&state).unwrap().1.points.is_empty());
        assert!(state.scene.redo());
        assert_eq!(selected(&state).unwrap().1.points.len(), 1);
        select(&mut state, first);
        assert_eq!(selected(&state).unwrap().1, curve);
        // Saving goes through `get_components`, which reads the live World
        // component, so the level file carries the edited curve.
        let object = selected(&state).unwrap().0;
        let saved = scene_edit::components::get_components(&state.scene.world(), &object.id);
        let spline = saved.iter().find(|c| c.class_name == SPLINE_CLASS).unwrap();
        assert_eq!(
            serde_json::from_value::<SplineData>(spline.data.clone()).unwrap(),
            curve
        );
    }
    #[test]
    fn edits_reach_the_world_component_the_renderer_reads() {
        let mut state = LevelEditorState::new();
        let line = preset(&state, "line");
        create(&mut state, line);
        edit(&mut state, |d| d.closed = true);
        let object = selected(&state).unwrap().0;
        let world = state.scene.world();
        let entity = world.entity_for(&object.id).unwrap();
        let live = world
            .get::<SplineData>(entity)
            .expect("typed World component");
        assert!(live.closed);
        assert!(object.props.get(LEGACY_SPLINE_PROPERTY).is_none());
    }
    #[test]
    fn legacy_prop_curves_load_and_move_to_the_component_on_edit() {
        let mut state = LevelEditorState::new();
        let mut legacy = SplineData::default();
        legacy.points.push(SplinePoint::new([4., 5., 6.]));
        let mut object = SceneObjectData {
            id: String::new(),
            name: "Old spline".into(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        };
        object.props.insert(
            LEGACY_SPLINE_PROPERTY.into(),
            serde_json::to_value(&legacy).unwrap(),
        );
        let result = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object,
                parent_id: None,
            },
        );
        select(&mut state, result.affected_ids[0].clone());
        assert_eq!(selected(&state).unwrap().1, legacy);
        edit(&mut state, |d| d.closed = true);
        let (object, curve) = selected(&state).unwrap();
        assert!(curve.closed);
        assert!(object.props.get(LEGACY_SPLINE_PROPERTY).is_none());
        assert!(live_component(&state.scene.world(), &object.id).is_some());
    }
    #[test]
    fn locked_curve_cannot_be_changed() {
        let mut state = LevelEditorState::new();
        create(&mut state, SplineData::default());
        let id = state.scene.selected_object().unwrap();
        execute_command(
            &mut state,
            SceneCommand::SetVisibility {
                id,
                visible: None,
                locked: Some(true),
            },
        );
        edit(&mut state, |d| d.closed = true);
        assert!(!selected(&state).unwrap().1.closed);
    }
}
