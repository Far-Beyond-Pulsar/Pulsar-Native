// Visual Block-Based Type Alias Editor
// This is a Scratch-like visual editor for composing Rust type aliases

pub mod type_block;
pub mod constructor_palette;
pub mod block_canvas;
pub mod visual_editor;
pub mod type_palette;

// Export the visual editor as the main AliasEditor
pub use visual_editor::{VisualAliasEditor as AliasEditor, ShowTypePickerRequest};
pub use type_block::{TypeBlock, BlockId};
pub use constructor_palette::{ConstructorPalette, TypeSelected};
pub use block_canvas::{BlockCanvas, DragState, DropTarget};
pub use type_palette::{TypeLibraryPalette, TypeItem};
