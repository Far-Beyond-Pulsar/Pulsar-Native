/// Modular Struct Editor
///
/// A professional struct type editor with multi-panel layout for
/// visually editing Rust struct definitions.
///
/// Features:
/// - Properties Panel: Edit struct metadata (name, visibility, description)
/// - Fields Panel: Add, remove, and edit struct fields
/// - Code Preview: Real-time Rust code generation with syntax highlighting
/// - Workspace Integration: Dock-based panel system
/// - Type Picker: Visual type selection for fields

mod editor;
mod field_editor;
mod workspace_panels;

pub use editor::StructEditor;
pub use field_editor::FieldEditorView;
pub use workspace_panels::*;
