//! Terrain Domain — the foliage brush
//!
//! Authoring settings of the terrain tool: foliage painting. Voxel worlds
//! are created from the terrain tool's Manage tab and sculpted with brushes
//! (the voxel sculpt tool, `super::voxel`).

use serde::{Deserialize, Serialize};

// ── Foliage Brush ──────────────────────────────────────────────────────────

/// Brush parameters for painting foliage instances.
///
/// Mirrors `helio_component::FoliageComponent`'s own sub-property groups
/// (`general`/`placement`/`rendering`/`wind`/`interaction` —
/// `tool_modes::terrain::foliage::build_component` maps each field across
/// one-to-one) so the brush can author everything the component actually
/// renders, not just density/radius/slope. Two fields from the component are
/// deliberately NOT here: `altitude_min`/`altitude_max` are a world-Y
/// acceptance band, not a creative brush parameter — leaving them exposed
/// let a planet-surface stamp silently self-exclude (see `foliage.rs`'s
/// `recenter_altitude_band`, which computes them from the hit point instead
/// of a fixed brush setting). `base_color`/`wind_direction` are `[f32; 4]`/
/// `[f32; 3]` vectors with no `ToolWidget` representation yet (no color/
/// vector picker widget exists) — future work, not silently dropped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoliageBrush {
    // ── General ──
    pub type_id: String,
    pub density: f32,

    // ── Placement ──
    pub radius_m: f32,
    pub slope_limit: (f32, f32),
    /// Per-instance height range, in meters (`PlacementFoliageProps::
    /// height_min/max`).
    pub height_range: (f32, f32),
    /// Per-instance width/spread range, in meters (`width_min/max`).
    pub width_range: (f32, f32),

    // ── Rendering ──
    pub two_sided: bool,
    pub casts_shadow: bool,
    pub roughness: f32,
    pub metallic: f32,
    /// First LOD cutoff distance, in meters (`lod_distance_0`). The
    /// component has three more distance tiers; only the nearest is exposed
    /// as a brush setting today (a single "how far before it simplifies"
    /// knob covers the common case — full LOD-curve authoring is future work).
    pub lod_distance: f32,

    // ── Wind ──
    pub wind_enabled: bool,
    pub trunk_sway: f32,
    pub branch_flutter: f32,
    pub leaf_jitter: f32,
    pub wind_speed: f32,

    // ── Interaction ──
    /// Radius (meters) a moving object bends this foliage within
    /// (`InteractionFoliageProps::interactor_radius`).
    pub interactor_radius: f32,
}

impl Default for FoliageBrush {
    fn default() -> Self {
        Self {
            type_id: "default_grass".to_string(),
            density: 1.0,
            radius_m: 4.0,
            slope_limit: (0.0, 45.0),
            // Matches `PlacementFoliageProps::default()`.
            height_range: (0.18, 0.5),
            width_range: (0.012, 0.03),
            // Matches `RenderingFoliageProps::default()`.
            two_sided: true,
            casts_shadow: false,
            roughness: 0.85,
            metallic: 0.0,
            lod_distance: 8.0,
            // Matches `WindFoliageProps::default()`.
            wind_enabled: true,
            trunk_sway: 0.0,
            branch_flutter: 0.35,
            leaf_jitter: 1.0,
            wind_speed: 2.0,
            // Matches `InteractionFoliageProps::default()`.
            interactor_radius: 1.2,
        }
    }
}

// ── Terrain Domain ─────────────────────────────────────────────────────────

/// Terrain authoring configuration state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TerrainDomain {
    pub foliage: FoliageBrush,
    /// The sets/members the foliage brush paints from. See
    /// [`super::foliage_sets`].
    pub foliage_sets: super::foliage_sets::FoliageSetLibrary,
    /// Foliage brush density multiplier (0 = paints nothing, 1 = each
    /// member's own density), applied on top of per-member density.
    pub foliage_paint_density: super::foliage_sets::BrushDensity,
    /// What a foliage-brush click does: place instances or remove them.
    pub foliage_tool: FoliageTool,
    /// Fraction of instances inside the brush an erase stroke removes
    /// (1 removes everything under the brush).
    pub foliage_erase_density: super::foliage_sets::BrushDensity,
}

/// Foliage brush action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FoliageTool {
    #[default]
    Paint,
    Erase,
}

impl TerrainDomain {
    /// Choose a foliage action.
    pub fn activate_foliage_tool(&mut self, tool: FoliageTool) {
        self.foliage_tool = tool;
    }

    pub fn set_foliage_radius(&mut self, radius: f32) {
        self.foliage.radius_m = radius.clamp(1.0, 64.0);
    }
}

#[cfg(test)]
mod tool_activation_tests {
    use super::*;

    #[test]
    fn a_foliage_tool_selects_the_action() {
        let mut domain = TerrainDomain::default();
        domain.activate_foliage_tool(FoliageTool::Erase);
        assert_eq!(domain.foliage_tool, FoliageTool::Erase);
    }

    #[test]
    fn erase_and_paint_density_default_to_full_strength() {
        let domain = TerrainDomain::default();
        assert_eq!(domain.foliage_paint_density.0, 1.0);
        assert_eq!(domain.foliage_erase_density.0, 1.0);
    }
}
