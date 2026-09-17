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

// ── Sculpt Brush ───────────────────────────────────────────────────────────

/// Brush parameters for voxel terrain sculpting.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SculptBrush {
    pub mode: SculptMode,
    pub radius_m: f32,
    pub falloff: f32,
    pub strength: f32,
    pub material: u32,
}

impl Default for SculptBrush {
    fn default() -> Self {
        Self {
            mode: SculptMode::default(),
            radius_m: 8.0,
            falloff: 0.5,
            strength: 1.0,
            material: 1,
        }
    }
}

// ── Foliage Brush ──────────────────────────────────────────────────────────

/// Brush parameters for painting foliage instances / layer definitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoliageBrush {
    pub type_id: String,
    pub density: f32,
    pub radius_m: f32,
    pub slope_limit: (f32, f32),
}

impl Default for FoliageBrush {
    fn default() -> Self {
        Self {
            type_id: "default_grass".to_string(),
            density: 1.0,
            radius_m: 4.0,
            slope_limit: (0.0, 45.0),
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
    pub start_height: f32,
}

// ── Terrain Domain ─────────────────────────────────────────────────────────

/// Terrain authoring configuration state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TerrainDomain {
    pub sculpt: SculptBrush,
    pub foliage: FoliageBrush,
    pub target: TerrainTarget,
    pub active_stroke: Option<Stroke>,
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

    pub fn set_target(&mut self, target: TerrainTarget) {
        self.target = target;
    }

    pub fn begin_stroke(&mut self, id: u64, start_height: f32) {
        self.active_stroke = Some(Stroke { id, start_height });
    }

    pub fn end_stroke(&mut self) -> Option<Stroke> {
        self.active_stroke.take()
    }
}
