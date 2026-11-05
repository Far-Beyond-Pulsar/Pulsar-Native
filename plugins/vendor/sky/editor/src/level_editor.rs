//! The sky in the level editor (#1057): its Create entry, its World Settings
//! section and its place in the environment mixer, registered through the
//! level editor's extension points (`ui_level_editor::extend`).
//!
//! A level has one sky: its `AtmosphereComponent` ([`level_atmosphere`]).
//! Without one the sky renders black. The section shows the sky's
//! properties when the level has one and a button that creates it (with a
//! sun, the clear-day starter setup when the lights offer it) when it does
//! not.

use sky_component::{ATMOSPHERE_CLASS, AtmosphereComponent, level_atmosphere};
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::{CommandResult, TypedComponent};
use ui_level_editor::extend::{
    CreateEntry, LevelComponentSection, add_object, apply_environment_preset,
};
use ui_level_editor::scene_edit::{ObjectType, Transform};

/// Name of the object [`create_sky`] adds.
pub const SKY_OBJECT_NAME: &str = "Sky";

/// Add the level's sky: an object named [`SKY_OBJECT_NAME`] holding a
/// default (Earth) `AtmosphereComponent`, as one undoable command, and
/// select it. Refused with a message when the level already has one.
pub fn create_sky(state: &mut LevelEditorState) -> CommandResult {
    add_object(
        state,
        SKY_OBJECT_NAME,
        ObjectType::Empty,
        Transform::default(),
        vec![TypedComponent::new(AtmosphereComponent::default())],
    )
}

/// The section's create button: an atmosphere without a directional sun
/// has zero illuminance and composites as a black sky, so start with the
/// clear-day setup (a sky and a sun) when the project's lights offer it.
fn create_sky_with_sun(state: &mut LevelEditorState) -> CommandResult {
    let preset = apply_environment_preset(state, "clear_day");
    if preset.changed {
        preset
    } else {
        create_sky(state)
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "sky",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Sky").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 300,
        create: create_sky,
    }
}

inventory::submit! {
    LevelComponentSection {
        origin: pulsar_plugins::origin!(),
        title: "Sky",
        sidebar_title: "Sky and Atmosphere",
        sidebar_missing: "Missing · creates a sky",
        order: 0,
        class_name: ATMOSPHERE_CLASS,
        find: level_atmosphere,
        missing_note: "This level has no sky, so it renders black.",
        create_label: "Create Sky",
        create: create_sky_with_sun,
        describe: |object, enabled| {
            if enabled {
                format!("The sky is {object}'s AtmosphereComponent.")
            } else {
                format!("{object}'s AtmosphereComponent is disabled, so the sky renders black.")
            }
        },
    }
}
