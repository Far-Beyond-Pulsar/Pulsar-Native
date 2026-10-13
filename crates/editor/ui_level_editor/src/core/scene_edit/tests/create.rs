//! The Create menu's objects (#1012): each kind adds one selected object
//! with the components its name promises, as one undo step.

use crate::scene_edit::{components, create, objects};
use crate::state::LevelEditorState;

#[test]
fn every_create_kind_adds_one_selected_object_with_its_components() {
    let expected: &[(&str, &[&str])] = &[
        ("empty", &[]),
        ("folder", &[]),
        ("camera", &[]),
        ("light_point", &["LightComponent"]),
        ("light_spot", &["LightComponent"]),
        ("light_directional", &["LightComponent"]),
        ("mesh_cube", &["StaticMeshComponent"]),
        ("mesh_sphere", &["StaticMeshComponent"]),
        ("mesh_cylinder", &["StaticMeshComponent"]),
        ("mesh_plane", &["StaticMeshComponent"]),
        ("sky", &["AtmosphereComponent"]),
        ("wind", &["WindComponent"]),
    ];
    assert_eq!(
        create::KINDS.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        expected.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
    );
    for &(kind, classes) in expected {
        let mut state = LevelEditorState::new();
        let result = create::create(&mut state, kind);
        assert!(result.changed, "{kind}: {}", result.no_op_reason);
        let id = result.affected_ids[0].clone();
        assert_eq!(state.scene.selected_object(), Some(id.clone()), "{kind}");
        assert_eq!(
            components::get_component_class_names(&state.scene.world(), &id),
            classes,
            "{kind}"
        );

        assert!(state.scene.undo(), "{kind}");
        assert!(objects::get_all_objects(&state.scene.world()).is_empty(), "{kind}");
    }
}

#[test]
fn an_unknown_kind_adds_nothing() {
    let mut state = LevelEditorState::new();
    assert!(!create::create(&mut state, "dragon").changed);
    assert!(!state.scene.can_undo());
}
