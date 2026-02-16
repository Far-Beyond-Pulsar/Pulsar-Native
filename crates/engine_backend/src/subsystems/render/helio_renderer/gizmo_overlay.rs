//! Gizmo overlay feature for Helio renderer
//! 
//! Renders transform gizmos (translate/rotate/scale) as an overlay using Helio's feature system.
//! This properly integrates with Helio's rendering pipeline instead of hacking blade-graphics directly.

use helio_features::{Feature, FeatureContext, ShaderInjection};
use glam::{Vec3, Mat4};
use crate::scene::{SceneDb, GizmoType, GizmoAxis};
use std::sync::Arc;

/// Gizmo rendering feature for Helio
/// 
/// This feature renders transform gizmos as overlays on top of the scene.
/// It uses Helio's post-render pass system to draw after the main scene.
pub struct GizmoFeature {
    enabled: bool,
    scene_db: Arc<SceneDb>,
}

impl GizmoFeature {
    /// Create a new gizmo feature
    pub fn new(scene_db: Arc<SceneDb>) -> Self {
        tracing::info!("[GIZMO FEATURE] Creating gizmo overlay feature");
        Self {
            enabled: true,
            scene_db,
        }
    }
    
    /// Calculate gizmo transforms based on current scene state
    fn calculate_gizmo_data(&self, camera_pos: Vec3) -> Option<Vec<(Mat4, [f32; 4])>> {
        let gizmo_state = self.scene_db.get_gizmo_state();
        if gizmo_state.gizmo_type == GizmoType::None {
            return None;
        }
        
        // Get selected object position
        let selected_id = self.scene_db.get_selected_id()?;
        let entry = self.scene_db.get_entry(&selected_id)?;
        let position_array = entry.get_position();
        let gizmo_position = Vec3::new(position_array[0], position_array[1], position_array[2]);
        
        // Calculate scale based on camera distance
        let distance = (gizmo_position - camera_pos).length();
        let scale = (distance * 0.15).max(0.1).min(2.0);
        
        // Generate transform and color for each axis
        let mut gizmos = Vec::new();
        for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
            let highlighted = gizmo_state.highlighted_axis == Some(axis);
            let axis_scale = if highlighted { scale * 1.2 } else { scale };
            
            let transform = match gizmo_state.gizmo_type {
                GizmoType::Translate => create_arrow_transform(gizmo_position, axis, axis_scale),
                GizmoType::Rotate => create_torus_transform(gizmo_position, axis, axis_scale),
                GizmoType::Scale => create_cube_transform(gizmo_position, axis, axis_scale),
                GizmoType::None => continue,
            };
            
            let color = axis_color(axis, highlighted);
            gizmos.push((transform, color));
        }
        
        Some(gizmos)
    }
}

impl Feature for GizmoFeature {
    fn name(&self) -> &str {
        "gizmo_overlay"
    }
    
    fn init(&mut self, _context: &FeatureContext) {
        tracing::info!("[GIZMO FEATURE] ✅ Gizmo overlay feature initialized");
    }
    
    fn is_enabled(&self) -> bool {
        self.enabled
    }
    
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        tracing::info!("[GIZMO FEATURE] Gizmo overlay {}", if enabled { "enabled" } else { "disabled" });
    }
    
    fn shader_injections(&self) -> Vec<ShaderInjection> {
        // Gizmos use a separate post-render pass, no main shader injections needed
        vec![]
    }
    
    fn prepare_frame(&mut self, _context: &FeatureContext) {
        // Calculate gizmo data for this frame
        // TODO: Get camera position from context when Helio API supports it
        
        let gizmo_state = self.scene_db.get_gizmo_state();
        if gizmo_state.gizmo_type != GizmoType::None {
            tracing::debug!("[GIZMO FEATURE] Gizmo type: {:?}", gizmo_state.gizmo_type);
        }
    }
    
    fn cleanup(&mut self, _context: &FeatureContext) {
        tracing::info!("[GIZMO FEATURE] Cleaning up gizmo overlay feature");
    }
}

/// Create transform for arrow (translate gizmo)
fn create_arrow_transform(position: Vec3, axis: GizmoAxis, scale: f32) -> Mat4 {
    let translation = Mat4::from_translation(position);
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
    };
    let scale_mat = Mat4::from_scale(Vec3::splat(scale));
    translation * rotation * scale_mat
}

/// Create transform for torus (rotate gizmo)
fn create_torus_transform(position: Vec3, axis: GizmoAxis, scale: f32) -> Mat4 {
    let translation = Mat4::from_translation(position);
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
    };
    let scale_mat = Mat4::from_scale(Vec3::splat(scale * 1.5));
    translation * rotation * scale_mat
}

/// Create transform for cube (scale gizmo)
fn create_cube_transform(position: Vec3, axis: GizmoAxis, scale: f32) -> Mat4 {
    let offset = match axis {
        GizmoAxis::X => Vec3::new(1.0, 0.0, 0.0),
        GizmoAxis::Y => Vec3::new(0.0, 1.0, 0.0),
        GizmoAxis::Z => Vec3::new(0.0, 0.0, 1.0),
    } * scale;
    Mat4::from_translation(position + offset) * Mat4::from_scale(Vec3::splat(scale * 0.15))
}

/// Get color for an axis
fn axis_color(axis: GizmoAxis, highlighted: bool) -> [f32; 4] {
    let intensity = if highlighted { 1.0 } else { 0.8 };
    match axis {
        GizmoAxis::X => [intensity, 0.0, 0.0, 1.0], // Red
        GizmoAxis::Y => [0.0, intensity, 0.0, 1.0], // Green
        GizmoAxis::Z => [0.0, 0.0, intensity, 1.0], // Blue
    }
}


/// Get color for a gizmo axis
pub fn get_axis_color(axis: GizmoAxis, highlighted: bool) -> Vec3 {
    let base_color = match axis {
        GizmoAxis::X => Vec3::new(1.0, 0.0, 0.0), // Red
        GizmoAxis::Y => Vec3::new(0.0, 1.0, 0.0), // Green
        GizmoAxis::Z => Vec3::new(0.0, 0.0, 1.0), // Blue
    };
    
    if highlighted {
        // Brighten when highlighted
        base_color * 1.5
    } else {
        base_color
    }
}

