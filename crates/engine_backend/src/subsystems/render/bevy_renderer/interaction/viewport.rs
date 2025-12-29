//! Viewport Interaction System
//! 
//! Handles mouse interaction with the viewport for:
//! - Object selection via raycasting (FULLY ASYNC with Bevy's AsyncComputeTaskPool)
//! - Gizmo manipulation (drag to transform objects)
//! - Camera-independent picking

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use std::sync::{Arc, Mutex};
use crate::subsystems::render::bevy_renderer::core::{GameObjectId, MainCamera};
use crate::subsystems::render::bevy_renderer::gizmos::rendering::{
    GizmoStateResource, GizmoVisual, GizmoAxis, GizmoType,
};
use crate::subsystems::render::bevy_renderer::core::SharedGizmoStateResource;

/// Mouse input for viewport interaction (set from GPUI)
#[derive(Resource, Default, Clone)]
pub struct ViewportMouseInput {
    /// Mouse position in viewport space (0,0 = top-left, normalized 0-1)
    pub mouse_pos: Vec2,
    /// Left button just pressed
    pub left_clicked: bool,
    /// Left button is held down
    pub left_down: bool,
    /// Mouse delta since last frame (for dragging)
    pub mouse_delta: Vec2,
}

/// State for active gizmo manipulation
#[derive(Resource, Default)]
pub struct GizmoInteractionState {
    pub is_dragging: bool,
    pub drag_axis: Option<GizmoAxis>,
    pub drag_start_pos: Vec2,
    pub drag_start_transform: Option<Transform>,
    pub drag_start_object_id: Option<u64>,
}

/// Result of async raycast operation
#[derive(Debug, Clone)]
pub struct RaycastResult {
    pub selected_id: Option<u64>,
    pub hit_distance: Option<f32>,
}

/// Active raycast task running on worker thread
#[derive(Resource, Default)]
pub struct ActiveRaycastTask {
    pub task: Option<Task<RaycastResult>>,
}

/// System to initiate async raycast on click (spawns Bevy async task)
pub fn viewport_click_initiate_raycast_system(
    mouse_input: Res<ViewportMouseInput>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    object_query: Query<(Entity, &GameObjectId, &GlobalTransform)>,
    mut active_task: ResMut<ActiveRaycastTask>,
) {
    // Only process on NEW click (not if task already running)
    if !mouse_input.left_clicked || active_task.task.is_some() {
        return;
    }
        
    // Get camera
    let Ok((camera, camera_transform)) = camera_query.single() else {
        tracing::debug!("[RAYCAST] ⚠️ No camera found!");
        return;
    };
    
    // Create ray from mouse position
    let ray = screen_to_world_ray(
        mouse_input.mouse_pos,
        camera,
        camera_transform,
    );
    
    // Collect all objects (lightweight: just ID + position)
    let objects: Vec<(u64, Vec3)> = object_query.iter()
        .map(|(_, game_obj_id, transform)| (game_obj_id.0, transform.translation()))
        .collect();
        
    let ray_origin = ray.origin;
    let ray_direction = ray.direction;
    
    // Get Bevy's async compute task pool
    let task_pool = AsyncComputeTaskPool::get();
    
    // Spawn fully async task (runs on worker threads, ZERO frame time impact!)
    let task = task_pool.spawn(async move {
        let mut closest_hit: Option<(u64, f32)> = None;
        
        // Process ALL objects without blocking (we're on a worker thread!)
        for (obj_id, obj_pos) in objects.iter() {
            // Ray-sphere intersection test
            let ray_to_object = *obj_pos - ray_origin;
            let projection = ray_to_object.dot(*ray_direction);
            
            if projection > 0.0 {
                let closest_point = ray_origin + *ray_direction * projection;
                let dist_to_ray = closest_point.distance(*obj_pos);
                let radius = 1.5;
                
                if dist_to_ray < radius {
                    if closest_hit.is_none() || projection < closest_hit.unwrap().1 {
                        closest_hit = Some((*obj_id, projection));
                    }
                }
            }
        }
        
        RaycastResult {
            selected_id: closest_hit.map(|(id, _)| id),
            hit_distance: closest_hit.map(|(_, dist)| dist),
        }
    });
    
    active_task.task = Some(task);
}

/// System to poll async raycast and apply results (runs on Bevy main thread)
pub fn viewport_poll_raycast_system(
    mut active_task: ResMut<ActiveRaycastTask>,
    mut gizmo_state: ResMut<GizmoStateResource>,
    shared_gizmo_state: Res<SharedGizmoStateResource>,
    object_query: Query<(Entity, &GameObjectId, &GlobalTransform)>,
) {
    // Check if we have an active task
    if let Some(task) = &mut active_task.task {
        // Check if task is finished (non-blocking)
        if task.is_finished() {
            // Task completed! Take it and get result
            let task = active_task.task.take().unwrap();
            let result = block_on(task);
                // Task completed! Apply result                
                if let Some(selected_id) = result.selected_id {
                    // Convert numeric ID to string ID (matching scene database)
                    let string_id = match selected_id {
                        1 => "cube_red".to_string(),
                        2 => "sphere_blue".to_string(),
                        3 => "sphere_gold".to_string(),
                        4 => "sphere_green".to_string(),
                        _ => format!("object_{}", selected_id),
                    };
                    
                    tracing::debug!("[RAYCAST] 🎯 Selected '{}' at distance {:.3}", 
                        string_id, result.hit_distance.unwrap_or(0.0));
                    
                    // Update local Bevy ECS resource
                    gizmo_state.selected_object_id = Some(string_id.clone());
                    
                    // Update gizmo position
                    if let Some((_, _, transform)) = object_query.iter()
                        .find(|(_, id, _)| id.0 == selected_id) 
                    {
                        gizmo_state.target_position = transform.translation();
                    }
                    
                    // Update shared resource for GPUI
                    if let Ok(mut shared) = shared_gizmo_state.0.try_lock() {
                        shared.selected_object_id = Some(string_id.clone());
                        shared.target_position = gizmo_state.target_position;
                    }
                } else {
                    // No hits - deselect
                    tracing::debug!("[RAYCAST] ⭕ No hits - deselected");
                    gizmo_state.selected_object_id = None;
                    
                    if let Ok(mut shared) = shared_gizmo_state.0.try_lock() {
                        shared.selected_object_id = None;
                    }
                }
        }
        // If not finished, task stays in active_task and we check next frame
    }
}

/// System to handle gizmo dragging for object manipulation
/// Updates both Bevy ECS transforms AND syncs back to SharedGizmoStateResource for GPUI
pub fn gizmo_drag_system(
    mouse_input: Res<ViewportMouseInput>,
    gizmo_state: Res<GizmoStateResource>,
    shared_gizmo_state: Res<SharedGizmoStateResource>,
    mut interaction_state: ResMut<GizmoInteractionState>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    gizmo_query: Query<(&GizmoVisual, &GlobalTransform)>,
    mut object_query: Query<(&GameObjectId, &mut Transform)>,
) {
    // Only in edit mode with gizmo enabled
    if !gizmo_state.enabled || gizmo_state.selected_object_id.is_none() {
        interaction_state.is_dragging = false;
        return;
    }
    
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    
    // Start drag
    if mouse_input.left_clicked && !interaction_state.is_dragging {
        // Check if clicked on a gizmo axis
        let ray = screen_to_world_ray(mouse_input.mouse_pos, camera, camera_transform);
        
        // Test gizmo visuals for hit
        let mut closest_gizmo: Option<(GizmoAxis, f32)> = None;
        
        for (gizmo_visual, gizmo_transform) in gizmo_query.iter() {
            if let Some(distance) = test_ray_gizmo_hit(&ray, gizmo_visual, gizmo_transform) {
                if closest_gizmo.is_none() || distance < closest_gizmo.unwrap().1 {
                    closest_gizmo = Some((gizmo_visual.axis, distance));
                }
            }
        }
        
        if let Some((axis, _)) = closest_gizmo {
            // Start dragging this axis
            interaction_state.is_dragging = true;
            interaction_state.drag_axis = Some(axis);
            interaction_state.drag_start_pos = mouse_input.mouse_pos;
            
            // Store starting transform of selected object
            let selected_id_numeric = gizmo_state.selected_object_id.as_ref()
                .and_then(|id| match id.as_str() {
                    "cube_red" => Some(1),
                    "sphere_blue" => Some(2),
                    "sphere_gold" => Some(3),
                    "sphere_green" => Some(4),
                    _ => id.strip_prefix("object_").and_then(|n| n.parse::<u64>().ok()),
                });
            
            if let Some(id) = selected_id_numeric {
                for (game_obj_id, transform) in object_query.iter() {
                    if game_obj_id.0 == id {
                        interaction_state.drag_start_transform = Some(*transform);
                        interaction_state.drag_start_object_id = Some(id);
                        break;
                    }
                }
            }
        }
    }
    
    // Continue drag
    if interaction_state.is_dragging && mouse_input.left_down {
        if let (Some(axis), Some(start_transform), Some(obj_id)) = (
            interaction_state.drag_axis,
            interaction_state.drag_start_transform,
            interaction_state.drag_start_object_id,
        ) {
            // Find and update the object
            for (game_obj_id, mut transform) in object_query.iter_mut() {
                if game_obj_id.0 == obj_id {
                    // Apply transformation based on gizmo type and axis
                    apply_gizmo_transformation(
                        &mut transform,
                        &start_transform,
                        gizmo_state.gizmo_type,
                        axis,
                        mouse_input.mouse_delta,
                        camera_transform,
                    );

                    // CRITICAL: Sync transform back to GPUI via SharedGizmoStateResource
                    // This allows GPUI to update its scene database
                    if let Ok(mut shared) = shared_gizmo_state.0.try_lock() {
                        // Convert numeric ID to string ID for GPUI (matching scene database)
                        let string_id = match obj_id {
                            1 => "cube_red".to_string(),
                            2 => "sphere_blue".to_string(),
                            3 => "sphere_gold".to_string(),
                            4 => "sphere_green".to_string(),
                            _ => format!("object_{}", obj_id),
                        };

                        // Write updated transform to shared state
                        shared.updated_object_id = Some(string_id);
                        shared.updated_transform = Some(crate::subsystems::render::bevy_renderer::gizmos::SharedTransform {
                            position: [transform.translation.x, transform.translation.y, transform.translation.z],
                            rotation: [
                                transform.rotation.x,
                                transform.rotation.y,
                                transform.rotation.z,
                                transform.rotation.w,
                            ],
                            scale: [transform.scale.x, transform.scale.y, transform.scale.z],
                        });
                    }

                    break;
                }
            }
        }
    }

    // End drag
    if !mouse_input.left_down && interaction_state.is_dragging {
        tracing::info!("[BEVY-GIZMO] ✅ Gizmo drag ended - transform changes synced to GPUI");
        interaction_state.is_dragging = false;
        interaction_state.drag_axis = None;
        interaction_state.drag_start_transform = None;
        interaction_state.drag_start_object_id = None;
    }
}

/// Convert screen position to world ray
fn screen_to_world_ray(
    screen_pos: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Ray3d {
    // screen_pos is normalized (0-1), convert to NDC (-1 to 1)
    let ndc = Vec2::new(
        screen_pos.x * 2.0 - 1.0,
        1.0 - screen_pos.y * 2.0, // Flip Y
    );
    
    // Get camera position (ray origin in world space)
    let camera_pos = camera_transform.translation();
    
    // Get projection matrix and inverse
    let projection = camera.clip_from_view();
    let inv_projection = projection.inverse();
    
    // Unproject NDC to view space
    let near_ndc = Vec3::new(ndc.x, ndc.y, -1.0); // Near plane in NDC
    let far_ndc = Vec3::new(ndc.x, ndc.y, 1.0);   // Far plane in NDC
    
    // Convert from NDC to view space
    let near_view = inv_projection.project_point3(near_ndc);
    let far_view = inv_projection.project_point3(far_ndc);
    
    // Convert from view space to world space using camera transform
    let camera_affine = camera_transform.affine();
    let near_world = camera_affine.transform_point3(near_view);
    let far_world = camera_affine.transform_point3(far_view);
    
    // Calculate ray direction
    let direction = (far_world - near_world).normalize();
    
    // Use camera position as ray origin
    Ray3d::new(camera_pos, Dir3::new_unchecked(direction))
}

/// Test if ray hits a gizmo visual
fn test_ray_gizmo_hit(
    ray: &Ray3d,
    gizmo: &GizmoVisual,
    gizmo_transform: &GlobalTransform,
) -> Option<f32> {
    // Simple bounding sphere test
    let gizmo_pos = gizmo_transform.translation();
    let ray_to_gizmo = gizmo_pos - ray.origin;
    let projection = ray_to_gizmo.dot(*ray.direction);
    
    if projection > 0.0 {
        let closest_point = ray.origin + *ray.direction * projection;
        let dist_to_ray = closest_point.distance(gizmo_pos);
        
        // Gizmos are small - 0.3 unit hit radius
        if dist_to_ray < 0.3 {
            return Some(projection);
        }
    }
    
    None
}

/// Apply gizmo transformation to object
fn apply_gizmo_transformation(
    transform: &mut Transform,
    start_transform: &Transform,
    gizmo_type: GizmoType,
    axis: GizmoAxis,
    mouse_delta: Vec2,
    _camera_transform: &GlobalTransform,
) {
    let sensitivity = 0.01; // Adjust for comfortable manipulation
    
    match gizmo_type {
        GizmoType::Translate => {
            // Translate along axis
            let movement = match axis {
                GizmoAxis::X => Vec3::X * mouse_delta.x * sensitivity,
                GizmoAxis::Y => Vec3::Y * -mouse_delta.y * sensitivity, // Invert Y
                GizmoAxis::Z => Vec3::Z * mouse_delta.x * sensitivity,
                GizmoAxis::None => Vec3::ZERO,
            };
            transform.translation = start_transform.translation + movement;
        }
        GizmoType::Rotate => {
            // Rotate around axis
            let angle = mouse_delta.x * sensitivity;
            let rotation = match axis {
                GizmoAxis::X => Quat::from_axis_angle(Vec3::X, angle),
                GizmoAxis::Y => Quat::from_axis_angle(Vec3::Y, angle),
                GizmoAxis::Z => Quat::from_axis_angle(Vec3::Z, angle),
                GizmoAxis::None => Quat::IDENTITY,
            };
            transform.rotation = rotation * start_transform.rotation;
        }
        GizmoType::Scale => {
            // Scale along axis
            let scale_delta = 1.0 + mouse_delta.y * sensitivity;
            let new_scale = match axis {
                GizmoAxis::X => Vec3::new(start_transform.scale.x * scale_delta, start_transform.scale.y, start_transform.scale.z),
                GizmoAxis::Y => Vec3::new(start_transform.scale.x, start_transform.scale.y * scale_delta, start_transform.scale.z),
                GizmoAxis::Z => Vec3::new(start_transform.scale.x, start_transform.scale.y, start_transform.scale.z * scale_delta),
                GizmoAxis::None => start_transform.scale * scale_delta, // Uniform
            };
            transform.scale = new_scale.max(Vec3::splat(0.1)); // Prevent negative/zero scale
        }
        GizmoType::None => {}
    }
}
