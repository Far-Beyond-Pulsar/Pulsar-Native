//! Stub gizmo types for compatibility
//! Gizmos are removed but these types are kept to prevent breakage

use glam::{Vec2, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BevyGizmoType {
    #[default]
    None,
    Translate,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BevyGizmoAxis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Default)]
pub struct GizmoStateResource {
    pub gizmo_type: BevyGizmoType,
    pub selected_object_id: Option<u64>,
    pub target_position: [f32; 3],
}

#[derive(Debug, Clone, Default)]
pub struct ViewportMouseInput {
    pub position: Vec2,
    pub left_button: bool,
    pub right_button: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GizmoInteractionState {
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct ActiveRaycastTask;

#[derive(Debug, Clone)]
pub struct RaycastResult {
    pub hit: bool,
    pub position: Vec3,
}
