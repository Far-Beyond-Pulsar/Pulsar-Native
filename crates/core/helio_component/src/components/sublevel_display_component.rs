//! Sublevel display authoring component.
//!
//! Shows the whole contents of a **level instance** with the instance's origin
//! at the object's origin: put the component on an object, point it at a level
//! file, and that level's objects appear under the object, moving, rotating and
//! scaling with it. Any number of objects may show the same level.
//!
//! A level instance is what the default level already is, implicitly. Others are
//! made by loading a level file (this component) or in memory, by creating an
//! instance and adding objects to it (`pulsar_scene_model::level_instances`).
//!
//! Nothing here loads anything. The engine instantiates the file's objects as
//! generated children of the object (`engine_backend::scene::level_instances`),
//! the way placed classes expand: they are rebuilt from the file, never saved
//! with the level that shows them, and the object itself becomes a rigid frame, so
//! moving it moves the whole sublevel with one matrix.

use engine_class_derive::{engine_class, register_world_component};

pub const SUBLEVEL_DISPLAY_CLASS_NAME: &str = "SubLevelDisplayComponent";

#[engine_class(category = "Level", clone, debug, serialize, deserialize)]
#[category("General", category_color = "#B58CFF")]
pub struct SubLevelDisplayComponent {
    /// Whether the level is shown. Turning it off removes the shown objects.
    #[property]
    pub enabled: bool,
    /// The level file to show (project-relative path, for example
    /// `levels/room.level`). Its origin is this object's origin.
    #[property(category = "General")]
    #[serde(default)]
    pub level: String,
}

impl Default for SubLevelDisplayComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            level: String::new(),
        }
    }
}

#[register_world_component]
impl SubLevelDisplayComponent {}
