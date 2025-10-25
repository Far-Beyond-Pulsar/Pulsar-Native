//! Game object synchronization between game thread and render thread

use bevy::prelude::*;
use super::components::GameObjectId;
use super::resources::{GameThreadResource, SharedGizmoStateResource, SharedViewportMouseInputResource};
use super::gizmos_bevy::GizmoStateResource;
use super::viewport_interaction::ViewportMouseInput;

/// Sync gizmo state from GPUI (shared Arc<Mutex<>>) to Bevy's ECS resource
/// This allows GPUI to control gizmo type, selection, etc.
pub fn sync_gizmo_state_system(
    shared_gizmo: Res<SharedGizmoStateResource>,
    mut bevy_gizmo: ResMut<GizmoStateResource>,
) {
    // Try to lock shared state (non-blocking)
    if let Ok(shared) = shared_gizmo.0.try_lock() {
        // Copy all fields from shared to Bevy's resource
        bevy_gizmo.gizmo_type = shared.gizmo_type;
        bevy_gizmo.active_axis = shared.active_axis;
        bevy_gizmo.target_position = shared.target_position;
        bevy_gizmo.enabled = shared.enabled;
        bevy_gizmo.selected_object_id = shared.selected_object_id.clone();
    }
}

/// Sync viewport mouse input from GPUI to Bevy for raycast selection
pub fn sync_viewport_mouse_input_system(
    shared_mouse: Res<SharedViewportMouseInputResource>,
    mut bevy_mouse: ResMut<ViewportMouseInput>,
) {
    // Try to lock shared state (non-blocking) - parking_lot returns Option, not Result
    if let Some(shared) = shared_mouse.0.try_lock() {
        // Copy all mouse input fields
        bevy_mouse.mouse_pos = shared.mouse_pos;
        bevy_mouse.left_clicked = shared.left_clicked;
        bevy_mouse.left_down = shared.left_down;
        bevy_mouse.mouse_delta = shared.mouse_delta;
        
        // Debug log when click is detected
        if shared.left_clicked {
            println!("[BEVY-SYNC] 🖱️ Mouse click synced: pos=({:.3}, {:.3})", shared.mouse_pos.x, shared.mouse_pos.y);
        }
    }
}


/// Sync game thread object positions/rotations to Bevy entities
/// This system reads from the game thread state and updates matching Bevy transforms
pub fn sync_game_objects_system(
    game_thread: Res<GameThreadResource>,
    mut query: Query<(&GameObjectId, &mut Transform)>,
) {
    // Get game state if available
    let Some(ref game_state_arc) = game_thread.0 else {
        return; // No game thread connected
    };

    // Try to lock game state (non-blocking)
    let Ok(game_state) = game_state_arc.try_lock() else {
        return; // Game thread busy, skip this frame
    };

    // Update all entities that have a GameObjectId
    for (game_obj_id, mut transform) in query.iter_mut() {
        if let Some(game_obj) = game_state.get_object(game_obj_id.0) {
            // Sync position
            transform.translation = Vec3::new(
                game_obj.position[0],
                game_obj.position[1],
                game_obj.position[2],
            );

            // Sync rotation (convert degrees to radians)
            transform.rotation = Quat::from_euler(
                EulerRot::XYZ,
                game_obj.rotation[0].to_radians(),
                game_obj.rotation[1].to_radians(),
                game_obj.rotation[2].to_radians(),
            );

            // Sync scale
            transform.scale = Vec3::new(
                game_obj.scale[0],
                game_obj.scale[1],
                game_obj.scale[2],
            );
        }
    }
}

/// Update gizmo target position to follow selected object
/// This ensures the gizmo stays centered on the selected object even when it moves
pub fn update_gizmo_target_system(
    mut gizmo_state: ResMut<GizmoStateResource>,
    objects: Query<(&GameObjectId, &Transform)>,
) {
    // Only update if a gizmo tool is active and an object is selected
    if !gizmo_state.enabled || gizmo_state.selected_object_id.is_none() {
        return;
    }
    
    // Get the selected object ID
    let Some(ref selected_id) = gizmo_state.selected_object_id else {
        return;
    };
    
    // Map string IDs to numeric IDs (matching our current setup)
    let numeric_id = match selected_id.as_str() {
        "red_cube" => Some(1),
        "blue_sphere" => Some(2),
        "gold_sphere" => Some(3),
        _ => selected_id.parse::<u64>().ok(),
    };
    
    // Find the object and update gizmo position
    if let Some(id) = numeric_id {
        for (game_obj_id, transform) in objects.iter() {
            if game_obj_id.0 == id {
                gizmo_state.target_position = transform.translation;
                break;
            }
        }
    }
}
