//! Camera input synchronization and movement systems

use bevy::prelude::*;
use crate::subsystems::render::bevy_renderer::core::{CameraInput, CameraInputResource, MainCamera};

/// Sync camera input from the input thread to the Bevy ECS resource
/// This system reads from the shared Arc<Mutex<CameraInput>> that the input thread updates
/// and copies it to the Bevy ECS CameraInput resource that camera_movement_system uses
pub fn sync_camera_input_system(
    camera_input_resource: Res<CameraInputResource>,
    mut camera_input: ResMut<CameraInput>,
) {
    profiling::profile_scope!("Bevy::SyncCameraInput");
    // Try to lock the shared camera input without blocking
    if let Ok(mut shared_input) = camera_input_resource.0.try_lock() {
        // Copy the input from the input thread to the Bevy ECS resource
        *camera_input = shared_input.clone();
        
        // IMPORTANT: Clear the delta values in the shared input after copying
        // so they don't get re-applied on the next frame
        // The input thread will set new deltas if there's actual mouse movement
        shared_input.mouse_delta_x = 0.0;
        shared_input.mouse_delta_y = 0.0;
        shared_input.pan_delta_x = 0.0;
        shared_input.pan_delta_y = 0.0;
        shared_input.zoom_delta = 0.0;
    }
    // If lock fails, skip this frame - no blocking!
}

/// Unreal Engine-style camera movement system
/// Supports:
/// - WASD + QE for movement (with Shift for boost)
/// - Right mouse + drag for FPS rotation
/// - Middle mouse + drag for panning
/// - Mouse wheel for zoom (or move speed adjustment with right mouse held)
pub fn camera_movement_system(
    time: Res<Time>,
    mut camera_input: ResMut<CameraInput>,
    mut query: Query<&mut Transform, With<MainCamera>>,
) {
    profiling::profile_scope!("Bevy::CameraMovement");
    let Ok(mut transform) = query.single_mut() else {
        return;
    };

    let delta_time = time.delta_secs();
    
    // --- Unreal-style movement ramp-up (acceleration) with doubled ramp and mouse smoothing ---
    static mut VELOCITY: Vec3 = Vec3::ZERO;
    static mut SMOOTHED_MOUSE_DELTA: (f32, f32) = (0.0, 0.0);
    let mut input_dir = Vec3::ZERO;
    if camera_input.forward.abs() > 0.001 {
        input_dir += transform.forward().as_vec3() * camera_input.forward;
    }
    if camera_input.right.abs() > 0.001 {
        input_dir += transform.right().as_vec3() * camera_input.right;
    }
    if camera_input.up.abs() > 0.001 {
        input_dir += Vec3::Y * camera_input.up;
    }
    if input_dir.length_squared() > 0.0 {
        input_dir = input_dir.normalize();
    }
    let base_speed = if camera_input.boost {
        camera_input.move_speed * 3.0
    } else {
        camera_input.move_speed
    };
    let accel = base_speed * 12.0; // doubled acceleration rate
    let friction = 4.0;
    unsafe {
        // Accelerate towards input direction
        let desired_velocity = input_dir * base_speed;
        let delta_v = desired_velocity - VELOCITY;
        let accel_step = accel * delta_time;
        let friction_step = friction * delta_time;
        if delta_v.length() > accel_step {
            VELOCITY += delta_v.normalize() * accel_step;
        } else {
            VELOCITY = desired_velocity;
        }
        if input_dir == Vec3::ZERO {
            let speed = VELOCITY.length();
            if speed > friction_step {
                VELOCITY -= VELOCITY.normalize() * friction_step;
            } else {
                VELOCITY = Vec3::ZERO;
            }
        }
        transform.translation += VELOCITY * delta_time;
    }

    // --- Mouse delta smoothing for rotation ---
    let smoothing = 0.25; // 0 = no smoothing, 1 = infinite smoothing
    let (smooth_x, smooth_y) = unsafe {
        let (prev_x, prev_y) = SMOOTHED_MOUSE_DELTA;
        let target_x = camera_input.mouse_delta_x;
        let target_y = camera_input.mouse_delta_y;
        let smooth_x = prev_x + (target_x - prev_x) * smoothing;
        let smooth_y = prev_y + (target_y - prev_y) * smoothing;
        SMOOTHED_MOUSE_DELTA = (smooth_x, smooth_y);
        (smooth_x, smooth_y)
    };
    // Use smoothed deltas for rotation
    if smooth_x.abs() > 0.001 || smooth_y.abs() > 0.001 {
        let yaw_delta = -smooth_x * camera_input.look_sensitivity * delta_time;
        let (mut yaw, mut pitch, mut roll) = transform.rotation.to_euler(EulerRot::YXZ);
        yaw += yaw_delta;
        let pitch_delta = -smooth_y * camera_input.look_sensitivity * delta_time;
        pitch += pitch_delta;
        // Clamp pitch between -89 and 89 degrees (in radians)
        let min_pitch = -std::f32::consts::FRAC_PI_2 + 0.01745; // ~-89 deg
        let max_pitch = std::f32::consts::FRAC_PI_2 - 0.01745;  // ~89 deg
        if pitch < min_pitch {
            pitch = min_pitch;
        } else if pitch > max_pitch {
            pitch = max_pitch;
        }
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
    }
    // Clear mouse deltas after use
    camera_input.mouse_delta_x = 0.0;
    camera_input.mouse_delta_y = 0.0;
    
    // === ROTATION (Right mouse + drag) ===
    if camera_input.mouse_delta_x.abs() > 0.001 || camera_input.mouse_delta_y.abs() > 0.001 {
        // Yaw (rotate around world Y axis) and clamp pitch
        let yaw_delta = -camera_input.mouse_delta_x * camera_input.look_sensitivity * delta_time;
        let (mut yaw, mut pitch, mut roll) = transform.rotation.to_euler(EulerRot::YXZ);
        yaw += yaw_delta;
        let pitch_delta = -camera_input.mouse_delta_y * camera_input.look_sensitivity * delta_time;
        pitch += pitch_delta;
        // Clamp pitch between -89 and 89 degrees (in radians)
        let min_pitch = -std::f32::consts::FRAC_PI_2 + 0.01745; // ~-89 deg
        let max_pitch = std::f32::consts::FRAC_PI_2 - 0.01745;  // ~89 deg
        if pitch < min_pitch {
            pitch = min_pitch;
        } else if pitch > max_pitch {
            pitch = max_pitch;
        }
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
        // Clear mouse deltas after use
        camera_input.mouse_delta_x = 0.0;
        camera_input.mouse_delta_y = 0.0;
    }
    
    // === PANNING (Middle mouse + drag) ===
    if camera_input.pan_delta_x.abs() > 0.001 || camera_input.pan_delta_y.abs() > 0.001 {
        // Pan along camera's local axes
        let right = transform.right();
        let up = transform.up();
        
        transform.translation -= right.as_vec3() * camera_input.pan_delta_x * camera_input.pan_speed;
        transform.translation += up.as_vec3() * camera_input.pan_delta_y * camera_input.pan_speed;
        
        // Clear pan deltas after use
        camera_input.pan_delta_x = 0.0;
        camera_input.pan_delta_y = 0.0;
    }
    
    // === ZOOM (Mouse wheel) ===
    if camera_input.zoom_delta.abs() > 0.001 {
        let forward = transform.forward();
        transform.translation += forward.as_vec3() * camera_input.zoom_delta * camera_input.zoom_speed * delta_time;
        
        // Clear zoom delta after use
        camera_input.zoom_delta = 0.0;
    }
    
    // === ORBIT MODE (Alt + Left mouse - future enhancement) ===
    if camera_input.orbit_mode {
        // Calculate camera position relative to focus point
        let offset = transform.translation - camera_input.focus_point;
        let _distance = offset.length();
        
        // Rotate offset around focus point
        if camera_input.mouse_delta_x.abs() > 0.001 || camera_input.mouse_delta_y.abs() > 0.001 {
            // This would require converting to spherical coordinates and back
            // For now, keeping it simple with FPS rotation
        }
    }
}
