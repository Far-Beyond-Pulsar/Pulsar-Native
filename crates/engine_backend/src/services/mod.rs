//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports
//! - Rust Analyzer integration for code intelligence
//! - LSP completion provider for code suggestions

pub mod gpu_renderer;
pub mod lsp_completion_provider;
pub mod rust_analyzer_manager;

pub use gpu_renderer::GpuRenderer;
pub use lsp_completion_provider::GlobalRustAnalyzerCompletionProvider;
pub use rust_analyzer_manager::RustAnalyzerManager;
