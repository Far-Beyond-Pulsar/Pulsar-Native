//! User Interaction Systems
//!
//! This module handles all user interaction with the 3D viewport, including:
//! - Object selection via mouse raycasting
//! - Gizmo manipulation (translate, rotate, scale)
//! - Camera controls (handled in systems/camera.rs)
//!
//! ## Organization
//!
//! - `viewport` - Complete viewport interaction system (object selection, gizmo manipulation)
//!   - Async raycasting using Bevy's task pool
//!   - Object picking with bounding box tests
//!   - Gizmo axis highlighting and drag operations
//!
//! - `gizmo_interaction` - **DEPRECATED** - Legacy gizmo interaction code
//!   - Kept for reference during migration
//!   - All functionality moved to viewport module
//!   - Will be removed in future cleanup
//!
//! ## Usage
//!
//! The viewport module provides a complete interaction system:
//!
//! ```rust
//! use interaction::{ViewportMouseInput, GizmoInteractionState, RaycastResult};
//!
//! // GPUI sets mouse input every frame
//! viewport_mouse_input.mouse_pos = Vec2::new(x, y);
//! viewport_mouse_input.left_clicked = true;
//!
//! // Bevy systems process the input asynchronously
//! // - viewport_selection_system: Handles object picking
//! // - gizmo_manipulation_system: Handles gizmo dragging
//! ```

pub mod viewport;

// DEPRECATED: Legacy module kept for reference
#[deprecated(
    since = "0.2.0",
    note = "Use viewport module instead. This will be removed in a future version."
)]
pub mod gizmo_interaction;

// Re-export active interaction types
pub use viewport::{
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
    viewport_click_initiate_raycast_system, viewport_poll_raycast_system, gizmo_drag_system,
    clear_viewport_input_system,
};
