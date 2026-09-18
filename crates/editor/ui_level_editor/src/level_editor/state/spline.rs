//! Spline Domain — in-memory polyline authored by the Spline tool mode.
//!
//! Milestone 5 ("extensibility demo", design doc §8) proves that
//! `ToolModeRegistry::register(...)` supports a brand-new mode without
//! touching any Milestone 1-4 file's logic. This domain is that mode's
//! cross-cutting state: a list of points the user has clicked in the
//! viewport, kept here (rather than as a field on `SplineMode` itself)
//! because it needs to be read by both the toolbar (`toolbar_controls`) and
//! the status bar (`status`) — both of which build their own throwaway
//! `ToolModeContext` from `&LevelEditorState` alone, never through the
//! `SplineMode` instance living in the registry. `TerrainDomain` is the
//! precedent for this choice (design doc §4.3); see
//! `docs/adding-a-tool-mode.md` for the general rule.
//!
//! In-memory only by design (no persistence, no undo) — this is a proof of
//! the registry contract, not a production authoring tool.

use glam::Vec3;

/// A simple polyline authored by clicking in the viewport.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplineDomain {
    /// Points in world space, in click order.
    pub points: Vec<[f32; 3]>,
}

impl SplineDomain {
    /// Appends a point to the end of the path.
    pub fn push_point(&mut self, point: [f32; 3]) {
        self.points.push(point);
    }

    /// Removes every point, starting a fresh path.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Sum of the straight-line distance between consecutive points, in
    /// meters. `0.0` for zero or one point.
    pub fn total_length_m(&self) -> f32 {
        self.points
            .windows(2)
            .map(|pair| (Vec3::from_array(pair[1]) - Vec3::from_array(pair[0])).length())
            .sum()
    }
}
