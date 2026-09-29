//! Terrain Domain — sculpt brush, foliage brush, target, and stroke state
//!
//! Stores authoring configuration for voxel terrain sculpting and foliage painting.
//! Actual voxel volume / planetary terrain data remains in engine/subsystem runtimes,
//! while this domain tracks brush properties, active target, and active strokes.

use serde::{Deserialize, Serialize};

// ── Sculpt Mode ────────────────────────────────────────────────────────────

/// Sculpt operation mode for voxel terrain editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SculptMode {
    #[default]
    Raise,
    Lower,
    Flatten,
    Paint,
}

// ── Brush Shape ────────────────────────────────────────────────────────────

/// Which `pulsar_terrain::EditShape` a sculpt stamp is built from.
///
/// Both variants exist in the canonical edit format already (`EditShape::
/// Sphere`/`Box`, added for Milestone 3's flat volumes) — this just makes the
/// choice a brush setting instead of always defaulting to `Sphere`, which is
/// all Milestones 2-4 ever stamped with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BrushShape {
    #[default]
    Sphere,
    Box,
}

// ── Sculpt Brush ───────────────────────────────────────────────────────────

/// Brush parameters for voxel terrain sculpting.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SculptBrush {
    pub mode: SculptMode,
    pub shape: BrushShape,
    pub radius_m: f32,
    pub falloff: f32,
    pub strength: f32,
    pub material: u32,
}

impl Default for SculptBrush {
    fn default() -> Self {
        Self {
            mode: SculptMode::default(),
            shape: BrushShape::default(),
            radius_m: 8.0,
            falloff: 0.5,
            strength: 1.0,
            material: 1,
        }
    }
}

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

// ── Terrain Target ─────────────────────────────────────────────────────────

/// Identifies which terrain surface or volume is being edited.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerrainTarget {
    #[default]
    None,
    Planet(String),
    Volume(String),
}

// ── Stroke ─────────────────────────────────────────────────────────────────

/// An active brush stroke capturing initial height and stroke identifier for undo coalescing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub id: u64,
    /// Altitude the stroke was anchored to, in meters from the planet centre.
    ///
    /// `f64`, not `f32`: on an Earth-sized planet this is ~6.4e6, where an
    /// `f32` step is around half a metre -- enough to make a Flatten stroke
    /// visibly stair-step instead of levelling.
    pub start_height: f64,
}

// ── Terrain Domain ─────────────────────────────────────────────────────────

/// Terrain authoring configuration state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TerrainDomain {
    pub sculpt: SculptBrush,
    pub foliage: FoliageBrush,
    pub target: TerrainTarget,
    pub active_stroke: Option<Stroke>,
    /// Whether Terrain mode's active brush is the foliage stamp rather than
    /// the voxel sculpt brush.
    ///
    /// Design doc §6: foliage painting is a sub-tab/toggle of `TerrainMode`,
    /// not a separate `ToolMode` -- this is that toggle's state, flipped by
    /// the toolbar's `ToolWidget::Toggle` and read by
    /// `TerrainMode::on_pointer` to pick which stamp a brush click produces.
    pub paint_foliage: bool,
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
    pub fn set_sculpt_mode(&mut self, mode: SculptMode) {
        self.sculpt.mode = mode;
    }

    pub fn set_brush_radius(&mut self, radius: f32) {
        self.sculpt.radius_m = radius.clamp(1.0, 64.0);
    }

    pub fn set_brush_strength(&mut self, strength: f32) {
        self.sculpt.strength = strength.clamp(0.1, 10.0);
    }

    pub fn set_brush_falloff(&mut self, falloff: f32) {
        self.sculpt.falloff = falloff.clamp(0.0, 1.0);
    }

    pub fn set_brush_material(&mut self, material: u32) {
        self.sculpt.material = material;
    }

    pub fn set_brush_shape(&mut self, shape: BrushShape) {
        self.sculpt.shape = shape;
    }

    pub fn set_paint_foliage(&mut self, on: bool) {
        self.paint_foliage = on;
    }

    /// Choose a sculpt tool and make the *terrain* brush the live one (a
    /// tool button both selects and activates, like Unreal's tool row).
    pub fn activate_sculpt_tool(&mut self, mode: SculptMode) {
        self.sculpt.mode = mode;
        self.paint_foliage = false;
    }

    /// Choose a paint material: switches to the Paint tool with that
    /// material and makes the terrain brush live.
    pub fn activate_paint_material(&mut self, material: u32) {
        self.sculpt.material = material;
        self.activate_sculpt_tool(SculptMode::Paint);
    }

    /// Choose a foliage action and make the foliage brush the live one.
    pub fn activate_foliage_tool(&mut self, tool: FoliageTool) {
        self.foliage_tool = tool;
        self.paint_foliage = true;
    }

    pub fn set_foliage_density(&mut self, density: f32) {
        self.foliage.density = density.clamp(0.0, 2048.0);
    }

    pub fn set_foliage_radius(&mut self, radius: f32) {
        self.foliage.radius_m = radius.clamp(1.0, 64.0);
    }

    /// Clamped against the current max so the min slider can never cross it
    /// (the max slider clamps symmetrically against the min below).
    pub fn set_foliage_slope_min(&mut self, degrees: f32) {
        self.foliage.slope_limit.0 = degrees.clamp(0.0, 90.0).min(self.foliage.slope_limit.1);
    }

    pub fn set_foliage_slope_max(&mut self, degrees: f32) {
        self.foliage.slope_limit.1 = degrees.clamp(0.0, 90.0).max(self.foliage.slope_limit.0);
    }

    pub fn set_foliage_height_min(&mut self, meters: f32) {
        self.foliage.height_range.0 = meters.clamp(0.01, 10.0).min(self.foliage.height_range.1);
    }

    pub fn set_foliage_height_max(&mut self, meters: f32) {
        self.foliage.height_range.1 = meters.clamp(0.01, 10.0).max(self.foliage.height_range.0);
    }

    pub fn set_foliage_width_min(&mut self, meters: f32) {
        self.foliage.width_range.0 = meters.clamp(0.001, 5.0).min(self.foliage.width_range.1);
    }

    pub fn set_foliage_width_max(&mut self, meters: f32) {
        self.foliage.width_range.1 = meters.clamp(0.001, 5.0).max(self.foliage.width_range.0);
    }

    pub fn set_foliage_two_sided(&mut self, on: bool) {
        self.foliage.two_sided = on;
    }

    pub fn set_foliage_casts_shadow(&mut self, on: bool) {
        self.foliage.casts_shadow = on;
    }

    pub fn set_foliage_roughness(&mut self, value: f32) {
        self.foliage.roughness = value.clamp(0.0, 1.0);
    }

    pub fn set_foliage_metallic(&mut self, value: f32) {
        self.foliage.metallic = value.clamp(0.0, 1.0);
    }

    pub fn set_foliage_lod_distance(&mut self, meters: f32) {
        self.foliage.lod_distance = meters.clamp(1.0, 500.0);
    }

    pub fn set_foliage_wind_enabled(&mut self, on: bool) {
        self.foliage.wind_enabled = on;
    }

    pub fn set_foliage_trunk_sway(&mut self, value: f32) {
        self.foliage.trunk_sway = value.clamp(0.0, 5.0);
    }

    pub fn set_foliage_branch_flutter(&mut self, value: f32) {
        self.foliage.branch_flutter = value.clamp(0.0, 5.0);
    }

    pub fn set_foliage_leaf_jitter(&mut self, value: f32) {
        self.foliage.leaf_jitter = value.clamp(0.0, 5.0);
    }

    pub fn set_foliage_wind_speed(&mut self, value: f32) {
        self.foliage.wind_speed = value.clamp(0.0, 20.0);
    }

    pub fn set_foliage_interactor_radius(&mut self, meters: f32) {
        self.foliage.interactor_radius = meters.clamp(0.0, 10.0);
    }

    pub fn set_target(&mut self, target: TerrainTarget) {
        self.target = target;
    }

    pub fn begin_stroke(&mut self, id: u64, start_height: f64) {
        self.active_stroke = Some(Stroke { id, start_height });
    }

    pub fn end_stroke(&mut self) -> Option<Stroke> {
        self.active_stroke.take()
    }
}

#[cfg(test)]
mod tool_activation_tests {
    use super::*;

    #[test]
    fn a_sculpt_tool_selects_its_mode_and_makes_the_terrain_brush_live() {
        let mut domain = TerrainDomain::default();
        domain.paint_foliage = true;
        domain.activate_sculpt_tool(SculptMode::Lower);
        assert_eq!(domain.sculpt.mode, SculptMode::Lower);
        assert!(!domain.paint_foliage);
    }

    #[test]
    fn picking_a_material_switches_to_the_paint_tool_with_that_material() {
        let mut domain = TerrainDomain::default();
        domain.activate_paint_material(7);
        assert_eq!(domain.sculpt.mode, SculptMode::Paint);
        assert_eq!(domain.sculpt.material, 7);
        assert!(!domain.paint_foliage);
    }

    #[test]
    fn a_foliage_tool_selects_the_action_and_makes_the_foliage_brush_live() {
        let mut domain = TerrainDomain::default();
        domain.activate_foliage_tool(FoliageTool::Erase);
        assert_eq!(domain.foliage_tool, FoliageTool::Erase);
        assert!(domain.paint_foliage);
    }

    #[test]
    fn erase_and_paint_density_default_to_full_strength() {
        let domain = TerrainDomain::default();
        assert_eq!(domain.foliage_paint_density.0, 1.0);
        assert_eq!(domain.foliage_erase_density.0, 1.0);
    }
}
