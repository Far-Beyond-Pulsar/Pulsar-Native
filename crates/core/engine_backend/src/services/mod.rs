//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports
//! - Rust Analyzer integration for code intelligence
//! - LSP completion provider for code suggestions

pub mod core_project_builder;
pub mod gizmo_interaction_controller;
pub mod gpu_renderer;
pub mod lsp_completion_provider;
pub mod physics_query_service;
pub mod pie_blit;
pub mod pie_host;
pub mod rust_analyzer_manager {
    pub use pulsar_lsp::rust_analyzer::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
}

pub use core_project_builder::ensure_core_bootstrap;
pub use gizmo_interaction_controller::{DragState, GizmoInteractionController, InteractionState};
pub use gpu_renderer::GpuRenderer;
pub use lsp_completion_provider::GlobalRustAnalyzerCompletionProvider;
pub use physics_query_service::{ColliderTag, GizmoType, PhysicsQueryService, RaycastHit};
pub use pie_blit::PieBlit;
pub use pie_host::PieHost;
pub use pulsar_lsp::rust_analyzer::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
