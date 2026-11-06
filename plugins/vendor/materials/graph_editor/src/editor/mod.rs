//! Main editor state container and lifecycle management.
//!
//! [`ShaderEditorPanel`] holds the canvas that owns the document, the history,
//! and the chrome around them (toolbar, dock workspace, compiler output).

pub mod canvas_view;
pub mod panel;
pub mod panel_render;
pub mod toolbar;
pub mod workspace;
pub mod workspace_panels;

