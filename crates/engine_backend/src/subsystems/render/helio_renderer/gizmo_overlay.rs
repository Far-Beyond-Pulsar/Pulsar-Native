//! Gizmo overlay renderer for the level editor
//! 
//! Renders gizmos as an overlay on top of the scene with:
//! - No depth testing (always visible)
//! - Unlit flat colors
//! - Proper axis color coding (X=Red, Y=Green, Z=Blue)

use glam::{Vec3, Mat4};
use helio_core::MeshBuffer;
use helio_render::TransformUniforms;
use blade_graphics::{CommandEncoder, TextureView};
use crate::scene::{SceneDb, GizmoType, GizmoAxis};

/// Renders gizmos as an overlay on top of the rendered scene
pub struct GizmoOverlayRenderer {
    arrow_mesh: MeshBuffer,
    torus_mesh: MeshBuffer,
    cube_mesh: MeshBuffer,
}

impl GizmoOverlayRenderer {
    pub fn new(
        context: &blade_graphics::Context,
        arrow_mesh: MeshBuffer,
        torus_mesh: MeshBuffer,
        cube_mesh: MeshBuffer,
    ) -> Self {
        Self {
            arrow_mesh,
            torus_mesh,
            cube_mesh,
        }
    }

    /// Render gizmos as an overlay on top of the scene
    /// 
    /// This should be called AFTER the main scene rendering is complete,
    /// but before presenting the final image.
    pub fn render_overlay(
        &self,
        scene_db: &SceneDb,
        command_encoder: &mut CommandEncoder,
        render_target_view: &TextureView,
        camera_uniforms: &helio_render::CameraUniforms,
        context: &blade_graphics::Context,
    ) {
        profiling::profile_scope!("GizmoOverlayRenderer::render");

        // Get gizmo state from scene database
        let gizmo_state = scene_db.get_gizmo_state();
        
        // Early exit if no gizmos to render
        if gizmo_state.gizmo_type == GizmoType::None {
            return;
        }

        // Get selected object position
        let selected_id = scene_db.get_selected_id();
        if selected_id.is_none() {
            return;
        }

        let selected_entry = scene_db.get_entry(&selected_id.unwrap());
        if selected_entry.is_none() {
            return;
        }

        let entry = selected_entry.unwrap();
        let position_array = entry.get_position();
        let gizmo_position = Vec3::new(position_array[0], position_array[1], position_array[2]);

        // Calculate camera distance for scaling
        let camera_pos = Vec3::new(
            camera_uniforms.position[0],
            camera_uniforms.position[1],
            camera_uniforms.position[2],
        );
        let distance = (gizmo_position - camera_pos).length();
        let scale = (distance * 0.15).max(0.1).min(2.0); // Screen-space scaling

        // Apply user scale factor from gizmo state
        let final_scale = scale * gizmo_state.scale_factor;

        // Prepare gizmo meshes to render
        let mut gizmo_meshes = Vec::new();

        match gizmo_state.gizmo_type {
            GizmoType::Translate => {
                // Render 3 arrows for X, Y, Z
                for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                    let transform = create_gizmo_arrow_transform(
                        gizmo_position,
                        axis,
                        final_scale,
                        gizmo_state.highlighted_axis == Some(axis),
                    );
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.arrow_mesh, axis));
                }
            }
            GizmoType::Rotate => {
                // Render 3 toruses for X, Y, Z rotation
                for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                    let transform = create_gizmo_torus_transform(
                        gizmo_position,
                        axis,
                        final_scale,
                        gizmo_state.highlighted_axis == Some(axis),
                    );
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.torus_mesh, axis));
                }
            }
            GizmoType::Scale => {
                // Render 3 cubes for X, Y, Z scale handles
                for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                    let offset = match axis {
                        GizmoAxis::X => Vec3::new(1.0, 0.0, 0.0),
                        GizmoAxis::Y => Vec3::new(0.0, 1.0, 0.0),
                        GizmoAxis::Z => Vec3::new(0.0, 0.0, 1.0),
                    } * final_scale;

                    let highlight = gizmo_state.highlighted_axis == Some(axis);
                    let cube_scale = if highlight { 0.25 } else { 0.2 } * final_scale;
                    
                    let transform = Mat4::from_translation(gizmo_position + offset)
                        * Mat4::from_scale(Vec3::splat(cube_scale));
                    
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.cube_mesh, axis));
                }
            }
            GizmoType::None => {}
        }

        // TODO: Render gizmo meshes with:
        // - Depth testing disabled (or depth read only, no write)
        // - Unlit flat colors based on axis
        // - Simple shader that just colors based on axis (no lighting)
        //
        // For now, this is a placeholder. Full implementation requires:
        // 1. Custom gizmo shader (vertex + fragment)
        // 2. Pipeline with depth testing disabled
        // 3. Render pass that draws after main scene
        //
        // This will be implemented in the next iteration.
    }
}

/// Create transform matrix for a gizmo arrow along a specific axis
fn create_gizmo_arrow_transform(position: Vec3, axis: GizmoAxis, scale: f32, highlighted: bool) -> Mat4 {
    let translation = Mat4::from_translation(position);
    
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
    };
    
    let scale_factor = if highlighted { scale * 1.2 } else { scale };
    let scale_mat = Mat4::from_scale(Vec3::splat(scale_factor));
    
    translation * rotation * scale_mat
}

/// Create transform matrix for a rotation torus along a specific axis
fn create_gizmo_torus_transform(position: Vec3, axis: GizmoAxis, scale: f32, highlighted: bool) -> Mat4 {
    let translation = Mat4::from_translation(position);
    
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
    };
    
    let scale_factor = if highlighted { scale * 1.2 } else { scale };
    let scale_mat = Mat4::from_scale(Vec3::splat(scale_factor));
    
    translation * rotation * scale_mat
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
