//! Portal authoring component.
//!
//! A portal is a rectangular opening that shows what is on the other side of
//! its partner: two portals with the same non-zero [`PortalComponent::link_id`]
//! are a pair. The opening lies in the object's local X/Y plane (width along X,
//! height along Y) and looks along local +Z; what you see through one is the
//! world as seen from the partner, so the partner's own opening is the "door"
//! on the far side.
//!
//! Nothing here renders anything. The engine pairs the authored portals each
//! frame (`engine_backend::scene::portals`), runs Helio's projection bridge over
//! them, and hands the portal passes their rows. Portals inside a level instance
//! (a `SubLevelDisplayComponent`'s contents) pair with portals outside it the
//! same way, which is how a doorway into another level instance is made.

use engine_class_derive::{engine_class, register_world_component};

pub const PORTAL_CLASS_NAME: &str = "PortalComponent";

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("General", category_color = "#6EC5FF")]
pub struct PortalComponent {
    /// Whether the portal shows anything. A disabled portal also stops its
    /// partner from showing it.
    #[property]
    pub enabled: bool,
    /// Pairs two portals: exactly two enabled portals sharing a non-zero Link
    /// ID are partners. Zero means unlinked.
    #[property(min = 0.0, max = 1_000_000.0, step = 1.0, category = "General")]
    #[serde(default)]
    pub link_id: i32,
    /// Opening width in metres (local X).
    #[property(min = 0.1, max = 1000.0, step = 0.1, category = "General")]
    pub width: f32,
    /// Opening height in metres (local Y).
    #[property(min = 0.1, max = 1000.0, step = 0.1, category = "General")]
    pub height: f32,
}

impl Default for PortalComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            link_id: 0,
            width: 2.0,
            height: 3.0,
        }
    }
}

#[register_world_component]
impl PortalComponent {}
