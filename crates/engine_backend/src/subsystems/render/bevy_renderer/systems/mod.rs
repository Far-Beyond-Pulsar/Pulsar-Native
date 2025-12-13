//! Bevy Systems
//!
//! This module contains all Bevy ECS systems that run every frame to update
//! the renderer state, synchronize with other threads, and manage the scene.
//!
//! ## Organization
//!
//! - `camera` - Camera input synchronization and movement (Unreal-style controls)
//! - `sync` - Game thread synchronization (object positions, gizmo state, input)
//! - `metrics` - Performance metrics tracking and GPU profiling
//! - `scene` - Scene setup and management (spawning objects, lights, camera)
//!
//! ## System Execution Order
//!
//! 1. **Sync Phase** - Copy data from external threads to Bevy resources
//!    - `sync_camera_input_system` - Camera input from input thread
//!    - `sync_gizmo_state_system` - Gizmo state from GPUI editor
//!    - `sync_viewport_mouse_input_system` - Mouse input from GPUI viewport
//!    - `sync_game_objects_system` - Object positions from game thread
//!
//! 2. **Update Phase** - Process game logic and user input
//!    - `camera_movement_system` - Apply camera controls
//!    - `viewport_selection_system` - Handle object selection raycasts
//!    - `gizmo_manipulation_system` - Handle gizmo drag operations
//!
//! 3. **Metrics Phase** - Track performance data
//!    - `update_metrics_system` - Update FPS, frame time, etc.
//!    - `update_gpu_profiler_system` - Collect GPU timing data

pub mod camera;
pub mod sync;
pub mod metrics;
pub mod scene;

// Re-export system functions for easy registration
pub use camera::{sync_camera_input_system, camera_movement_system};
pub use sync::{
    sync_gizmo_state_system, sync_viewport_mouse_input_system,
    sync_game_objects_system, update_gizmo_target_system,
    update_camera_viewport_system,
};
pub use metrics::{update_metrics_system, update_gpu_profiler_system};
pub use scene::{
    setup_scene, animate_objects_system, swap_render_buffers_system, debug_rendering_system,
};
