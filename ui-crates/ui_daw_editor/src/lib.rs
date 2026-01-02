//! DAW Editor UI
//!
//! Digital Audio Workstation for sound design

mod daw_editor;
pub mod builtin_provider;

// Re-export main types
pub use daw_editor::{
    DawEditorPanel,
    AudioService,
};
pub use builtin_provider::DawEditorProvider;
