//! Host function implementations for WASM plugins
//!
//! These functions are provided by the host and callable from WASM plugins

use anyhow::Result;
use wasmtime::component::Linker;

use super::WasmState;

/// Add host functions to the linker
pub fn add_to_linker(linker: &mut Linker<WasmState>) -> Result<()> {
    // Import the generated host trait
    use super::pulsar::editor::host::Host;

    // Implement the host interface
    impl Host for WasmState {
        fn log_debug<'life0, 'async_trait>(
            &'life0 mut self,
            message: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                tracing::debug!("[WASM Plugin] {}", message);
                Ok(())
            })
        }

        fn log_info<'life0, 'async_trait>(
            &'life0 mut self,
            message: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                tracing::info!("[WASM Plugin] {}", message);
                Ok(())
            })
        }

        fn log_warn<'life0, 'async_trait>(
            &'life0 mut self,
            message: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                tracing::warn!("[WASM Plugin] {}", message);
                Ok(())
            })
        }

        fn log_error<'life0, 'async_trait>(
            &'life0 mut self,
            message: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                tracing::error!("[WASM Plugin] {}", message);
                Ok(())
            })
        }

        fn read_file<'life0, 'async_trait>(
            &'life0 mut self,
            path: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<Result<String, String>>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Ok(content)),
                    Err(e) => Ok(Err(e.to_string())),
                }
            })
        }

        fn write_file<'life0, 'async_trait>(
            &'life0 mut self,
            path: String,
            content: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<Result<(), String>>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                match std::fs::write(&path, content) {
                    Ok(_) => Ok(Ok(())),
                    Err(e) => Ok(Err(e.to_string())),
                }
            })
        }

        fn file_exists<'life0, 'async_trait>(
            &'life0 mut self,
            path: String,
        ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = wasmtime::Result<bool>> + ::core::marker::Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                Ok(std::path::Path::new(&path).exists())
            })
        }
    }

    // Add the host interface to the linker
    super::pulsar::editor::host::add_to_linker(linker, |state| state)?;

    Ok(())
}
