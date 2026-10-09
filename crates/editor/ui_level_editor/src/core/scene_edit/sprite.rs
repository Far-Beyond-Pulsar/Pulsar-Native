//! Placing 2D sprites (#1060) in the editor.
//!
//! A sprite is an object holding a `SpriteComponent`. It draws over the 3D
//! scene in a 2D screen space of its own (1 unit = 1 pixel, origin at the
//! centre of the view, Y up): its owner's X and Y place it, the owner's
//! roll turns it, and its `z_index` orders it among sprites. Moving,
//! turning and scaling the object (gizmo or transform fields) moves the
//! sprite; the 3D camera does not.

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;

/// Name of the objects [`create_sprite`] adds.
pub const SPRITE_OBJECT_NAME: &str = "Sprite";

/// Add a sprite at the 2D `position` (pixels from the centre of the view)
/// as one undoable command: an object named [`SPRITE_OBJECT_NAME`] holding
/// `sprite`. The new object is selected.
pub fn create_sprite(
    state: &mut LevelEditorState,
    position: [f32; 2],
    sprite: helio_component::components::SpriteComponent,
) -> CommandResult {
    let result = execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: SPRITE_OBJECT_NAME.to_string(),
                object_type: ObjectType::Empty,
                transform: Transform {
                    position: [position[0], position[1], 0.0],
                    ..Transform::default()
                },
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id: None,
            components: vec![TypedComponent::new(sprite)],
        },
    );
    if let Some(id) = result.affected_ids.first() {
        state.scene.select_object(Some(id.clone()));
    }
    result
}
