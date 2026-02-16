//! Gizmo interaction controller for handling mouse-based gizmo manipulation
//! 
//! Handles:
//! - Mouse hover detection (raycasting to gizmo axes)
//! - Click and drag operations
//! - Transform delta calculations
//! - Applying transforms to selected objects

use glam::{Vec2, Vec3, Mat4, Quat};
use crate::scene::{SceneDb, GizmoType, GizmoAxis};
use crate::services::{PhysicsQueryService, ColliderTag};
use std::sync::Arc;

/// State of gizmo interaction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionState {
    Idle,
    Hovering(GizmoAxis),
    Dragging {
        axis: GizmoAxis,
        gizmo_type: GizmoType,
    },
}

/// Stores state for active drag operation
#[derive(Debug, Clone)]
pub struct DragState {
    pub axis: GizmoAxis,
    pub gizmo_type: GizmoType,
    pub initial_position: Vec3,
    pub initial_rotation: Vec3,
    pub initial_scale: Vec3,
    pub drag_start_screen_pos: Vec2,
    pub drag_plane_origin: Vec3,
    pub drag_plane_normal: Vec3,
}

/// Controller for gizmo interactions
pub struct GizmoInteractionController {
    interaction_state: InteractionState,
    drag_state: Option<DragState>,
    physics_query: Arc<PhysicsQueryService>,
    scene_db: Arc<SceneDb>,
}

impl GizmoInteractionController {
    pub fn new(
        physics_query: Arc<PhysicsQueryService>,
        scene_db: Arc<SceneDb>,
    ) -> Self {
        Self {
            interaction_state: InteractionState::Idle,
            drag_state: None,
            physics_query,
            scene_db,
        }
    }

    /// Update gizmo hover state based on mouse position
    /// 
    /// # Arguments
    /// * `mouse_pos` - Normalized screen coordinates (0-1 range)
    /// * `camera_*` - Camera parameters for raycasting
    pub fn update_hover(
        &mut self,
        mouse_pos: Vec2,
        camera_position: Vec3,
        camera_forward: Vec3,
        camera_right: Vec3,
        camera_up: Vec3,
        fov: f32,
        aspect_ratio: f32,
    ) {
        // Don't update hover during drag
        if matches!(self.interaction_state, InteractionState::Dragging { .. }) {
            return;
        }

        // Get gizmo state
        let gizmo_state = self.scene_db.get_gizmo_state();
        if gizmo_state.gizmo_type == GizmoType::None {
            self.interaction_state = InteractionState::Idle;
            self.scene_db.set_gizmo_highlighted_axis(None);
            return;
        }

        // Raycast to detect gizmo hover
        let ray_origin = camera_position;
        let ndc_x = (mouse_pos.x * 2.0) - 1.0;
        let ndc_y = 1.0 - (mouse_pos.y * 2.0);
        let half_height = (fov / 2.0).tan();
        let half_width = half_height * aspect_ratio;
        let ray_dir = (camera_forward 
            + camera_right * (ndc_x * half_width)
            + camera_up * (ndc_y * half_height)).normalize();

        // Check if we hit a gizmo
        if let Some(collider_tag) = self.physics_query.raycast_gizmo(ray_origin, ray_dir, 1000.0) {
            let axis = match collider_tag {
                ColliderTag::GizmoAxisX => GizmoAxis::X,
                ColliderTag::GizmoAxisY => GizmoAxis::Y,
                ColliderTag::GizmoAxisZ => GizmoAxis::Z,
                _ => return,
            };

            self.interaction_state = InteractionState::Hovering(axis);
            self.scene_db.set_gizmo_highlighted_axis(Some(axis));
        } else {
            self.interaction_state = InteractionState::Idle;
            self.scene_db.set_gizmo_highlighted_axis(None);
        }
    }

    /// Start a drag operation
    pub fn start_drag(
        &mut self,
        mouse_pos: Vec2,
        camera_position: Vec3,
        camera_forward: Vec3,
        camera_right: Vec3,
        camera_up: Vec3,
    ) -> bool {
        // Can only start drag if hovering
        let axis = match self.interaction_state {
            InteractionState::Hovering(axis) => axis,
            _ => return false,
        };

        // Get selected object
        let selected_id = self.scene_db.get_selected_id();
        if selected_id.is_none() {
            return false;
        }

        let entry = self.scene_db.get_entry(&selected_id.unwrap());
        if entry.is_none() {
            return false;
        }

        let entry = entry.unwrap();
        let gizmo_state = self.scene_db.get_gizmo_state();
        
        // Store initial transform
        let initial_position_arr = entry.get_position();
        let initial_rotation_arr = entry.get_rotation();
        let initial_scale_arr = entry.get_scale();

        let initial_position = Vec3::new(
            initial_position_arr[0],
            initial_position_arr[1],
            initial_position_arr[2],
        );

        // Calculate drag plane
        let (plane_origin, plane_normal) = calculate_drag_plane(
            initial_position,
            axis,
            gizmo_state.gizmo_type,
            camera_position,
            camera_forward,
        );

        self.drag_state = Some(DragState {
            axis,
            gizmo_type: gizmo_state.gizmo_type,
            initial_position,
            initial_rotation: Vec3::new(
                initial_rotation_arr[0],
                initial_rotation_arr[1],
                initial_rotation_arr[2],
            ),
            initial_scale: Vec3::new(
                initial_scale_arr[0],
                initial_scale_arr[1],
                initial_scale_arr[2],
            ),
            drag_start_screen_pos: mouse_pos,
            drag_plane_origin: plane_origin,
            drag_plane_normal: plane_normal,
        });

        self.interaction_state = InteractionState::Dragging {
            axis,
            gizmo_type: gizmo_state.gizmo_type,
        };

        true
    }

    /// Update drag operation
    pub fn update_drag(
        &mut self,
        mouse_pos: Vec2,
        camera_position: Vec3,
        camera_forward: Vec3,
        camera_right: Vec3,
        camera_up: Vec3,
        fov: f32,
        aspect_ratio: f32,
    ) {
        if !matches!(self.interaction_state, InteractionState::Dragging { .. }) {
            return;
        }

        let drag_state = match &self.drag_state {
            Some(state) => state.clone(),
            None => return,
        };

        // Get selected object
        let selected_id = match self.scene_db.get_selected_id() {
            Some(id) => id,
            None => return,
        };

        // Calculate ray from mouse position
        let ndc_x = (mouse_pos.x * 2.0) - 1.0;
        let ndc_y = 1.0 - (mouse_pos.y * 2.0);
        let half_height = (fov / 2.0).tan();
        let half_width = half_height * aspect_ratio;
        let ray_dir = (camera_forward 
            + camera_right * (ndc_x * half_width)
            + camera_up * (ndc_y * half_height)).normalize();

        // Intersect ray with drag plane
        let hit_point = intersect_ray_plane(
            camera_position,
            ray_dir,
            drag_state.drag_plane_origin,
            drag_state.drag_plane_normal,
        );

        if let Some(hit) = hit_point {
            // Calculate transform delta based on gizmo type
            match drag_state.gizmo_type {
                GizmoType::Translate => {
                    // Calculate movement along axis
                    let delta = hit - drag_state.initial_position;
                    let axis_vec = match drag_state.axis {
                        GizmoAxis::X => Vec3::X,
                        GizmoAxis::Y => Vec3::Y,
                        GizmoAxis::Z => Vec3::Z,
                    };
                    
                    // Project delta onto axis
                    let movement = delta.dot(axis_vec) * axis_vec;
                    let new_position = drag_state.initial_position + movement;
                    
                    self.scene_db.set_position(
                        &selected_id,
                        [new_position.x, new_position.y, new_position.z],
                    );
                }
                GizmoType::Rotate => {
                    // Calculate rotation delta
                    let to_start = (drag_state.drag_plane_origin - drag_state.initial_position).normalize();
                    let to_current = (hit - drag_state.initial_position).normalize();
                    
                    // Calculate angle between vectors
                    let cos_angle = to_start.dot(to_current).clamp(-1.0, 1.0);
                    let angle = cos_angle.acos();
                    
                    // Determine sign based on cross product
                    let axis_vec = match drag_state.axis {
                        GizmoAxis::X => Vec3::X,
                        GizmoAxis::Y => Vec3::Y,
                        GizmoAxis::Z => Vec3::Z,
                    };
                    let cross = to_start.cross(to_current);
                    let sign = if cross.dot(axis_vec) < 0.0 { -1.0 } else { 1.0 };
                    
                    let rotation_delta = sign * angle.to_degrees();
                    
                    // Apply rotation to initial rotation
                    let mut new_rotation = drag_state.initial_rotation;
                    match drag_state.axis {
                        GizmoAxis::X => new_rotation.x += rotation_delta,
                        GizmoAxis::Y => new_rotation.y += rotation_delta,
                        GizmoAxis::Z => new_rotation.z += rotation_delta,
                    }
                    
                    self.scene_db.set_rotation(
                        &selected_id,
                        [new_rotation.x, new_rotation.y, new_rotation.z],
                    );
                }
                GizmoType::Scale => {
                    // Calculate scale delta
                    let delta = hit - drag_state.initial_position;
                    let axis_vec = match drag_state.axis {
                        GizmoAxis::X => Vec3::X,
                        GizmoAxis::Y => Vec3::Y,
                        GizmoAxis::Z => Vec3::Z,
                    };
                    
                    let scale_delta = delta.dot(axis_vec);
                    let scale_multiplier = 1.0 + scale_delta;
                    
                    let mut new_scale = drag_state.initial_scale;
                    match drag_state.axis {
                        GizmoAxis::X => new_scale.x *= scale_multiplier.max(0.01),
                        GizmoAxis::Y => new_scale.y *= scale_multiplier.max(0.01),
                        GizmoAxis::Z => new_scale.z *= scale_multiplier.max(0.01),
                    }
                    
                    self.scene_db.set_scale(
                        &selected_id,
                        [new_scale.x, new_scale.y, new_scale.z],
                    );
                }
                GizmoType::None => {}
            }
        }
    }

    /// End drag operation
    pub fn end_drag(&mut self) {
        self.drag_state = None;
        self.interaction_state = InteractionState::Idle;
        self.scene_db.set_gizmo_highlighted_axis(None);
    }

    /// Get current interaction state
    pub fn interaction_state(&self) -> InteractionState {
        self.interaction_state
    }
}

/// Calculate the drag plane for a gizmo operation
fn calculate_drag_plane(
    object_position: Vec3,
    axis: GizmoAxis,
    gizmo_type: GizmoType,
    camera_position: Vec3,
    camera_forward: Vec3,
) -> (Vec3, Vec3) {
    let axis_vec = match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
    };

    match gizmo_type {
        GizmoType::Translate => {
            // For translation, use a plane perpendicular to camera that contains the axis
            let to_camera = (camera_position - object_position).normalize();
            let plane_normal = to_camera.cross(axis_vec).normalize();
            
            // If axis is parallel to camera, use a perpendicular plane
            let plane_normal = if plane_normal.length() < 0.01 {
                // Find any perpendicular vector
                if axis_vec.x.abs() < 0.9 {
                    Vec3::X.cross(axis_vec).normalize()
                } else {
                    Vec3::Y.cross(axis_vec).normalize()
                }
            } else {
                plane_normal
            };
            
            (object_position, plane_normal)
        }
        GizmoType::Rotate => {
            // For rotation, use a plane perpendicular to the rotation axis
            (object_position, axis_vec)
        }
        GizmoType::Scale => {
            // For scale, use same logic as translate
            let to_camera = (camera_position - object_position).normalize();
            let plane_normal = to_camera.cross(axis_vec).normalize();
            
            let plane_normal = if plane_normal.length() < 0.01 {
                if axis_vec.x.abs() < 0.9 {
                    Vec3::X.cross(axis_vec).normalize()
                } else {
                    Vec3::Y.cross(axis_vec).normalize()
                }
            } else {
                plane_normal
            };
            
            (object_position, plane_normal)
        }
        GizmoType::None => (object_position, Vec3::Y),
    }
}

/// Intersect a ray with a plane
fn intersect_ray_plane(
    ray_origin: Vec3,
    ray_direction: Vec3,
    plane_origin: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let denom = plane_normal.dot(ray_direction);
    
    // Ray is parallel to plane
    if denom.abs() < 0.0001 {
        return None;
    }
    
    let t = (plane_origin - ray_origin).dot(plane_normal) / denom;
    
    // Intersection is behind the ray origin
    if t < 0.0 {
        return None;
    }
    
    Some(ray_origin + ray_direction * t)
}
