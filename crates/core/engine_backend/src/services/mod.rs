//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports
//! - Rust Analyzer integration for code intelligence
//! - LSP completion provider for code suggestions

pub mod core_project_builder;
#[cfg(feature = "render")]
pub mod gpu_renderer;
#[cfg(feature = "editor-ui")]
pub mod lsp_completion_provider;
#[cfg(feature = "physics")]
pub mod physics_query_service;
#[cfg(feature = "pie")]
pub mod pie_blit;
#[cfg(feature = "pie")]
pub mod pie_host;
#[cfg(feature = "editor-ui")]
pub mod rust_analyzer_manager {
    pub use pulsar_lsp::rust_analyzer::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
}

pub use core_project_builder::ensure_core_bootstrap;
#[cfg(feature = "render")]
pub use gpu_renderer::GpuRenderer;
#[cfg(feature = "editor-ui")]
pub use lsp_completion_provider::GlobalRustAnalyzerCompletionProvider;
#[cfg(feature = "physics")]
pub use physics_query_service::{ColliderTag, GizmoType, PhysicsQueryService, RaycastHit};
#[cfg(feature = "pie")]
pub use pie_blit::PieBlit;
#[cfg(feature = "pie")]
pub use pie_host::PieHost;
#[cfg(feature = "editor-ui")]
pub use pulsar_lsp::rust_analyzer::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
