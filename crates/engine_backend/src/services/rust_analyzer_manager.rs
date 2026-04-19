//! Re-exports the canonical [`RustAnalyzerManager`] from `pulsar_lsp`.
//!
//! The single authoritative implementation lives in
//! `crates/pulsar_lsp/src/rust_analyzer/mod.rs`.
//! This shim exists so that existing import paths
//! (`engine_backend::services::rust_analyzer_manager::*`) continue to compile
//! without touching every call-site.

pub use pulsar_lsp::rust_analyzer::{
    AnalyzerEvent,
    AnalyzerStatus,
    RustAnalyzerManager,
};
