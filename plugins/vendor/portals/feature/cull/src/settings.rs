//! The portal feature's host settings, described once (as `helio_settings`
//! describes the renderer's): engines build their settings UI and read the
//! values back through it.

use helio_settings::{toggle, Group, Resolved};

use crate::PortalSettings;

/// Whether the portal passes build: a host lists it with its other settings
/// groups.
pub static PORTALS: Group = Group {
    key: "portals",
    title: "Portals",
    description: "Openings that show another place",
    settings: &[toggle(
        "enabled",
        "Portals",
        "Portals",
        true,
        "Portal passes (about 10 MB of GPU memory)",
    )],
};

impl PortalSettings {
    /// The settings `settings` (of [`PORTALS`]) hold.
    pub fn from_settings(settings: &Resolved) -> Self {
        debug_assert!(std::ptr::eq(settings.group(), &PORTALS));
        Self {
            enabled: settings.bool("enabled"),
        }
    }
}
