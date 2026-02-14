//! WASM-based plugin host - Production Implementation
//!
//! This provides a complete WebAssembly runtime for plugins using Wasmtime's
//! component model, similar to Zed's extension system.

mod host_impl;

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use plugin_editor_api::*;

// Generate bindings from WIT
wasmtime::component::bindgen!({
    world: "pulsar-plugin",
    path: "wit/editor.wit",
    async: true,
});

use pulsar::editor::types as wit_types;

/// WASM plugin state that implements WasiView
pub struct WasmState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for WasmState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

/// WASM plugin host - manages the runtime
pub struct WasmPluginHost {
    engine: Engine,
    linker: Linker<WasmState>,
}

impl WasmPluginHost {
    /// Create a new WASM plugin host
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);

        // Add WASI support
        wasmtime_wasi::add_to_linker_async(&mut linker)?;

        // Add host functions
        host_impl::add_to_linker(&mut linker)?;

        Ok(Self { engine, linker })
    }

    /// Load a plugin from WASM file
    pub async fn load_plugin(&self, wasm_path: &Path) -> Result<WasmPlugin> {
        tracing::info!("Loading WASM plugin from: {:?}", wasm_path);

        let wasm_bytes = std::fs::read(wasm_path)
            .with_context(|| format!("Failed to read WASM file: {:?}", wasm_path))?;

        let component = Component::from_binary(&self.engine, &wasm_bytes)
            .context("Failed to compile WASM component")?;

        let wasi = WasiCtxBuilder::new()
            .inherit_stdio()
            .inherit_env()
            .build();

        let state = WasmState {
            wasi,
            table: ResourceTable::new(),
        };

        let mut store = Store::new(&self.engine, state);
        
        let bindings =
            PulsarPlugin::instantiate_async(&mut store, &component, &self.linker)
                .await
                .context("Failed to instantiate WASM plugin")?;

        // Call on_load
        bindings.pulsar_editor_plugin()
            .call_on_load(&mut store)
            .await?;

        // Get metadata
        let meta_wit = bindings.pulsar_editor_plugin()
            .call_metadata(&mut store)
            .await?;

        let metadata = PluginMetadata {
            id: PluginId::new(&meta_wit.id.value),
            name: meta_wit.name,
            version: meta_wit.version,
            author: meta_wit.author,
            description: meta_wit.description,
        };

        tracing::info!("Loaded WASM plugin: {} v{}", metadata.name, metadata.version);

        Ok(WasmPlugin {
            store: Arc::new(Mutex::new(store)),
            bindings,
            metadata,
            path: wasm_path.to_path_buf(),
            editors: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

/// A loaded WASM plugin instance
pub struct WasmPlugin {
    store: Arc<Mutex<Store<WasmState>>>,
    bindings: PulsarPlugin,
    metadata: PluginMetadata,
    path: PathBuf,
    editors: Arc<Mutex<HashMap<String, WasmEditorInstance>>>,
}

impl WasmPlugin {
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn file_types(&self) -> Result<Vec<FileTypeDefinition>> {
        let mut store = self.store.lock().unwrap();
        let file_types_wit = self.bindings.pulsar_editor_plugin()
            .call_file_types(&mut *store)
            .await?;

        Ok(file_types_wit.into_iter().map(|ft| {
            FileTypeDefinition {
                id: FileTypeId::new(&ft.id.value),
                extension: ft.extension,
                display_name: ft.display_name,
                icon: ui::IconName::Code, // Map from string
                color: gpui::Hsla::from(gpui::rgb(ft.color)),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!({}),
                categories: ft.categories,
            }
        }).collect())
    }

    pub async fn editors(&self) -> Result<Vec<EditorMetadata>> {
        let mut store = self.store.lock().unwrap();
        let editors_wit = self.bindings.pulsar_editor_plugin()
            .call_editors(&mut *store)
            .await?;

        Ok(editors_wit.into_iter().map(|ed| {
            EditorMetadata {
                id: EditorId::new(&ed.id.value),
                display_name: ed.display_name,
                supported_file_types: ed.supported_file_types
                    .into_iter()
                    .map(|ft| FileTypeId::new(&ft.value))
                    .collect(),
            }
        }).collect())
    }

    pub async fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
    ) -> Result<String> {
        let mut store = self.store.lock().unwrap();
        
        let editor_id_wit = wit_types::EditorId {
            value: editor_id.as_str().to_string(),
        };

        let instance_id = self.bindings.pulsar_editor_plugin()
            .call_create_editor(
                &mut *store,
                &editor_id_wit,
                &file_path.to_string_lossy(),
            )
            .await?
            .map_err(|e| anyhow!("Failed to create editor: {}", e))?;

        // Store editor instance
        self.editors.lock().unwrap().insert(
            instance_id.clone(),
            WasmEditorInstance {
                instance_id: instance_id.clone(),
                file_path,
            },
        );

        Ok(instance_id)
    }

    pub async fn save_editor(&self, instance_id: &str) -> Result<()> {
        let mut store = self.store.lock().unwrap();
        self.bindings.pulsar_editor_plugin()
            .call_save_editor(&mut *store, instance_id)
            .await?
            .map_err(|e| anyhow!("Save failed: {}", e))
    }

    pub async fn is_dirty(&self, instance_id: &str) -> Result<bool> {
        let mut store = self.store.lock().unwrap();
        Ok(self.bindings.pulsar_editor_plugin()
            .call_is_dirty(&mut *store, instance_id)
            .await?)
    }

    pub async fn on_unload(&self) -> Result<()> {
        let mut store = self.store.lock().unwrap();
        self.bindings.pulsar_editor_plugin()
            .call_on_unload(&mut *store)
            .await
    }
}

struct WasmEditorInstance {
    instance_id: String,
    file_path: PathBuf,
}

impl Drop for WasmPlugin {
    fn drop(&mut self) {
        tracing::debug!("Dropping WASM plugin: {:?}", self.path);
    }
}
