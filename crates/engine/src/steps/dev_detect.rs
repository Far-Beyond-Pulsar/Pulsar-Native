//! Dev/source detection step: must run before `set_global`.

use crate::init::{InitContext, InitError};
use crate::Assets;

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let engine_context = ctx
        .engine_context
        .as_ref()
        .ok_or(InitError::MissingContext("Engine context not initialized"))?;

    let dev = engine_state::DevContext::detect();
    if dev.is_source_build {
        tracing::info!(
            "Source build detected — workspace root: {:?}",
            dev.source_path
        );
    } else {
        tracing::debug!("Running from installed/distributed binary");
    }
    engine_context
        .store
        .get_or_init::<engine_state::DevContext>()
        .set(dev);

    // Stash the embedded default level bytes so the level editor can
    // seed new projects without depending on the engine crate directly.
    if let Some(file) = Assets::get("default.level") {
        tracing::info!("Embedded default.level found ({} bytes)", file.data.len());
        engine_context
            .store
            .get_or_init::<Option<Vec<u8>>>()
            .set(Some(file.data.into_owned()));
    } else {
        tracing::debug!("No embedded default.level — new projects start empty");
    }

    Ok(())
}
