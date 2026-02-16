//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports
//! - Rust Analyzer integration for code intelligence
//! - LSP completion provider for code suggestions

pub mod gpu_renderer;
pub mod lsp_completion_provider;
pub mod path_utils;
pub mod rust_analyzer_manager;
pub mod physics_query_service;
pub mod gizmo_interaction_controller;

pub use gpu_renderer::GpuRenderer;
pub use lsp_completion_provider::GlobalRustAnalyzerCompletionProvider;
pub use path_utils::path_to_uri;
pub use rust_analyzer_manager::RustAnalyzerManager;
pub use physics_query_service::{PhysicsQueryService, RaycastHit, ColliderTag, GizmoType};
pub use gizmo_interaction_controller::{GizmoInteractionController, InteractionState, DragState};
