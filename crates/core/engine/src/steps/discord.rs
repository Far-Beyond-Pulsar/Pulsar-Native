//! Discord step: Rich Presence initialization.

use crate::consts;
use crate::init::{InitContext, InitError};

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let engine_context = ctx
        .engine_context
        .as_ref()
        .ok_or(InitError::MissingContext("Engine context not initialized"))?;

    if let Err(e) = crate::discord::init_discord(engine_context, consts::DISCORD_APP_ID) {
        tracing::warn!("Failed to initialize Discord Rich Presence: {}", e);
    }
    Ok(())
}
