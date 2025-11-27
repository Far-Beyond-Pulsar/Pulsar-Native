//! Game object synchronization between game thread and render thread

use bevy::prelude::*;
use std::sync::atomic::Ordering;
use crate::subsystems::render::bevy_renderer::core::{
    GameObjectId, GameThreadResource, SharedGizmoStateResource, SharedViewportMouseInputResource,
    CameraInputResource, MainCamera,
};
use crate::subsystems::render::bevy_renderer::gizmos::rendering::GizmoStateResource;
use crate::subsystems::render::bevy_renderer::interaction::viewport::ViewportMouseInput;

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

/// Update camera viewport and resize render textures based on GPUI viewport bounds
/// This ensures Bevy renders at the exact resolution of the viewport
/// 
/// Since Bevy 0.17.2's Viewport API is private, we resize the render target instead.
/// This gives us the correct resolution rendering.
pub fn update_camera_viewport_system(
    camera_input_res: Res<CameraInputResource>,
    mut camera_query: Query<&mut Camera, With<MainCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Read viewport bounds from shared camera input
    let Ok(camera_input) = camera_input_res.0.try_lock() else {
        return; // Can't lock, skip this frame
    };

    // Round to integer pixels for exact pixel alignment
    let width = camera_input.viewport_width.round() as u32;
    let height = camera_input.viewport_height.round() as u32;

    // Only update if dimensions are valid and reasonable
    if width < 64 || height < 64 || width > 8192 || height > 8192 {
        return; // Invalid or extreme dimensions
    }

    // Update camera render target size if it has changed
    for mut camera in camera_query.iter_mut() {
        // Get the current render target - extract the actual image handle
        let current_handle = match &camera.target {
            bevy::camera::RenderTarget::Image(img_rt) => {
                // ImageRenderTarget contains the Handle<Image>
                img_rt.handle.clone()
            }
            _ => continue, // Not an image target, skip
        };
        
        // Check if we need to resize
        if let Some(image) = images.get(&current_handle) {
                let current_width = image.texture_descriptor.size.width;
                let current_height = image.texture_descriptor.size.height;
                
                // Only resize if dimensions changed significantly (avoid constant resizing)
                if (current_width as i32 - width as i32).abs() > 2 || 
                   (current_height as i32 - height as i32).abs() > 2 {
                    
                    // Log resize
                    static LAST_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                    let last = LAST_LOG.load(Ordering::Relaxed);
                    if now - last > 1 { // Log at most once per second
                        // TODO: Better logo levels later
                        // println!("[BEVY-VIEWPORT] 🔄 Resizing render target: {}x{} -> {}x{}", 
                        //         current_width, current_height, width, height);
                        // LAST_LOG.store(now, Ordering::Relaxed);
                    }
                    
                    // Create new image with correct size
                    let bytes_per_pixel = 4; // BGRA8
                    let texture_size = (width * height * bytes_per_pixel) as usize;
                    
                    let mut new_image = Image {
                        texture_descriptor: bevy::render::render_resource::TextureDescriptor {
                            label: Some("Resized Render Target"),
                            size: bevy::render::render_resource::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: bevy::render::render_resource::TextureDimension::D2,
                            format: bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
                            usage: bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT 
                                 | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        ..default()
                    };
                    new_image.data = Some(vec![0u8; texture_size]);
                    
                    // Replace the old image with the new one (reuses the same handle)
                    let new_handle = images.add(new_image);
                    camera.target = bevy::camera::RenderTarget::Image(new_handle.into());
                }
            }
    }
}
