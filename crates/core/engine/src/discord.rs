//! Discord Rich Presence integration for Pulsar Engine
//
// This module handles initialization and error handling for Discord Rich Presence.

use crate::engine_state::EngineContext;

/// Initialize Discord Rich Presence if configured.
pub fn init_discord(engine_context: &EngineContext, discord_app_id: &str) -> anyhow::Result<()> {
    if discord_app_id != "YOUR_DISCORD_APPLICATION_ID_HERE" {
        engine_context.init_discord(discord_app_id)?;
        tracing::debug!("✅ Discord Rich Presence initialized");
        Ok(())
    } else {
        tracing::debug!("ℹ️  Discord Rich Presence not configured (set discord_app_id in main.rs)");
        Ok(())
    }
}
