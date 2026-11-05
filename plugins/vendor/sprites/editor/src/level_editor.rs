//! Placing 2D sprites (#1060) in the level editor: the hierarchy's quick-add
//! button, registered through the level editor's extension points
//! (`ui_level_editor::extend`).
//!
//! A sprite is an object holding a `SpriteComponent`. It draws over the 3D
//! scene in a 2D screen space of its own (1 unit = 1 pixel, origin at the
//! centre of the view, Y up): its owner's X and Y place it, the owner's
//! roll turns it, and its `z_index` orders it among sprites. Moving,
//! turning and scaling the object (gizmo or transform fields) moves the
//! sprite; the 3D camera does not.

use sprites_component::SpriteComponent;
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::{CommandResult, TypedComponent};
use ui_level_editor::extend::{CreateEntry, add_object};
use ui_level_editor::scene_edit::{ObjectType, Transform};

/// Name of the objects [`create_sprite`] adds.
pub const SPRITE_OBJECT_NAME: &str = "Sprite";

/// Add a sprite at the 2D `position` (pixels from the centre of the view)
/// as one undoable command: an object named [`SPRITE_OBJECT_NAME`] holding
/// `sprite`. The new object is selected.
pub fn create_sprite(
    state: &mut LevelEditorState,
    position: [f32; 2],
    sprite: SpriteComponent,
) -> CommandResult {
    add_object(
        state,
        SPRITE_OBJECT_NAME,
        ObjectType::Empty,
        Transform {
            position: [f64::from(position[0]), f64::from(position[1]), 0.0],
            ..Transform::default()
        },
        vec![TypedComponent::new(sprite)],
    )
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "sprite",
        menu_label: None,
        environment_label: None,
        quick_add: Some((|| ui::IconName::Image, "Add 2D Sprite")),
        offered: None,
        order: 500,
        create: |state| create_sprite(state, [0.0, 0.0], SpriteComponent::default()),
    }
}
