//! Backend step: engine backend subsystems (physics, etc.).

use crate::init::{InitContext, InitError};

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let rt = ctx
        .runtime
        .as_ref()
        .ok_or(InitError::MissingContext("Runtime not initialized"))?;

    let backend = rt.block_on(async { engine_backend::EngineBackend::init().await });

    // Set backend as global for access from other parts of the engine.
    // It is globally accessible via EngineBackend::global() afterwards,
    // so it does not need to be stored in InitContext.
    engine_backend::EngineBackend::set_global(backend);

    Ok(())
}
