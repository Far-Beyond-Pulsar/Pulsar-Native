//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports
//! - Rust Analyzer integration for code intelligence
//! - LSP completion provider for code suggestions

pub mod gizmo_interaction_controller;
pub mod gpu_renderer;
pub mod lsp_completion_provider;
pub mod path_utils;
pub mod physics_query_service;
pub mod rust_analyzer_manager;

pub use gizmo_interaction_controller::{DragState, GizmoInteractionController, InteractionState};
pub use gpu_renderer::GpuRenderer;
pub use lsp_completion_provider::GlobalRustAnalyzerCompletionProvider;
pub use path_utils::path_to_uri;
pub use physics_query_service::{ColliderTag, GizmoType, PhysicsQueryService, RaycastHit};
pub use rust_analyzer_manager::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
