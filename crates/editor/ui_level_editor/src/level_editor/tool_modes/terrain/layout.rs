//! Terrain mode's layout file — the declarative half of its dock contributions.
//!
//! One stable id per panel, one row per dock placement, so "what this mode
//! opens where" reads like a manifest. Kept separate from [`super::panels`]
//! (the actual GPUI views) so a mode can decide where its panels live without
//! dragging in any UI code — and so a future mode can copy this file as its
//! own layout manifest. See the design doc's §11.

use super::super::{ModePanelDescriptor, ModePanelPlacement};

/// Stable panel id for the Terrain panel (Manage / Sculpt / Paint tabs).
pub const TERRAIN_PANEL: &str = "terrain.panel";

/// Stable panel id for the foliage sets panel.
pub const TERRAIN_FOLIAGE: &str = "terrain.foliage";

/// Dock panels Terrain adds to the level editor while this mode is active.
/// Both dock on the left, sharing one native tab strip.
pub fn contributed_panels() -> Vec<ModePanelDescriptor> {
    vec![
        ModePanelDescriptor {
            id: TERRAIN_PANEL,
            title_key: "LevelEditor.TerrainPanel.Title",
            icon: Some(ui::IconName::Globe),
            placement: ModePanelPlacement::Left,
        },
        ModePanelDescriptor {
            id: TERRAIN_FOLIAGE,
            title_key: "LevelEditor.FoliagePanel.Title",
            icon: None,
            placement: ModePanelPlacement::Left,
        },
    ]
}
