//! Script Editor UI
//!
//! Code and text editing with LSP support

mod script_editor;
pub mod builtin_provider;

// Re-export main types
pub use script_editor::{
    ScriptEditor as ScriptEditorPanel,
    TextEditorEvent,
    FileExplorer,
    TextEditor,
    ScriptEditorMode,
    DiffFileEntry,
};
pub use builtin_provider::ScriptEditorProvider;
