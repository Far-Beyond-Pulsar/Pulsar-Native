//! Host function implementations for WASM plugins
//!
//! These functions are provided by the host and callable from WASM plugins

use anyhow::Result;
use async_trait::async_trait;
use wasmtime::component::Linker;

use super::WasmState;
use super::pulsar::editor::host::Host;

// Implement the host interface
#[async_trait]
impl Host for WasmState {
    async fn log_debug(&mut self, message: String) -> () {
        tracing::debug!("[WASM Plugin] {}", message);
    }

    async fn log_info(&mut self, message: String) -> () {
        tracing::info!("[WASM Plugin] {}", message);
    }

    async fn log_warn(&mut self, message: String) -> () {
        tracing::warn!("[WASM Plugin] {}", message);
    }

    async fn log_error(&mut self, message: String) -> () {
        tracing::error!("[WASM Plugin] {}", message);
    }

    async fn read_file(&mut self, path: String) -> Result<String, String> {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    async fn write_file(&mut self, path: String, content: String) -> Result<(), String> {
        std::fs::write(&path, content).map_err(|e| e.to_string())
    }

    async fn file_exists(&mut self, path: String) -> bool {
        std::path::Path::new(&path).exists()
    }
}

/// Add host functions to the linker
pub fn add_to_linker(linker: &mut Linker<WasmState>) -> Result<()> {
    // Add the host interface to the linker
    super::pulsar::editor::host::add_to_linker(linker, |state| state)?;

    Ok(())
}
