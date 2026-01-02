mod editor;
mod method_editor;
mod workspace_panels;
pub mod builtin_provider;

pub use editor::TraitEditor;
pub use method_editor::MethodEditorView;
pub use workspace_panels::{PropertiesPanel, MethodsPanel, CodePreviewPanel};
pub use builtin_provider::TraitEditorProvider;
