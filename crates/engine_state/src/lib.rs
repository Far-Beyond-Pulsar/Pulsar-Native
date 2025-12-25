//! Engine State Management
//!
//! Centralized state management for the Pulsar Engine.
//! Provides thread-safe storage for:
//! - Metadata (key-value pairs)
//! - GPU renderers (per-window)
//! - Communication channels
//! - Global registries

mod metadata;
mod renderers;
mod channels;
mod discord;

pub use metadata::Metadata;
pub use renderers::{RendererRegistry, RendererHandle};
pub use channels::{WindowRequest, WindowRequestSender, WindowRequestReceiver, window_request_channel};
pub use discord::DiscordPresence;

use std::sync::Arc;
use parking_lot::RwLock;
use type_db::TypeDatabase;

/// Global engine state
#[derive(Clone)]
pub struct EngineState {
    inner: Arc<RwLock<EngineStateInner>>,
}

struct EngineStateInner {
    metadata: Metadata,
    renderers: RendererRegistry,
    window_count: usize,
    window_sender: Option<WindowRequestSender>,
    discord_presence: Option<DiscordPresence>,
    type_database: Option<Arc<TypeDatabase>>,
}

impl EngineState {
    /// Create new engine state
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(EngineStateInner {
                metadata: Metadata::new(),
                renderers: RendererRegistry::new(),
                window_count: 0,
                window_sender: None,
                discord_presence: None,
                type_database: None,
            })),
        }
    }

    /// Add window sender (builder pattern)
    pub fn with_window_sender(self, sender: WindowRequestSender) -> Self {
        self.inner.write().window_sender = Some(sender);
        self
    }

    /// Get metadata
    pub fn metadata(&self) -> Metadata {
        self.inner.read().metadata.clone()
    }

    /// Set metadata key-value pair (convenience method)
    pub fn set_metadata(&self, key: String, value: String) {
        self.metadata().set(key, value);
    }

    /// Get metadata value (convenience method)
    pub fn get_metadata(&self, key: &str) -> Option<String> {
        self.metadata().get(key)
    }

    /// Get renderer registry
    pub fn renderers(&self) -> RendererRegistry {
        self.inner.read().renderers.clone()
    }

    /// Request a window
    pub fn request_window(&self, request: WindowRequest) {
        let inner = self.inner.read();
        if let Some(sender) = &inner.window_sender {
            if let Err(e) = sender.send(request) {
                eprintln!("❌ Failed to send window request: {}", e);
            }
        } else {
            eprintln!("❌ Window sender not initialized!");
        }
    }

    /// Set window GPU renderer
    pub fn set_window_gpu_renderer(&self, window_id: u64, renderer: RendererHandle) {
        self.renderers().register(window_id, renderer);
    }

    /// Get window GPU renderer
    pub fn get_window_gpu_renderer(&self, window_id: u64) -> Option<RendererHandle> {
        self.renderers().get(window_id)
    }

    /// Remove window GPU renderer
    pub fn remove_window_gpu_renderer(&self, window_id: u64) -> Option<RendererHandle> {
        self.renderers().unregister(window_id)
    }

    /// Increment window count
    pub fn increment_window_count(&self) -> usize {
        let mut inner = self.inner.write();
        inner.window_count += 1;
        inner.window_count
    }

    /// Decrement window count
    pub fn decrement_window_count(&self) -> usize {
        let mut inner = self.inner.write();
        inner.window_count = inner.window_count.saturating_sub(1);
        inner.window_count
    }

    /// Get window count
    pub fn window_count(&self) -> usize {
        self.inner.read().window_count
    }

    /// Set global instance
    pub fn set_global(self) {
        GLOBAL_STATE.set(self).ok();
    }

    /// Get global instance (returns Option for compatibility)
    pub fn global() -> Option<&'static Self> {
        GLOBAL_STATE.get()
    }

    /// Initialize Discord Rich Presence
    /// 
    /// # Arguments
    /// * `application_id` - Your Discord application ID (get from https://discord.com/developers/applications)
    pub fn init_discord(&self, application_id: impl Into<String>) -> anyhow::Result<()> {
        let presence = DiscordPresence::new(application_id);
        presence.connect()?;
        self.inner.write().discord_presence = Some(presence);
        Ok(())
    }

    /// Get Discord presence instance
    pub fn discord(&self) -> Option<DiscordPresence> {
        self.inner.read().discord_presence.clone()
    }

    /// Update Discord presence with current project and editor state
    pub fn update_discord_presence(&self, project_name: Option<String>, tab_name: Option<String>, file_path: Option<String>) {
        if let Some(discord) = self.discord() {
            discord.update_all(project_name, tab_name, file_path);
        }
    }

    /// Set the global TypeDatabase instance
    pub fn set_type_database(&self, type_database: Arc<TypeDatabase>) {
        self.inner.write().type_database = Some(type_database);
    }

    /// Get the global TypeDatabase instance
    pub fn type_database(&self) -> Option<Arc<TypeDatabase>> {
        self.inner.read().type_database.clone()
    }
}

impl Default for EngineState {
    fn default() -> Self {
        Self::new()
    }
}

use std::sync::OnceLock;
static GLOBAL_STATE: OnceLock<EngineState> = OnceLock::new();
