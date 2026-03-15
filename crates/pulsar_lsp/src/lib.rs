//! `pulsar_lsp` — LSP service layer for Pulsar.
//!
//! Provides:
//! - [`RustAnalyzerManager`] — process lifecycle and LSP protocol implementation
//! - [`traits::LspBackend`] — generic backend trait for any LSP server
//! - [`traits::DefinitionProvider`] and [`traits::HoverProvider`] — GPUI-compatible LSP traits
//! - [`rust_analyzer::path_utils`] — path/URI helpers shared by LSP clients

pub mod rust_analyzer;
pub mod traits;
pub mod types;

pub use rust_analyzer::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
pub use traits::{DefinitionProvider, HoverProvider, LspBackend};
