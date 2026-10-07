//! Spline tool and panel preferences. The curve itself is Helio's
//! `SplineComponent`, a SceneDB World component on the owning object; the
//! aliases below keep the editor's names for it.

pub use helio_component::components::{CurveAlgorithm, SplinePoint};

/// The editor's name for a curve: Helio's SceneDB spline component.
pub type SplineData = helio_component::components::SplineComponent;

/// Locale keys for curve algorithms, which are editor text.
pub trait CurveAlgorithmText {
    fn key(self) -> &'static str;
    fn description(self) -> &'static str;
}
impl CurveAlgorithmText for CurveAlgorithm {
    fn key(self) -> &'static str {
        match self {
            Self::Linear => "LevelEditor.SplinePanel.Linear",
            Self::CatmullRom => "LevelEditor.SplinePanel.CatmullRom",
            Self::Bezier => "LevelEditor.SplinePanel.Bezier",
            Self::Hermite => "LevelEditor.SplinePanel.Hermite",
            Self::BSpline => "LevelEditor.SplinePanel.BSpline",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Linear => "LevelEditor.SplinePanel.LinearHelp",
            Self::CatmullRom => "LevelEditor.SplinePanel.CatmullHelp",
            Self::Bezier => "LevelEditor.SplinePanel.BezierHelp",
            Self::Hermite => "LevelEditor.SplinePanel.HermiteHelp",
            Self::BSpline => "LevelEditor.SplinePanel.BSplineHelp",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplineTool {
    #[default]
    Navigate,
    Draw,
    Select,
    Move,
    Insert,
    Delete,
}
impl SplineTool {
    pub const ALL: [Self; 6] = [
        Self::Navigate,
        Self::Draw,
        Self::Select,
        Self::Move,
        Self::Insert,
        Self::Delete,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Navigate => "LevelEditor.SplinePanel.Navigate",
            Self::Draw => "LevelEditor.SplinePanel.Draw",
            Self::Select => "LevelEditor.SplinePanel.Select",
            Self::Move => "LevelEditor.SplinePanel.Move",
            Self::Insert => "LevelEditor.SplinePanel.Insert",
            Self::Delete => "LevelEditor.SplinePanel.DeletePoint",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawingPlane {
    #[default]
    XZ,
    XY,
    YZ,
}
impl DrawingPlane {
    pub fn normal_axis(self) -> usize {
        match self {
            Self::XZ => 1,
            Self::XY => 2,
            Self::YZ => 0,
        }
    }
    pub fn axes(self) -> (usize, usize) {
        match self {
            Self::XZ => (0, 2),
            Self::XY => (0, 1),
            Self::YZ => (1, 2),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SplineDomain {
    pub tool: SplineTool,
    pub selected_point: Option<usize>,
    pub selected_object: Option<String>,
    pub plane: DrawingPlane,
    pub plane_offset: f32,
    pub snap: bool,
    pub snap_step: f32,
    pub show_all: bool,
    pub show_polygon: bool,
    pub show_points: bool,
    pub show_tangents: bool,
    pub line_width: f32,
    pub preset_radius: f32,
    pub preset_count: usize,
    pub helix_height: f32,
    pub resample_count: usize,
    pub smooth_strength: f32,
}
impl Default for SplineDomain {
    fn default() -> Self {
        Self {
            tool: SplineTool::Navigate,
            selected_point: None,
            selected_object: None,
            plane: DrawingPlane::XZ,
            plane_offset: 0.,
            snap: false,
            snap_step: 1.,
            show_all: true,
            show_polygon: true,
            show_points: true,
            show_tangents: false,
            line_width: 2.,
            preset_radius: 5.,
            preset_count: 8,
            helix_height: 10.,
            resample_count: 16,
            smooth_strength: 0.5,
        }
    }
}
