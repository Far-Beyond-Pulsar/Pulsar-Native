//! GPUI actions of the shader editor (the Tools menu and its key bindings).

use gpui::actions;

actions!(
    shader_editor,
    [
        /// Compile the shader and refresh the preview (F7).
        CompileShader,
        /// Drop unsaved edits and load the shader from disk again.
        ReloadShaderFromDisk,
        /// Add a comment box at the centre of the graph.
        AddGraphComment,
    ]
);
