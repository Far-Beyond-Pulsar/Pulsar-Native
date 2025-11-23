// Blueprint Editor Panel Module
//
// This module contains the main BlueprintEditorPanel implementation
// and all its sub-modules.

// Constants
pub const NODE_MENU_WIDTH: f32 = 320.0;
pub const NODE_MENU_MAX_HEIGHT: f32 = 600.0;

// Core panel implementation
mod core;
pub use core::{BlueprintEditorPanel, ResizeHandle, ConnectionDrag, TabDragInfo, CompilationHistoryEntry};

// Rendering
mod render;
mod render_workspace;

// Operations
mod node_ops;
mod connection_ops;
mod comment_ops;
mod selection;
mod viewport;
mod menu;

// State management
pub mod variables;
pub mod tabs;

// File operations
mod file_io;
mod compilation;
mod graph_conversion;

// Workspace panels
mod workspace_panels;
pub use workspace_panels::*;
