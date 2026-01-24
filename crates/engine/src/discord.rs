//! Discord Rich Presence integration for Pulsar Engine
//
// This module handles initialization and error handling for Discord Rich Presence.

use crate::engine_state::{EngineState, EngineContext};

/// Initialize Discord Rich Presence if configured (EngineContext version).
pub fn init_discord_ctx(engine_context: &EngineContext, discord_app_id: &str) -> anyhow::Result<()> {
    if discord_app_id != "YOUR_DISCORD_APPLICATION_ID_HERE" {
        engine_context.init_discord(discord_app_id)?;
        tracing::debug!("✅ Discord Rich Presence initialized");
        Ok(())
    } else {
        tracing::debug!("ℹ️  Discord Rich Presence not configured (set discord_app_id in main.rs)");
        Ok(())
    }
}

/// Initialize Discord Rich Presence if configured (legacy EngineState version).
#[deprecated(note = "Use init_discord_ctx with EngineContext instead")]
pub fn init_discord(engine_state: &EngineState, discord_app_id: &str) {
    if discord_app_id != "YOUR_DISCORD_APPLICATION_ID_HERE" {
        match engine_state.init_discord(discord_app_id) {
            Ok(_) => tracing::debug!("✅ Discord Rich Presence initialized"),
            Err(e) => tracing::warn!("⚠️  Discord Rich Presence failed to initialize: {}", e),
        }
    } else {
        tracing::debug!("ℹ️  Discord Rich Presence not configured (set discord_app_id in main.rs)");
    }
}
