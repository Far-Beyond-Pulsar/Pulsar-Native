//! The Create menu's objects (#1012): each kind adds one selected object
//! with the components its name promises, as one undo step. The shell's own
//! kinds here; the plugins' (lights, meshes, sky, wind) are tested where
//! every installed plugin's editor module is linked
//! (`installed_plugins`' `tests/level_editor.rs`).

use crate::scene_edit::{components, create, objects};
use crate::state::LevelEditorState;

#[test]
fn every_shell_create_kind_adds_one_selected_object() {
    let ids: Vec<_> = crate::extend::create_entries()
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    assert_eq!(ids, ["empty", "folder", "camera"]);
    for kind in ids {
        let mut state = LevelEditorState::new();
        let result = create::create(&mut state, kind);
        assert!(result.changed, "{kind}: {}", result.no_op_reason);
        let id = result.affected_ids[0].clone();
        assert_eq!(state.scene.selected_object(), Some(id.clone()), "{kind}");
        assert!(
            components::get_component_class_names(&state.scene.world(), &id).is_empty(),
            "{kind}"
        );

        assert!(state.scene.undo(), "{kind}");
        assert!(
            objects::get_all_objects(&state.scene.world()).is_empty(),
            "{kind}"
        );
    }
}

#[test]
fn an_unknown_kind_adds_nothing() {
    let mut state = LevelEditorState::new();
    assert!(!create::create(&mut state, "dragon").changed);
    assert!(!state.scene.can_undo());
}
