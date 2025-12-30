//! Script Editor UI
//!
//! Code and text editing with LSP support

mod script_editor;

// Re-export main types
pub use script_editor::{
    ScriptEditor as ScriptEditorPanel,
    TextEditorEvent,
    FileExplorer,
    TextEditor,
    ScriptEditorMode,
    DiffFileEntry,
};
