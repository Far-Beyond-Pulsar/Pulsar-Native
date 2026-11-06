//! Feature modules: the shader-specific parts of the editor.
//!
//! Graph editing itself (nodes, connections, comments, selection, clipboard,
//! viewport, undo) belongs to the shared Graphy canvas; what is left here is what
//! only shaders have.

pub mod compilation;
pub mod import_companion;
pub mod preview;
pub mod shader_model;

/// Initialize all feature modules that need startup registration
pub fn initialize_features() {
    shader_model::standard::register_standard_models();
}
