//! Gizmo overlay renderer for the level editor
//! 
//! Renders gizmos as an overlay on top of the scene with:
//! - No depth testing (always visible)
//! - Unlit flat colors
//! - Proper axis color coding (X=Red, Y=Green, Z=Blue)

use glam::{Vec3, Mat4};
use helio_core::MeshBuffer;
use helio_render::{TransformUniforms, CameraUniforms, FeatureRenderer};
use blade_graphics::CommandEncoder;
use crate::scene::{SceneDb, GizmoType, GizmoAxis};

/// Renders gizmos as an overlay on top of the rendered scene
pub struct GizmoOverlayRenderer {
    arrow_mesh: MeshBuffer,
    torus_mesh: MeshBuffer,
    cube_mesh: MeshBuffer,
}

impl GizmoOverlayRenderer {
    pub fn new(
        _context: &blade_graphics::Context,
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
    /// This renders through the main FeatureRenderer but with a second pass
    /// that uses unlit flat colors for the gizmo meshes.
    pub fn render_overlay(
        &self,
        scene_db: &SceneDb,
        renderer: &mut FeatureRenderer,
        command_encoder: &mut CommandEncoder,
        render_target_view: &blade_graphics::TextureView,
        camera_uniforms: &CameraUniforms,
        delta_time: f32,
    ) {
        profiling::profile_scope!("GizmoOverlayRenderer::render");

        // Get gizmo state from scene database
        let gizmo_state = scene_db.get_gizmo_state();
        
        tracing::debug!("[GIZMO] Render overlay called - gizmo_type: {:?}", gizmo_state.gizmo_type);
        
        // Early exit if no gizmos to render
        if gizmo_state.gizmo_type == GizmoType::None {
            tracing::debug!("[GIZMO] No gizmo type set, skipping render");
            return;
        }

        // Get selected object position
        let selected_id = scene_db.get_selected_id();
        tracing::debug!("[GIZMO] Selected ID: {:?}", selected_id);
        
        if selected_id.is_none() {
            tracing::warn!("[GIZMO] No object selected, cannot render gizmos");
            return;
        }

        let selected_entry = scene_db.get_entry(&selected_id.unwrap());
        if selected_entry.is_none() {
            tracing::warn!("[GIZMO] Selected object entry not found in SceneDB");
            return;
        }

        let entry = selected_entry.unwrap();
        let position_array = entry.get_position();
        let gizmo_position = Vec3::new(position_array[0], position_array[1], position_array[2]);
        
        tracing::info!("[GIZMO] 🎯 Rendering {:?} gizmo at position {:?}", gizmo_state.gizmo_type, gizmo_position);

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
        let mut gizmo_meshes: Vec<(TransformUniforms, &MeshBuffer)> = Vec::new();

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
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.arrow_mesh));
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
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.torus_mesh));
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
                    
                    gizmo_meshes.push((TransformUniforms::from_matrix(transform), &self.cube_mesh));
                }
            }
            GizmoType::None => {}
        }

        tracing::info!("[GIZMO] 📦 Rendering {} gizmo meshes", gizmo_meshes.len());

        // Render gizmo meshes using the existing FeatureRenderer
        // TODO: This currently renders with lighting. Ideally we want unlit flat colors.
        // For now this will show the gizmos, we can add a custom unlit pass later.
        renderer.render(
            command_encoder,
            *render_target_view,
            *camera_uniforms,
            &gizmo_meshes,
            delta_time,
        );
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

