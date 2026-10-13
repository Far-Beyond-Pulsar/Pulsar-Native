//! The component that puts a mapping to work on an object.

use engine_class_derive::{engine_class, register_world_component};

/// The class name scripts and level files use.
pub const PLAYER_INPUT_CLASS_NAME: &str = "PlayerInputComponent";

/// Makes an input mapping active while this object exists. The actions the
/// mapping defines are published on this object's event channel
/// (`input::Jump.triggered`, ...), so a script attached to the same object
/// hears its own player's input and nobody else's.
#[engine_class(category = "Input", clone, debug, serialize, deserialize)]
#[category("Input", category_color = "#4ECDC4")]
#[serde(default)]
pub struct PlayerInputComponent {
    /// Turn the mapping off without removing the component.
    #[property(category = "Input")]
    pub enabled: bool,
    /// Project-relative path of an `.inputmap` asset.
    #[property(category = "Input")]
    pub mapping: String,
    /// Added to the mapping's own priority; higher sees input first.
    #[property(min = -1000.0, max = 1000.0, step = 1.0, category = "Input")]
    pub priority: i32,
}

impl Default for PlayerInputComponent {
    fn default() -> Self {
        Self { enabled: true, mapping: String::new(), priority: 0 }
    }
}

#[register_world_component]
impl PlayerInputComponent {}
