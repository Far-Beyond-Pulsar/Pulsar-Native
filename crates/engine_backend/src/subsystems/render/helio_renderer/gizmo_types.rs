//! Gizmo stub types for compatibility
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
    pub selected_object_id: Option<String>,
    pub target_position: Vec3,
    pub enabled: bool,
    pub updated_object_id: Option<String>,
    pub updated_transform: Option<Vec3>,
}

#[derive(Debug, Clone, Default)]
pub struct ViewportMouseInput {
    pub position: Vec2,
    pub mouse_pos: Vec2,
    pub mouse_delta: Vec2,
    pub left_button: bool,
    pub left_clicked: bool,
    pub left_down: bool,
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
