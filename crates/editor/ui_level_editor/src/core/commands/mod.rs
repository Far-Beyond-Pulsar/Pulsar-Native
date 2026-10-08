/// Scene command system - single execution path for all scene mutations.
///
/// `SceneCommand` is a self-contained description of one editor operation.
/// `execute_command()` applies it through the `scene_edit` functions, which
/// write the `SceneDb` world directly; the renderer reads that same world,
/// so the viewport follows without a separate update.
///
/// Both user GPUI action handlers and AI tool implementations call
/// `execute_command()`, giving a single auditable code path that is ready for
/// undo / redo to be layered on top.
mod executor;
mod types;

pub use executor::execute_command;
pub use types::{CommandResult, ComponentData, SceneCommand, TypedComponent};

/// The editor's Duplicate: one copy of `source_id`, where the source is,
/// selected. The copy starts on its source; leaving the source selected made
/// the gizmo and the properties panel act on the source, so the two looked
/// tied together (Pulsar-Native#1048). Returns the copy's id.
pub fn duplicate_and_select(
    state: &mut crate::state::LevelEditorState,
    source_id: &str,
) -> Option<crate::scene_edit::ObjectId> {
    let copy = execute_command(
        state,
        SceneCommand::DuplicateObject {
            source_id: source_id.to_string(),
            count: 1,
            position_offset: None,
        },
    )
    .affected_ids
    .last()
    .cloned()?;
    execute_command(
        state,
        SceneCommand::SelectObject {
            id: Some(copy.clone()),
        },
    );
    Some(copy)
}
