//! Terrain mode's layout file — the declarative half of its dock contributions.
//!
//! One stable id per panel, one row per dock placement, so "what this mode
//! opens where" reads like a manifest. Kept separate from [`super::panels`]
//! (the actual GPUI views) so a mode can decide where its panels live without
//! dragging in any UI code — and so a future mode can copy this file as its
//! own layout manifest. See the design doc's §11.

use super::super::{ModePanelDescriptor, ModePanelPlacement};

/// Stable panel id for Terrain's brush palette dock panel. `TerrainMode::
/// build_panel` matches on this to construct the view; the shell uses it to
/// track the panel across mode switches.
pub const TERRAIN_PALETTE: &str = "terrain.palette";

/// Dock panels Terrain adds to the level editor while this mode is active.
pub fn contributed_panels() -> Vec<ModePanelDescriptor> {
    vec![ModePanelDescriptor {
        id: TERRAIN_PALETTE,
        title_key: "LevelEditor.TerrainPalette.Title",
        icon: Some(ui::IconName::Globe),
        placement: ModePanelPlacement::Left,
    }]
}