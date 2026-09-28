//! Scene spline storage and undoable authoring operations.

use crate::level_editor::{
    commands::{SceneCommand, execute_command},
    scene_edit::{self, ObjectType, SceneObjectData, Transform},
    state::{
        LevelEditorState,
        spline::{SplineData, SplinePoint},
    },
};
use glam::{Mat4, Quat, Vec3};
use rust_i18n::t;

pub const SPLINE_PROPERTY: &str = "editor_spline";

pub fn data(object: &SceneObjectData) -> Option<SplineData> {
    let data: SplineData =
        serde_json::from_value(object.props.get(SPLINE_PROPERTY)?.clone()).ok()?;
    (data.points.len() <= 4096
        && data.tension.is_finite()
        && data.points.iter().all(|p| {
            p.position
                .iter()
                .chain(&p.arrive)
                .chain(&p.leave)
                .all(|v| v.is_finite())
        }))
    .then_some(data)
}
pub fn all(state: &LevelEditorState) -> Vec<(SceneObjectData, SplineData)> {
    let mut rows: Vec<_> = scene_edit::objects::get_all_objects(&state.scene.world())
        .into_iter()
        .filter_map(|o| data(&o).map(|d| (o, d)))
        .collect();
    rows.sort_by(|a, b| a.0.name.cmp(&b.0.name).then(a.0.id.cmp(&b.0.id)));
    rows
}
pub fn selected(state: &LevelEditorState) -> Option<(SceneObjectData, SplineData)> {
    let object = state.scene.get_selected_object()?;
    let spline = data(&object)?;
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
    let Some((mut object, mut curve)) = selected(state) else {
        return;
    };
    if object.locked {
        return;
    }
    let before = curve.clone();
    apply(&mut curve);
    if curve == before
        || curve.points.len() > 4096
        || !curve.points.iter().all(|p| {
            p.position
                .iter()
                .chain(&p.arrive)
                .chain(&p.leave)
                .all(|v| v.is_finite())
        })
    {
        return;
    }
    if let Ok(value) = serde_json::to_value(curve) {
        object.props.insert(SPLINE_PROPERTY.into(), value);
        execute_command(state, SceneCommand::UpdateObject { data: object });
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
    let mut object = SceneObjectData {
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
        component_instances: None,
    };
    object.props.insert(
        SPLINE_PROPERTY.into(),
        serde_json::to_value(curve).expect("finite spline data"),
    );
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
            curve.algorithm = crate::level_editor::state::spline::CurveAlgorithm::Linear;
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
        // The same SceneObjectData serialized into .level files carries all curve data.
        let object = selected(&state).unwrap().0;
        let serialized = serde_json::to_string(&object).unwrap();
        let restored: SceneObjectData = serde_json::from_str(&serialized).unwrap();
        assert_eq!(data(&restored).unwrap(), curve);
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
