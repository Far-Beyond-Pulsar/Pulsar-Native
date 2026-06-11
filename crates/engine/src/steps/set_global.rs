//! Set global step: register the engine context globally.

use crate::init::{InitContext, InitError};

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let engine_context = ctx
        .engine_context
        .as_ref()
        .ok_or(InitError::MissingContext("Engine context not initialized"))?;

    engine_context.clone().set_global();
    Ok(())
}
