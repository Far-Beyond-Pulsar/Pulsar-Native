mod editor;
mod variant_editor;
mod workspace_panels;

pub use editor::EnumEditor;
pub use variant_editor::{VariantEditorView, VariantEditorEvent};
pub use workspace_panels::{PropertiesPanel, VariantsPanel, CodePreviewPanel};
