//! Gizmo System
//!
//! This module provides 3D manipulation gizmos for the level editor, similar to
//! Unity/Unreal/Blender gizmos. Gizmos are interactive 3D widgets that allow
//! users to visually manipulate object transforms.
//!
//! ## Features
//!
//! - **Translate Gizmo** - XYZ arrows for moving objects
//! - **Rotate Gizmo** - XYZ circles for rotating objects  
//! - **Scale Gizmo** - XYZ handles for scaling objects
//! - **Axis Highlighting** - Hovered/selected axis glows
//! - **Mode Awareness** - Automatically hidden during Play mode
//!
//! ## Organization
//!
//! - `rendering` - Gizmo visual rendering and spawning
//!   - GizmoType, GizmoAxis enums
//!   - GizmoStateResource (shared with GPUI)
//!   - Gizmo mesh generation and rendering systems
//!
//! ## Integration
//!
//! Gizmos are controlled by the GPUI editor through shared state:
//!
//! ```rust
//! // GPUI updates gizmo state
//! gizmo_state.gizmo_type = GizmoType::Translate;
//! gizmo_state.selected_object_id = Some("cube_001".to_string());
//! gizmo_state.enabled = true; // Hide in Play mode
//!
//! // Bevy renders the gizmo at the selected object's position
//! ```
//!
//! ## Rendering Pipeline
//!
//! 1. **Sync** - `sync_gizmo_state_system` copies state from GPUI to Bevy
//! 2. **Update** - `update_gizmo_visuals_system` spawns/updates gizmo meshes
//! 3. **Render** - Standard Bevy rendering renders the gizmo geometry
//! 4. **Interact** - `interaction::viewport` handles mouse picking and dragging

pub mod rendering;

// Re-export gizmo types
pub use rendering::{
    GizmoType, GizmoAxis, GizmoVisual, GizmoStateResource, SharedTransform,
    update_gizmo_visuals, update_selection_highlighting,
};
